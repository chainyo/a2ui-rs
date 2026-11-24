//! Checks the upstream submodule and inventories pending conformance cases.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use a2ui_rs::data_model::DataModel;
use a2ui_rs::protocol::{AgentToRenderer, AgentToRendererMessage, Component};
use a2ui_rs::surface::{SurfaceError, SurfaceStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[path = "upstream/accessibility.rs"]
mod accessibility;
#[path = "upstream/common_types.rs"]
mod common_types;
#[path = "upstream/events.rs"]
mod events;
#[path = "upstream/nodes.rs"]
mod nodes;
#[path = "upstream/resolution.rs"]
mod resolution;
#[path = "upstream/routing.rs"]
mod routing;
#[path = "upstream/rpc.rs"]
mod rpc;
#[path = "upstream/validation.rs"]
mod validation;

type TestError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct Case {
    name: String,
    #[serde(rename = "protocolVersion")]
    protocol_version: Option<String>,
    #[serde(default)]
    action: String,
    fixture: Option<PathBuf>,
    definition: Option<String>,
    catalog: Option<Value>,
    #[serde(rename = "catalogId")]
    catalog_id: Option<String>,
    #[serde(rename = "catalogPath")]
    catalog_path: Option<String>,
    #[serde(rename = "catalogPaths", default)]
    catalog_paths: Vec<String>,
    #[serde(rename = "strictMode")]
    strict_mode: Option<bool>,
    #[serde(rename = "validationConfig")]
    validation_config: Option<a2ui_rs::validation::ValidationConfig>,
    expect: Option<Value>,
    #[serde(rename = "expectFile")]
    expect_file: Option<String>,
    #[serde(rename = "builtinCatalog")]
    builtin_catalog: Option<String>,
    #[serde(rename = "expectSelected")]
    expect_selected: Option<String>,
    assertions: Option<Value>,
    messages: Option<Value>,
    payload: Option<Value>,
    #[serde(rename = "expectError")]
    expect_error: Option<Value>,
    input: Option<Value>,
    args: Option<Value>,
    #[serde(rename = "surfaceId")]
    surface_id: Option<String>,
    #[serde(rename = "actionPayload")]
    action_payload: Option<Value>,
    #[serde(rename = "sourceComponentId")]
    source_component_id: Option<String>,
    scope: Option<String>,
    metadata: Option<Value>,
    #[serde(rename = "userActivation")]
    user_activation: Option<bool>,
    #[serde(rename = "expectDispatched")]
    expect_dispatched: Option<Value>,
    #[serde(rename = "expectDataModel")]
    expect_data_model: Option<Value>,
    function: Option<String>,
    surface: Option<Value>,
    catalogs: Option<Value>,
    #[serde(rename = "dataModel")]
    data_model: Option<Value>,
    #[serde(rename = "expect_error")]
    parse_error: Option<Value>,
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
        let cases = if suite_name == "core/common_types.yaml"
            || is_data_model_suite(&suite_name)
            || suite_name == SURFACE_SUITE
        {
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
            let (status, reason) = if is_legacy_suite(suite)
                || is_legacy_case(&suite_name, &case)
                || nodes::uses_legacy_fixture(&upstream, &suite_name, &case)?
            {
                out_of_scope += 1;
                ("skipped", Some("legacy protocol outside scope".to_owned()))
            } else if supports_case(&suite_name, &case) {
                match execute_supported_case(&upstream, &suite_name, &case) {
                    Ok(()) => {
                        if let Some(reason) = pending_after_execution(&suite_name, &case) {
                            pending += 1;
                            ("skipped", Some(reason.to_owned()))
                        } else {
                            passed += 1;
                            ("passed", None)
                        }
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
                harness: "a2ui-rs-protocol",
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
    fs::write(target.join("protocol.jsonl"), manifest)?;
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

fn is_data_model_suite(suite: &str) -> bool {
    matches!(
        suite,
        "core/data_model_pointers.yaml" | "core/data_deletion.yaml"
    )
}

const SURFACE_SUITE: &str = "core/message_processor_v1_0.yaml";

fn pending_after_execution(suite: &str, case: &Case) -> Option<&'static str> {
    if rpc::activation_fixture(suite, case) {
        Some(
            "Verified catalog rejection and local user activation. The fixture declares agent-callable activation, which v1.0 forbids.",
        )
    } else if routing::non_json_argument_fixture(suite, case) {
        Some(
            "Verified that non-finite arguments cannot enter the JSON protocol. The fixture expects a later routing diagnostic.",
        )
    } else if accessibility::supports(suite, case) {
        Some("Core assertions passed. Platform accessibility and axe-core assertions pending.")
    } else if suite == "core/validation_result.yaml"
        && case.name == "test_validation_result_multiple_rules_preserve_order"
    {
        Some(
            "Protocol assertions passed. Fixture uses the fallback message in place of the function message.",
        )
    } else {
        None
    }
}

fn supports_case(suite: &str, case: &Case) -> bool {
    if accessibility::supports(suite, case)
        || common_types::supports(suite, case)
        || events::supports(suite, case)
        || nodes::supports(suite, case)
        || validation::supports(suite, case)
        || resolution::supports(suite, case)
        || rpc::supports(suite, case)
        || routing::supports(suite, case)
    {
        return true;
    }
    if suite == SURFACE_SUITE {
        return supports_direct_surface_case(case);
    }

    is_data_model_suite(suite)
        && case.action == "validate"
        && case
            .catalog
            .as_ref()
            .and_then(|catalog| catalog["protocolVersion"].as_str())
            == Some("1.0")
}

fn supports_direct_surface_case(case: &Case) -> bool {
    case.action == "process_messages"
        && case.protocol_version.as_deref() == Some("1.0")
        && matches!(
            case.name.as_str(),
            "test_v10_create_surface_inline_initialization"
                | "test_v10_create_surface_optional_catalog_id"
                | "test_v10_create_surface_with_metadata_extensions"
                | "test_v10_create_surface_metadata_extension_key_must_be_identifier"
                | "test_v10_create_surface_metadata_only_holds_extensions"
                | "test_v10_update_data_model_deletion"
                | "test_update_data_model_on_deleted_surface_error"
                | "test_delete_non_existent_surface_is_safe_noop"
                | "test_v10_create_surface_theme_rejected_strict"
                | "test_batch_multi_stage_lifecycle_pipeline"
                | "test_update_components_on_deleted_surface_error"
                | "test_v10_update_components_stores_at_prefixed_component_metadata"
                | "test_v10_update_components_rejects_null_at_catalog_id"
        )
}

fn execute_supported_case(upstream: &Path, suite: &str, case: &Case) -> Result<(), TestError> {
    if accessibility::supports(suite, case) {
        accessibility::execute(upstream, case)
    } else if common_types::supports(suite, case) {
        common_types::execute(upstream, case)
    } else if events::supports(suite, case) {
        events::execute(upstream, case)
    } else if nodes::supports(suite, case) {
        nodes::execute(upstream, case)
    } else if validation::supports(suite, case) {
        validation::execute(upstream, suite, case)
    } else if resolution::supports(suite, case) {
        resolution::execute(upstream, suite, case)
    } else if rpc::supports(suite, case) {
        rpc::execute(upstream, case)
    } else if routing::supports(suite, case) {
        routing::execute(upstream, case)
    } else if is_data_model_suite(suite) {
        execute_data_model_case(case)
    } else if suite == SURFACE_SUITE {
        execute_surface_case(case)
    } else {
        Err("unsupported conformance case reached execution".into())
    }
}

fn execute_surface_case(case: &Case) -> Result<(), TestError> {
    let mut store = SurfaceStore::default();
    if let Some(messages) = &case.messages {
        if !case.steps.is_empty() {
            return Err("fixture has both messages and steps".into());
        }
        return execute_surface_step(
            &mut store,
            messages,
            case.expect.as_ref(),
            case.expect_error.as_ref(),
        );
    }
    if case.steps.is_empty() {
        return Err("fixture has no surface messages".into());
    }
    for step in &case.steps {
        execute_surface_step(
            &mut store,
            step.get("messages").ok_or("missing surface messages")?,
            step.get("expect"),
            step.get("expectError"),
        )?;
    }
    Ok(())
}

fn execute_surface_step(
    store: &mut SurfaceStore,
    messages: &Value,
    expect: Option<&Value>,
    expect_error: Option<&Value>,
) -> Result<(), TestError> {
    let messages = messages.as_array().ok_or("expected a message array")?;
    if messages.is_empty() || (expect.is_none() && expect_error.is_none()) {
        return Err("missing messages or expectations".into());
    }
    let mut rejected = false;
    for (index, value) in messages.iter().enumerate() {
        let before = store.clone();
        let failure = match serde_json::from_value(value.clone()) {
            Err(error) => Some(("ValidationError", error.to_string())),
            Ok(message) => match store.apply(message) {
                Ok(()) => None,
                Err(
                    error @ (SurfaceError::UnknownSurface(_)
                    | SurfaceError::DuplicateSurface(_)
                    | SurfaceError::DuplicateComponent(_)),
                ) => Some(("IntegrityError", error.to_string())),
                Err(error) => return Err(error.into()),
            },
        };
        if let Some((category, message)) = failure {
            let expected = expect_error.ok_or_else(|| message.clone())?;
            if expected.as_object().is_none_or(|fields| fields.len() != 1)
                || expected["category"] != category
                || index + 1 != messages.len()
                || *store != before
            {
                return Err(format!("unexpected surface failure: {category}: {message}").into());
            }
            rejected = true;
        }
    }
    if expect_error.is_some() && !rejected {
        return Err("expected a surface message to fail".into());
    }
    if let Some(expect) = expect {
        let surfaces = expect["surfaces"]
            .as_object()
            .ok_or("missing surface expectations")?;
        if expect.as_object().is_none_or(|fields| fields.len() != 1) || surfaces.is_empty() {
            return Err("unsupported surface expectation".into());
        }
        for (id, fields) in surfaces {
            for (field, expected) in fields.as_object().ok_or("invalid surface expectation")? {
                let surface = store.get(id);
                let actual = match (field.as_str(), surface) {
                    ("exists", _) => Value::Bool(surface.is_some()),
                    ("sendDataModel", Some(surface)) => Value::Bool(surface.send_data_model()),
                    ("components", Some(surface)) => {
                        assert_surface_components(surface.components(), expected)?;
                        continue;
                    }
                    ("dataModel", Some(surface)) => surface.data_model().root().clone(),
                    _ => return Err(format!("cannot verify {field} on {id}").into()),
                };
                if actual != *expected {
                    return Err(format!("{id}.{field}: expected {expected}, got {actual}").into());
                }
            }
        }
    }
    Ok(())
}

fn assert_surface_components(actual: &[Component], expected: &Value) -> Result<(), TestError> {
    let expected = expected.as_array().ok_or("expected a component array")?;
    if actual.len() != expected.len() {
        return Err(format!(
            "expected {} components, got {}",
            expected.len(),
            actual.len()
        )
        .into());
    }
    for (actual, expected) in actual.iter().zip(expected) {
        let fields = expected
            .as_object()
            .ok_or("invalid component expectation")?;
        let serialized = serde_json::to_value(actual)?;
        for (field, value) in fields {
            let actual_field = serialized.get(field);
            if actual_field != Some(value)
                && !(field == "@metadata" && value.is_null() && actual_field.is_none())
            {
                return Err(format!(
                    "component {}.{field}: expected {value}, got {actual_field:?}",
                    actual.id
                )
                .into());
            }
        }
    }
    Ok(())
}

fn execute_data_model_case(case: &Case) -> Result<(), TestError> {
    let mut surface = None;
    for step in &case.steps {
        let messages = step
            .get("payload")
            .and_then(Value::as_array)
            .ok_or("missing payload")?;
        if messages.is_empty() {
            return Err("empty data model payload".into());
        }
        let mut rejected = false;
        for (index, message) in messages.iter().enumerate() {
            let AgentToRendererMessage::V1(operation) = serde_json::from_value(message.clone())?;
            match operation {
                AgentToRenderer::CreateSurface(create)
                    if surface.is_none() && create.components.is_none() =>
                {
                    surface = Some((
                        create.surface_id,
                        DataModel::new(create.data_model.unwrap_or_default()),
                    ));
                }
                AgentToRenderer::UpdateDataModel(update) => {
                    let (surface_id, model) = surface.as_mut().ok_or("update before create")?;
                    if *surface_id != update.surface_id {
                        return Err("unexpected surface ID".into());
                    }
                    let before = model.clone();
                    let result = model.set(update.path.as_deref().unwrap_or("/"), update.value);
                    if let Err(error) = result {
                        let Some(expected) = step.get("expectError") else {
                            return Err(error.into());
                        };
                        let expected_message = expected["message"]
                            .as_str()
                            .ok_or("missing error message")?;
                        if index + 1 != messages.len()
                            || expected["category"] != "DataError"
                            || !error.to_string().contains(expected_message)
                            || *model != before
                        {
                            return Err(format!("unexpected data model failure: {error}").into());
                        }
                        rejected = true;
                        break;
                    }
                }
                _ => return Err("unsupported data model fixture operation".into()),
            }
        }
        if step.get("expectError").is_some() && !rejected {
            return Err("expected a data model update to fail".into());
        }
    }
    let (surface_id, model) = surface.ok_or("missing surface setup")?;
    if let Some(expect) = &case.expect {
        if expect["surfaces"][&surface_id]["dataModel"] != *model.root() {
            return Err(format!("unexpected data model: {}", model.root()).into());
        }
    } else if !case
        .steps
        .iter()
        .any(|step| step.get("expectError").is_some())
    {
        return Err("missing data model expectation".into());
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

fn is_legacy_case(suite: &str, case: &Case) -> bool {
    if is_legacy_version(case_protocol_version(suite, case)) {
        return true;
    }
    match (suite, case.name.as_str()) {
        ("core/catalog.yaml", "test_catalog_from_json_override_catalog_id")
            if case.protocol_version.is_none() =>
        {
            is_legacy_version(
                case.expect
                    .as_ref()
                    .and_then(|expect| expect["protocolVersion"].as_str()),
            )
        }
        ("core/functions.yaml", _) if case.action == "evaluate_function" => {
            case.protocol_version.is_none()
                && case.catalog.is_none()
                && case.catalog_id.is_none()
                && case.catalog_path.is_none()
                && case.catalog_paths.is_empty()
                && case.catalogs.is_none()
                && case.surface.is_none()
                && case.builtin_catalog.is_none()
                && !resolution::uses_v1_format_string_fixture(case)
        }
        ("core/actions.yaml", "test_action_direct_name_shorthand") => {
            case.protocol_version.is_none()
        }
        (
            "core/node_resolution.yaml",
            "component_type_change_replaces_node" | "delete_surface_disposes_tree",
        ) => case
            .steps
            .iter()
            .filter_map(|step| step["messages"].as_array())
            .flatten()
            .any(|message| is_legacy_version(message["version"].as_str())),
        (
            "core/message_processor_v1_0.yaml",
            "test_v10_get_renderer_data_model_excludes_surface_without_default_from_v09"
            | "test_v10_get_renderer_data_model_filters_by_surface_protocol_version",
        ) => is_legacy_version(case.args.as_ref().and_then(|args| args["version"].as_str())),
        _ => false,
    }
}

fn is_legacy_version(version: Option<&str>) -> bool {
    matches!(
        version,
        Some("0.8" | "v0.8" | "0.9" | "v0.9" | "0.9.1" | "v0.9.1")
    )
}

fn case_protocol_version<'a>(suite: &str, case: &'a Case) -> Option<&'a str> {
    case.protocol_version.as_deref().or_else(|| {
        if matches!(
            suite,
            "core/catalog.yaml" | "core/data_model.yaml" | "core/data_context.yaml"
        ) {
            case.catalog.as_ref()?.get("protocolVersion")?.as_str()
        } else {
            None
        }
    })
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

#[test]
fn catalog_versions_apply_when_the_case_omits_its_version() -> Result<(), TestError> {
    let cases = parse_cases(
        r#"
- name: legacy_data
  catalog:
    protocolVersion: "v0.9"
- name: explicit_v1
  protocolVersion: "1.0"
  catalog:
    protocolVersion: "v0.9"
"#,
    )?;
    for suite in [
        "core/catalog.yaml",
        "core/data_model.yaml",
        "core/data_context.yaml",
    ] {
        assert_eq!(case_protocol_version(suite, &cases[0]), Some("v0.9"));
        assert_eq!(case_protocol_version(suite, &cases[1]), Some("1.0"));
    }
    assert_eq!(
        case_protocol_version("core/error_responses.yaml", &cases[0]),
        None
    );
    Ok(())
}

#[test]
fn unversioned_standalone_function_cases_use_the_legacy_default() -> Result<(), TestError> {
    let cases = parse_cases(
        r#"
- name: standalone
  action: evaluate_function
- name: versioned
  action: evaluate_function
  protocolVersion: "1.0"
- name: catalog_override
  action: evaluate_function
  catalogPaths: ["catalogs/basic/v1/catalog.json"]
- name: test_evaluate_function_format_string_basic
  action: evaluate_function
"#,
    )?;
    assert!(is_legacy_case("core/functions.yaml", &cases[0]));
    for case in &cases[1..] {
        assert!(!is_legacy_case("core/functions.yaml", case));
    }
    Ok(())
}
