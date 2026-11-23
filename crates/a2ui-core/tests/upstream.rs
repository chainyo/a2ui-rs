//! Checks the upstream submodule and inventories pending conformance cases.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;

use serde::Deserialize;

type TestError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct Case {
    name: String,
    #[serde(rename = "protocolVersion")]
    protocol_version: Option<String>,
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
    let mut pending = 0;
    let mut out_of_scope = 0;
    for suite in &suites {
        let suite_name = suite.strip_prefix(&upstream)?.display();
        let source = fs::read_to_string(suite)?;
        let cases = parse_cases(&source)?;
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
            if is_legacy_suite(suite)
                || matches!(
                    version,
                    Some("0.8" | "v0.8" | "0.9" | "v0.9" | "0.9.1" | "v0.9.1")
                )
            {
                println!(
                    "OUT_OF_SCOPE {}::{} (legacy protocol)",
                    suite_name, case.name
                );
                out_of_scope += 1;
            } else {
                println!("PENDING {suite_name}::{}", case.name);
                pending += 1;
            }
        }
    }
    if pending == 0 {
        return Err("no upstream conformance cases found".into());
    }
    println!(
        "Upstream {revision}: {} Core suites. Behavioral coverage: 0 executed, {pending} pending, {out_of_scope} legacy cases outside scope.",
        suites.len()
    );
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
    // Read case metadata only. The upstream input remains unchanged.
    let mut options = serde_saphyr::Options::default();
    options.non_finite_float_policy = serde_saphyr::NonFiniteFloatPolicy::AsString;
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
