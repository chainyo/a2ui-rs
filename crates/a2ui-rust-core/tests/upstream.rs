//! Checks the upstream submodule and inventories pending conformance cases.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use a2ui_rust_core::wire::{Extensions, FunctionResponse};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

type TestError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct Case {
    name: String,
    #[serde(rename = "protocolVersion")]
    protocol_version: Option<String>,
    #[serde(default)]
    action: String,
    definition: Option<String>,
    #[serde(default)]
    steps: Vec<Value>,
}

#[derive(Serialize)]
struct Record {
    harness: &'static str,
    suite: String,
    case: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[test]
fn upstream_submodule_and_case_inventory() -> Result<(), TestError> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Deep upstream fixtures exceed the default Windows thread stack.
    let worker = thread::Builder::new()
        .name("upstream-audit".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || audit(&root))?;
    worker
        .join()
        .map_err(|_| "upstream audit worker panicked")?
}

fn audit(root: &Path) -> Result<(), TestError> {
    let revision = verify_submodule(root)?;
    let upstream = root.join("fixtures/upstream");
    let suites = core_suites(&upstream)?;
    let mut pending = 0;
    let mut passed = 0;
    let mut failures = Vec::new();
    let mut records = Vec::new();
    let mut out_of_scope = 0;
    for suite in &suites {
        let suite_name = suite
            .strip_prefix(upstream.join("conformance"))?
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(suite)?;
        let cases = if suite_name == "core/common_types.yaml" {
            parse_cases_with_policy(&source, serde_saphyr::NonFiniteFloatPolicy::Reject)?
        } else {
            parse_cases(&source)?
        };
        if cases.is_empty() {
            return Err(format!("empty upstream suite: {}", suite.display()).into());
        }
        let mut names = BTreeSet::new();
        for case in cases {
            if case.name.is_empty() || !names.insert(case.name.clone()) {
                return Err(
                    format!("missing or duplicate case name in {}", suite.display()).into(),
                );
            }
            let version = case.protocol_version.as_deref();
            let (status, reason) = if is_legacy_suite(suite)
                || matches!(
                    version,
                    Some("0.8" | "v0.8" | "0.9" | "v0.9" | "0.9.1" | "v0.9.1")
                ) {
                out_of_scope += 1;
                ("skipped", Some("legacy protocol outside scope".to_owned()))
            } else if case.protocol_version.as_deref() == Some("1.0")
                && suite_name == "core/common_types.yaml"
                && case.action == "validate_common_type"
                && matches!(
                    case.definition.as_deref(),
                    Some("FunctionResponse" | "Extensions")
                )
            {
                match execute_case(&case) {
                    Ok(()) => {
                        passed += 1;
                        ("passed", None)
                    }
                    Err(error) => {
                        failures.push(format!("{suite_name}::{}: {error}", case.name));
                        ("failed", Some(error.to_string()))
                    }
                }
            } else {
                pending += 1;
                (
                    "skipped",
                    Some(format!(
                        "pending implementation: {} {}",
                        case.action,
                        case.definition.as_deref().unwrap_or("")
                    )),
                )
            };
            records.push(Record {
                harness: "a2ui-rust-core-wire",
                suite: suite_name.clone(),
                case: case.name,
                status,
                reason,
            });
        }
    }
    if pending == 0 {
        return Err("no upstream conformance cases found".into());
    }
    let target = root.join("target/conformance");
    fs::create_dir_all(&target)?;
    let manifest = records
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n")
        + "\n";
    fs::write(target.join("wire.jsonl"), manifest)?;
    println!(
        "Upstream {revision}: {} Core suites. {passed} passed, {} failed, {pending} pending, {out_of_scope} legacy cases outside scope.",
        suites.len(),
        failures.len()
    );
    if passed == 0 {
        return Err("no supported conformance case executed".into());
    }
    if !failures.is_empty() {
        return Err(failures.join("\n").into());
    }
    Ok(())
}

fn core_suites(upstream: &Path) -> Result<Vec<PathBuf>, TestError> {
    for reference in [
        "blueprints/modules/a2ui_core.blueprint.md",
        "blueprints/modules/a2ui_agent.blueprint.md",
        "blueprints/modules/a2ui_framework_adapter.blueprint.md",
        "specification/v1_0/docs/a2ui_protocol.md",
        "specification/v1_0/json/agent_to_renderer.json",
        "specification/v1_0/json/renderer_to_agent.json",
        "catalogs/basic/v1/catalog.json",
    ] {
        if !upstream.join(reference).is_file() {
            return Err(format!("missing upstream reference: {reference}").into());
        }
    }
    let mut suites = fs::read_dir(upstream.join("conformance/core"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    suites.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "yaml")
    });
    suites.sort();
    Ok(suites)
}

fn execute_case(case: &Case) -> Result<(), TestError> {
    if case.steps.is_empty() {
        return Err("supported case has no steps".into());
    }
    for step in &case.steps {
        match case.definition.as_deref() {
            Some("FunctionResponse") => validate_step::<FunctionResponse>(step)?,
            Some("Extensions") => validate_step::<Extensions>(step)?,
            _ => return Err("unsupported definition reached execution".into()),
        }
    }
    Ok(())
}

fn validate_step<T: DeserializeOwned + Serialize>(step: &Value) -> Result<(), TestError> {
    let value = step.get("value").ok_or("step has no value")?;
    let native = serde_json::from_value::<T>(value.clone());
    if let Some(expected) = step.get("expectError") {
        if expected.get("category").and_then(Value::as_str) != Some("ValidationError") {
            return Err("unsupported error expectation".into());
        }
        if native.is_ok() {
            return Err(format!("accepted invalid input: {value}").into());
        }
    } else if serde_json::to_value(native?)? != *value {
        return Err("serialization did not preserve the wire value".into());
    }
    Ok(())
}

fn verify_submodule(root: &Path) -> Result<String, TestError> {
    let entry = git_output(root, &["ls-files", "--stage", "--", "fixtures/upstream"])?;
    let fields: Vec<_> = entry.split_whitespace().collect();
    if fields.len() != 4 || fields[0] != "160000" || fields[2] != "0" {
        return Err("expected a pinned fixtures/upstream Git submodule".into());
    }
    let expected = fields[1];
    let upstream = root.join("fixtures/upstream");
    let actual = git_output(&upstream, &["rev-parse", "HEAD"])?;
    if actual.trim() != expected {
        return Err(
            "upstream checkout differs from the pin. Run git submodule update --init".into(),
        );
    }
    let status = git_output(&upstream, &["status", "--porcelain"])?;
    if !status.is_empty() {
        return Err("upstream submodule has local changes".into());
    }
    Ok(expected.to_owned())
}

fn git_output(directory: &Path, arguments: &[&str]) -> Result<String, TestError> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed. Initialize references with git submodule update --init: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn is_legacy_suite(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| {
            ["_v0_8", "_v0_9", "_v0_9_1"]
                .iter()
                .any(|version| stem.ends_with(version))
        })
}

fn parse_cases(source: &str) -> Result<Vec<Case>, serde_saphyr::Error> {
    parse_cases_with_policy(source, serde_saphyr::NonFiniteFloatPolicy::AsString)
}

fn parse_cases_with_policy(
    source: &str,
    policy: serde_saphyr::NonFiniteFloatPolicy,
) -> Result<Vec<Case>, serde_saphyr::Error> {
    // Only inventory inputs permit non-finite values. Executed inputs use JSON types.
    let mut options = serde_saphyr::Options::default();
    options.non_finite_float_policy = policy;
    // Legacy rejection fixtures deliberately exceed the parser's default depth.
    let mut budget = serde_saphyr::budget::Budget::default();
    budget.max_depth = 256;
    options.budget = Some(budget);
    serde_saphyr::from_str_with_options(source, options)
}

#[test]
fn inventory_keeps_negative_cases_with_non_finite_inputs() -> Result<(), TestError> {
    let cases = parse_cases(
        r#"
- name: rejects_non_finite_amount
  protocolVersion: "1.0"
  strictMode: true
  steps:
    - value: {amount: .nan}
      expectError: {category: ValidationError}
- name: rejects_recursive_graph
  protocolVersion: "1.0"
  expectError: {category: RecursionError}
"#,
    )?;
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0].name, "rejects_non_finite_amount");
    assert_eq!(cases[1].name, "rejects_recursive_graph");
    Ok(())
}

#[test]
fn dedicated_legacy_suites_remain_outside_scope() {
    for suite in ["message_processor_v0_8.yaml", "validator_v0_9.yaml"] {
        assert!(is_legacy_suite(Path::new(suite)));
    }
    for suite in ["message_processor_v1_0.yaml", "common_types.yaml"] {
        assert!(!is_legacy_suite(Path::new(suite)));
    }
}
