//! Verifies upstream references and inventories pending conformance cases.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Component, Path};
use std::thread;

use serde::Deserialize;
use sha2::{Digest, Sha256};

type TestError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    repository: String,
    revision: String,
    files: Vec<ReferenceFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceFile {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    #[serde(rename = "protocolVersion")]
    protocol_version: Option<String>,
}

#[test]
fn upstream_references_match_the_pinned_inventory() -> Result<(), TestError> {
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
    let upstream = root.join("fixtures/upstream");
    let manifest: Manifest = serde_json::from_slice(&fs::read(upstream.join("manifest.json"))?)?;
    if manifest.repository != "https://github.com/a2ui-project/a2ui"
        || manifest.revision.len() != 40
        || !manifest
            .revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || manifest.files.is_empty()
    {
        return Err("invalid upstream provenance".into());
    }
    let mut paths = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut pending = 0;
    let mut out_of_scope = 0;
    for reference in &manifest.files {
        let path = Path::new(&reference.path);
        if !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
            || !paths.insert(reference.path.clone())
        {
            return Err(format!("invalid or duplicate reference path: {}", reference.path).into());
        }
        let data = fs::read(upstream.join(path))?;
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(&data) {
            write!(digest, "{byte:02x}")?;
        }
        if digest != reference.sha256 {
            return Err(format!("upstream reference changed: {}", reference.path).into());
        }
        if reference.path.starts_with("conformance/core/")
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("yaml"))
        {
            let cases: Vec<Case> = parse_cases(std::str::from_utf8(&data)?)?;
            for case in cases {
                let name = case.name;
                if name.is_empty() {
                    return Err("case without name".into());
                }
                if !names.insert((reference.path.clone(), name.clone())) {
                    return Err(format!("duplicate upstream case: {name}").into());
                }
                let version = case.protocol_version.as_deref();
                if matches!(
                    version,
                    Some("0.8" | "v0.8" | "0.9" | "v0.9" | "0.9.1" | "v0.9.1")
                ) {
                    println!("OUT_OF_SCOPE {}::{name} (legacy protocol)", reference.path);
                    out_of_scope += 1;
                } else {
                    println!("PENDING {}::{name}", reference.path);
                    pending += 1;
                }
            }
        }
    }
    if pending == 0 {
        return Err("no upstream conformance cases found".into());
    }
    println!(
        "Verified {} reference files at {}. Behavioral coverage: 0 executed, {pending} pending, {out_of_scope} legacy cases outside scope.",
        manifest.files.len(),
        manifest.revision
    );
    Ok(())
}

fn parse_cases(source: &str) -> Result<Vec<Case>, serde_saphyr::Error> {
    // Read case metadata only. The original input bytes remain unchanged.
    let mut options = serde_saphyr::Options::default();
    options.non_finite_float_policy = serde_saphyr::NonFiniteFloatPolicy::AsString;
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
