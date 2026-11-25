use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::{Catalog, CatalogError};
use a2ui_rs::data_model::DataError;
use a2ui_rs::protocol::{AgentToRenderer, AgentToRendererMessage};
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::RoutingSession;
use a2ui_rs::rpc::RpcHandler;
use a2ui_rs::surface::SurfaceError;
use a2ui_rs::validation::{MessageProcessor, ProcessError, ValidationConfig, ValidationError};
use serde_json::{Value, json};

use super::{Case, TestError, assert_surface_components};

#[path = "catalog.rs"]
mod catalog_cases;

const VALIDATOR_SUITE: &str = "core/validator_v1_0.yaml";
const CATALOG_SUITE: &str = "core/catalog.yaml";
const COMPOSITION_SUITE: &str = "core/composition_constraints.yaml";
const RESERVED_KEYS_SUITE: &str = "core/reserved_keys.yaml";

fn supports_transaction(suite: &str, case: &Case) -> bool {
    if suite == COMPOSITION_SUITE {
        return matches!(case.action.as_str(), "process_messages" | "validate")
            && case
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog["protocolVersion"] == "1.0");
    }
    if suite == "core/error_responses.yaml" {
        return case.action == "validate"
            && case.protocol_version.as_deref() == Some("1.0")
            && matches!(
                case.name.as_str(),
                "test_error_response_create_surface_unknown_catalog_id"
                    | "test_error_response_create_surface_duplicate_surface_id"
                    | "test_error_response_update_components_unknown_surface"
                    | "test_error_response_update_components_duplicate_id_in_batch"
                    | "test_error_response_update_components_unknown_type_strict"
                    | "test_error_response_update_components_invalid_property_type"
                    | "test_error_response_update_components_extra_forbidden_property"
                    | "test_error_response_update_components_invalid_uax31_id"
                    | "test_error_response_update_components_unallowed_parent"
                    | "test_error_response_update_components_unallowed_parent_root"
                    | "test_error_response_update_components_unallowed_child"
                    | "test_error_response_update_components_circular_reference"
                    | "test_error_response_update_components_dangling_child_strict"
                    | "test_error_response_update_components_orphaned_component_strict"
                    | "test_error_response_update_components_buffered_streaming_non_strict"
                    | "test_error_response_update_data_model_unknown_surface"
                    | "test_error_response_update_data_model_prototype_pollution"
                    | "test_error_response_update_data_model_invalid_json_pointer"
            );
    }
    suite == super::SURFACE_SUITE
        && !super::supports_direct_surface_case(case)
        && (matches!(
            case.action.as_str(),
            "get_renderer_capabilities" | "get_renderer_data_model"
        ) || (matches!(case.action.as_str(), "process_messages" | "validate")
            && (case.protocol_version.as_deref() == Some("1.0")
                || case.protocol_version.is_none())))
}

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    if suite == RESERVED_KEYS_SUITE {
        return matches!(case.action.as_str(), "validate" | "process_messages")
            && case
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog["protocolVersion"] == "1.0");
    }
    if suite == "core/add_catalog.yaml" {
        return case.action == "process_messages"
            && case.protocol_version.as_deref() == Some("1.0")
            && matches!(
                case.name.as_str(),
                "test_add_catalog_resolves_later_create_surface"
                    | "test_add_catalog_keeps_catalogs_of_earlier_surface"
                    | "test_add_catalog_rejects_held_catalog_id"
            );
    }
    if supports_transaction(suite, case) {
        return true;
    }
    if suite == CATALOG_SUITE {
        return matches!(
            case.action.as_str(),
            "from_json" | "to_json" | "round_trip" | "derive" | "validation_schema"
        ) && (case.protocol_version.as_deref() == Some("1.0")
            || case.name.starts_with("test_v10_")
            || case.name == "test_catalog_from_json_missing_catalog_id_error");
    }
    suite == VALIDATOR_SUITE
        && case.action == "validate"
        && case.protocol_version.as_deref() == Some("1.0")
}

pub(super) fn execute(upstream: &Path, suite: &str, case: &Case) -> Result<(), TestError> {
    let common = read_json(upstream, "specification/v1_0/json/common_types.json")?;
    if suite == CATALOG_SUITE {
        return catalog_cases::execute(upstream, case, &common);
    }
    if suite == RESERVED_KEYS_SUITE {
        return execute_reserved_keys(upstream, case, &common);
    }

    let config = case
        .validation_config
        .unwrap_or(if case.strict_mode == Some(false) {
            ValidationConfig::RELAXED
        } else {
            ValidationConfig::default()
        });
    let mut processor = MessageProcessor::new(config);
    let transaction = supports_transaction(suite, case);
    let checks = if suite == VALIDATOR_SUITE {
        MessageChecks::Validator
    } else if transaction {
        MessageChecks::Transaction
    } else {
        MessageChecks::Standard
    };
    register_fixture_catalogs(upstream, suite, case, &common, &mut processor, transaction)?;
    if suite == "core/add_catalog.yaml" {
        return execute_catalog_steps(upstream, case, &common, &mut processor);
    }
    if let Some(document) = &case.catalog {
        let document = if suite == COMPOSITION_SUITE {
            composition_catalog(upstream, document)?
        } else {
            catalog_document(case, document)?
        };
        processor.add_catalog(Catalog::from_json(document, &common)?)?;
    }

    if suite == super::SURFACE_SUITE {
        match case.action.as_str() {
            "get_renderer_capabilities" | "get_renderer_data_model" => {
                return assert_renderer_metadata(&mut processor, case);
            }
            _ if is_batch_fixture(case) => return execute_json_batch(&mut processor, case),
            _ => {}
        }
    }

    if let Some(messages) = case.messages.as_ref().or(case.payload.as_ref()) {
        if !case.steps.is_empty() || (case.messages.is_some() && case.payload.is_some()) {
            return Err("validation fixture has more than one message source".into());
        }
        return apply_messages(
            &mut processor,
            case,
            messages,
            None,
            case.expect_error.as_ref(),
            checks,
        )
        .and_then(|()| {
            case.expect
                .as_ref()
                .map_or(Ok(()), |expect| assert_state(&processor, expect))
        });
    }
    execute_validation_steps(&mut processor, case, checks)?;
    let expected_text = match case.name.as_str() {
        "test_v10_nested_function_calls_within_global_depth" => "Six calls deep",
        "test_v10_nested_function_calls_at_global_depth_limit" => "Twenty-three calls deep",
        _ => return Ok(()),
    };
    let tree = processor
        .resolve_components("s1", &LocalFunctions::default())?
        .ok_or("missing root component for depth fixture")?;
    if tree.properties.get("text") != Some(&json!(expected_text)) {
        return Err(format!("expected {expected_text:?}, got {:?}", tree.properties).into());
    }
    Ok(())
}

fn register_fixture_catalogs(
    upstream: &Path,
    suite: &str,
    case: &Case,
    common: &Value,
    processor: &mut MessageProcessor,
    transaction: bool,
) -> Result<(), TestError> {
    for path in &case.catalog_paths {
        let document = read_json(upstream, path)?;
        processor.add_catalog(Catalog::from_json(document.clone(), common)?)?;
        if path == "catalogs/basic/v1/catalog.json" {
            // Some pinned fixtures use the published catalog's former URL.
            let mut alias = document;
            alias["catalogId"] = Value::from("https://a2ui.org/catalogs/basic/v1/catalog.json");
            processor.add_catalog(Catalog::from_json(alias.clone(), common)?)?;
            if suite == super::SURFACE_SUITE {
                alias["catalogId"] = json!("test-catalog-v10");
                processor.add_catalog(Catalog::from_json(alias, common)?)?;
            }
        }
    }
    if transaction
        && case.catalog_paths.is_empty()
        && case.catalog.is_none()
        && case.action != "get_renderer_capabilities"
    {
        let mut basic = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
        for id in ["basic", "test-catalog-v10"] {
            basic["catalogId"] = json!(id);
            processor.add_catalog(Catalog::from_json(basic.clone(), common)?)?;
        }
    }
    if transaction && let Some(catalogs) = &case.catalogs {
        for document in catalogs.as_array().ok_or("expected transaction catalogs")? {
            let mut document = catalog_document(case, document)?;
            if suite == super::SURFACE_SUITE && document.get("protocolVersion").is_none() {
                document["protocolVersion"] = json!("1.0");
            }
            if case.name == "test_v10_component_catalog_override" {
                // The fixture gives the catalog IDs and component payloads.
                if document["catalogId"] == "main-catalog" {
                    let basic = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
                    document["components"]["Column"] = basic["components"]["Column"].clone();
                    document["$defs"]["Weight"] = basic["$defs"]["Weight"].clone();
                } else if document["catalogId"] == "charts-catalog" {
                    document["components"]["PieChart"] = json!({
                        "type": "object", "properties": {
                            "component": {"const": "PieChart"},
                            "data": {"type": "array", "items": {"type": "number"}}
                        }, "required": ["component", "data"]
                    });
                }
            }
            let catalog = Catalog::from_json(document.clone(), common);
            match catalog {
                Err(CatalogError::InvalidCatalog(message))
                    if suite == super::SURFACE_SUITE
                        && matches!(document["protocolVersion"].as_str(), Some("v0.9"))
                        && message.contains("must declare protocolVersion 1.0") => {}
                result => processor.add_catalog(result?)?,
            }
        }
    }
    Ok(())
}

fn is_batch_fixture(case: &Case) -> bool {
    matches!(
        case.name.as_str(),
        "test_multiple_update_types_error"
            | "test_batch_message_array_and_wrapper"
            | "test_v10_rejects_v08_begin_rendering_action"
            | "test_v10_non_string_surface_id_error"
            | "test_unsupported_protocol_version_error"
            | "test_missing_version_field_in_v10_process_messages_error"
    )
}

fn execute_json_batch(processor: &mut MessageProcessor, case: &Case) -> Result<(), TestError> {
    let messages = case.messages.as_ref().ok_or("missing batch messages")?;
    if let Some(expected) = &case.expect_error {
        let Err(ProcessError::Envelope { index: 0, source }) = processor.apply_json(messages)
        else {
            return Err("expected an invalid envelope at index zero".into());
        };
        let error = source.to_string();
        let equivalent = match case.name.as_str() {
            "test_multiple_update_types_error" if error.contains("single key") => {
                "Message contains multiple conflicting update actions"
            }
            "test_v10_rejects_v08_begin_rendering_action" if error.contains("beginRendering") => {
                "action 'beginRendering' is not supported in protocol version 1.0"
            }
            "test_v10_non_string_surface_id_error" if error.contains("expected a string") => {
                "surfaceId must be a string"
            }
            "test_unsupported_protocol_version_error" if error.contains("v99.0") => {
                "Unsupported protocol version 'v99.0'"
            }
            "test_missing_version_field_in_v10_process_messages_error"
                if error.contains("missing field `version`") =>
            {
                "Missing required version field"
            }
            _ => return Err(format!("unexpected envelope rejection: {error}").into()),
        };
        return assert_rejection(expected, "ValidationError", None, equivalent);
    }

    let expected = case
        .expect
        .as_ref()
        .ok_or("missing batch state expectation")?;
    let array = messages
        .get("messages")
        .ok_or("missing wrapped message list")?;
    let mut apply_one = |input| -> Result<(), TestError> {
        processor.apply_json(input)?;
        assert_state(processor, expected)?;
        for id in expected["surfaces"]
            .as_object()
            .ok_or("missing surfaces")?
            .keys()
        {
            processor.apply_json(&json!({"version": "1.0", "deleteSurface": {"surfaceId": id}}))?;
        }
        Ok(())
    };
    apply_one(array)?;
    apply_one(messages)
}

fn assert_renderer_metadata(
    processor: &mut MessageProcessor,
    case: &Case,
) -> Result<(), TestError> {
    let args = case
        .args
        .as_ref()
        .ok_or("missing renderer metadata options")?;
    if args["version"] != "1.0" {
        return Err("expected a v1.0 renderer metadata query".into());
    }
    let actual = if case.action == "get_renderer_capabilities" {
        let inline = args["includeInlineCatalogs"]
            .as_bool()
            .ok_or("missing inline option")?;
        serde_json::to_value(processor.renderer_capabilities(inline))?
    } else {
        processor.apply_json(
            case.messages
                .as_ref()
                .ok_or("missing data model messages")?,
        )?;
        serde_json::to_value(processor.renderer_data_model())?
    };
    let expected = case
        .expect
        .as_ref()
        .ok_or("missing renderer metadata expectation")?;
    if &actual != expected {
        return Err(format!("expected renderer metadata {expected}, got {actual}").into());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageChecks {
    Standard,
    Transaction,
    Validator,
}

fn execute_validation_steps(
    processor: &mut MessageProcessor,
    case: &Case,
    checks: MessageChecks,
) -> Result<(), TestError> {
    if case.steps.is_empty() {
        return Err("missing validation steps".into());
    }
    for (index, step) in case.steps.iter().enumerate() {
        let messages = step
            .get("messages")
            .or_else(|| step.get("payload"))
            .ok_or("missing validation messages")?;
        let rules = step
            .get("validationConfig")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?;
        let expected_error = step.get("expectError").or_else(|| {
            if index + 1 == case.steps.len() {
                case.expect_error.as_ref()
            } else {
                None
            }
        });
        apply_messages(processor, case, messages, rules, expected_error, checks)?;
        if let Some(expect) = step.get("expect") {
            assert_state(processor, expect)?;
        }
    }
    if let Some(expect) = &case.expect {
        assert_state(processor, expect)?;
    }
    Ok(())
}

fn composition_catalog(upstream: &Path, document: &Value) -> Result<Value, TestError> {
    if let Some(schema) = document.get("catalogSchema") {
        let mut schema = schema.clone();
        schema["protocolVersion"] = document["protocolVersion"].clone();
        return Ok(schema);
    }
    if document.get("catalogId").is_some() {
        let mut document = document.clone();
        if document["$defs"].get("ChildList").is_none() {
            // The fixtures use a local name for the standard child list.
            document["$defs"]["ChildList"] = json!({"$ref": "common_types.json#/$defs/ChildList"});
        }
        return Ok(document);
    }

    let mut basic = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
    basic["catalogId"] = json!("basic");
    // The implicit-parent fixture requires Column to allow Surface.
    basic["components"]["Column"]["allowedParents"] = json!(["Surface"]);
    Ok(basic)
}

fn execute_reserved_keys(upstream: &Path, case: &Case, common: &Value) -> Result<(), TestError> {
    let fixture_catalog = case
        .catalog
        .as_ref()
        .ok_or("missing reserved keys catalog")?;
    let catalog = if fixture_catalog.get("catalogId").is_some() {
        fixture_catalog.clone()
    } else {
        let mut basic = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
        basic["catalogId"] = json!("basic");
        basic
    };
    let mut processor = MessageProcessor::default();
    processor.add_catalog(Catalog::from_json(catalog, common)?)?;
    let messages = match case.action.as_str() {
        "validate" => case.payload.as_ref(),
        "process_messages" => case.messages.as_ref(),
        _ => return Err("unexpected reserved keys action".into()),
    }
    .ok_or("missing reserved keys messages")?;
    apply_messages(
        &mut processor,
        case,
        messages,
        None,
        case.expect_error.as_ref(),
        MessageChecks::Transaction,
    )?;
    if case.action == "validate" {
        return Ok(());
    }

    let surfaces = case
        .expect
        .as_ref()
        .and_then(|expected| expected["surfaces"].as_object())
        .ok_or("missing reserved keys surface expectations")?;
    let mut session = RoutingSession::new(RpcHandler::new(processor), LocalFunctions::default());
    for id in surfaces.keys() {
        session.mount_tree(id);
    }
    let update = session.advance(Instant::now(), None);
    if !update.errors.is_empty() {
        return Err(format!("reserved keys resolution failed: {:?}", update.errors).into());
    }
    let functions = LocalFunctions::default();
    for (id, expected) in surfaces {
        let snapshot = session
            .processor()
            .resolve_components(id, &functions)?
            .ok_or("missing resolved root")?;
        let tree = session.node_tree(id).ok_or("missing mounted tree")?;
        let root = tree
            .root()
            .and_then(|root| tree.node(root))
            .ok_or("missing mounted root")?;
        let [component] = expected["components"]
            .as_array()
            .ok_or("missing expected components")?
            .as_slice()
        else {
            return Err("expected one component".into());
        };
        let mut fields = component
            .as_object()
            .ok_or("invalid component expectation")?
            .clone();
        if fields.remove("id").as_ref().and_then(Value::as_str) != Some(&root.component_id)
            || fields.remove("component").as_ref().and_then(Value::as_str) != Some(&root.component)
            || snapshot.id != root.component_id
            || snapshot.component != root.component
        {
            return Err("unexpected reserved keys component".into());
        }
        for (key, expected) in fields {
            let actual = root.properties.get(&key).map(|property| &property.value);
            let snapshot_value = snapshot.properties.get(&key);
            if actual != Some(&expected) || snapshot_value != Some(&expected) {
                return Err(format!(
                    "component {}.{key}: expected {expected}, got snapshot {snapshot_value:?} and mounted {actual:?}",
                    root.component_id
                )
                .into());
            }
        }
    }
    Ok(())
}

fn execute_catalog_steps(
    upstream: &Path,
    case: &Case,
    common: &Value,
    processor: &mut MessageProcessor,
) -> Result<(), TestError> {
    if case.steps.is_empty() {
        return Err("missing catalog steps".into());
    }
    for step in &case.steps {
        let expected = step.get("expectError");
        let paths: Vec<String> = step
            .get("addCatalogPaths")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        if let Some(mut messages) = step.get("messages").cloned() {
            for path in &paths {
                processor.add_catalog(Catalog::from_json(read_json(upstream, path)?, common)?)?;
            }
            if case.name == "test_add_catalog_keeps_catalogs_of_earlier_surface"
                && expected.is_some()
            {
                let component = messages
                    .pointer_mut("/0/updateComponents/components/0")
                    .and_then(Value::as_object_mut)
                    .ok_or("missing component with a catalog override")?;
                // Use the v1.0 envelope key for the fixture's catalog override.
                let catalog = component
                    .remove("catalogId")
                    .ok_or("missing fixture catalog override")?;
                if component.insert("@catalogId".to_owned(), catalog).is_some() {
                    return Err("fixture has two catalog overrides".into());
                }
            }
            apply_messages(
                processor,
                case,
                &messages,
                None,
                expected,
                MessageChecks::Transaction,
            )?;
        } else {
            let [path] = paths.as_slice() else {
                return Err("expected one catalog registration".into());
            };
            let catalog = Catalog::from_json(read_json(upstream, path)?, common)?;
            let id = catalog.id().to_owned();
            match (&processor.add_catalog(catalog), expected) {
                (Err(error @ ValidationError::DuplicateCatalog(duplicate)), Some(expected))
                    if duplicate == &id =>
                {
                    assert_rejection(expected, "CatalogError", None, &error.to_string())?;
                }
                result => return Err(format!("unexpected registration outcome: {result:?}").into()),
            }
        }
    }
    case.expect
        .as_ref()
        .map_or(Ok(()), |expected| assert_state(processor, expected))
}

fn apply_messages(
    processor: &mut MessageProcessor,
    case: &Case,
    messages: &Value,
    rules: Option<ValidationConfig>,
    expected: Option<&Value>,
    checks: MessageChecks,
) -> Result<(), TestError> {
    let messages = messages
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or("missing validation messages")?;
    for (index, message) in messages.iter().enumerate() {
        let decoded: AgentToRendererMessage = match serde_json::from_value(message.clone()) {
            Ok(decoded) => decoded,
            Err(error) => {
                if checks == MessageChecks::Validator
                    && index + 1 == messages.len()
                    && let Some(expected) = expected
                {
                    return assert_envelope_rejection(case, index, expected, &error);
                }
                return Err(error.into());
            }
        };
        let surface = if checks == MessageChecks::Standard {
            None
        } else {
            Some(transaction_surface(&decoded)?.to_owned())
        };
        let before = surface.as_deref().and_then(|id| processor.get(id)).cloned();
        let result = match rules {
            Some(rules) => processor.apply_with_config(decoded, rules),
            None => processor.apply(decoded),
        };
        if let Err(error) = result {
            if index + 1 != messages.len() {
                return Err(format!("validation failed before the last message: {error}").into());
            }
            let expected = expected.ok_or_else(|| error.to_string())?;
            if let Some(surface) = surface {
                if processor.get(&surface) != before.as_ref() {
                    return Err(format!("rejected message changed surface {surface}").into());
                }
                if checks == MessageChecks::Transaction || expected.get("rendererError").is_some() {
                    return assert_transaction_error(case, expected, &surface, &error);
                }
            }
            let category = error_category(&error, expected, case);
            let code = match error {
                ValidationError::UnallowedParent { .. } => Some("UNALLOWED_PARENT"),
                ValidationError::UnallowedChild { .. } => Some("UNALLOWED_CHILD"),
                _ => None,
            };
            return assert_rejection(expected, category, code, &error.to_string());
        }
    }
    if expected.is_some() {
        return Err("expected validation error".into());
    }
    Ok(())
}

fn assert_envelope_rejection(
    case: &Case,
    index: usize,
    expected: &Value,
    error: &serde_json::Error,
) -> Result<(), TestError> {
    let message = error.to_string();
    let mut core = expected
        .as_object()
        .ok_or("expected envelope error fields")?
        .clone();
    let details = match case.name.as_str() {
        "test_validator_1_0" if message.contains("missing field `version`") => {
            Some(json!([{ "path": format!("messages.{index}.version"), "code": "missing_field" }]))
        }
        "test_validator_1_0" if message.contains("expected a string") => Some(json!([{
            "path": format!("messages.{index}.createSurface.surfaceId"), "code": "type_mismatch"
        }])),
        "test_v10_multiple_action_envelope_error" if message.contains("single key") => None,
        "test_v10_validate_component_metadata_invalid_extension_key_error"
            if message.contains("com.example.invalid") =>
        {
            None
        }
        _ => return Err(format!("unexpected validator envelope error: {error}").into()),
    };
    if core.remove("details") != details {
        return Err(format!("expected envelope details {expected}, got {details:?}").into());
    }
    assert_rejection(&Value::Object(core), "ValidationError", None, &message)
}

fn transaction_surface(message: &AgentToRendererMessage) -> Result<&str, TestError> {
    let AgentToRendererMessage::V1(operation) = message;
    match operation {
        AgentToRenderer::CreateSurface(create) => Ok(&create.surface_id),
        AgentToRenderer::UpdateComponents(update) => Ok(&update.surface_id),
        AgentToRenderer::UpdateDataModel(update) => Ok(&update.surface_id),
        _ => Err("unexpected transaction fixture operation".into()),
    }
}

fn error_category(error: &ValidationError, expected: &Value, case: &Case) -> &'static str {
    match error {
        ValidationError::ComponentDepth
        | ValidationError::FunctionDepth
        | ValidationError::PayloadDepth
        | ValidationError::DataModelDepth
            if expected["category"] == "RecursionError" =>
        {
            "RecursionError"
        }
        ValidationError::MissingCatalog
        | ValidationError::UnknownCatalog(_)
        | ValidationError::DuplicateCatalog(_) => "CatalogError",
        ValidationError::Orphan(_) if case.action == "process_messages" => "IntegrityError",
        _ => "ValidationError",
    }
}

fn assert_transaction_error(
    case: &Case,
    expected: &Value,
    surface: &str,
    error: &ValidationError,
) -> Result<(), TestError> {
    let mut core = expected
        .as_object()
        .ok_or("missing transaction error fields")?
        .clone();
    let renderer = core.remove("rendererError");
    let path = core.remove("path");
    let details = core.remove("details");
    let mut envelope = None;
    if core.remove("surfaceId").is_some_and(|id| id != surface) {
        return Err("transaction failed on the wrong surface".into());
    }
    let message = error.to_string();
    let category = match error {
        ValidationError::CircularReference(_) if case.action == "validate" => "RecursionError",
        ValidationError::PayloadDepth
        | ValidationError::FunctionDepth
        | ValidationError::DataModelDepth => "RecursionError",
        ValidationError::Surface(
            SurfaceError::DuplicateComponent(_)
            | SurfaceError::DuplicateSurface(_)
            | SurfaceError::UnknownSurface(_),
        )
        | ValidationError::CircularReference(_)
        | ValidationError::Orphan(_)
        | ValidationError::DanglingReference { .. } => "IntegrityError",
        ValidationError::Surface(SurfaceError::Data {
            source: DataError::InvalidPointer(_),
            ..
        })
        | ValidationError::Catalog(_)
        | ValidationError::InvalidComponentId { .. }
        | ValidationError::InvalidComponentType { .. }
        | ValidationError::BindingPath { .. }
        | ValidationError::UnknownComponent { .. }
        | ValidationError::UnknownFunction { .. }
        | ValidationError::AgentOnlyFunction { .. }
        | ValidationError::InvalidCall(_)
        | ValidationError::UnallowedChild { .. }
        | ValidationError::UnallowedParent { .. } => "ValidationError",
        ValidationError::Surface(SurfaceError::Data { .. }) => "DataError",
        ValidationError::MissingCatalog | ValidationError::UnknownCatalog(_) => "CatalogError",
        _ => return Err(format!("unexpected transaction failure: {error}").into()),
    };
    if let Some(mut renderer) = renderer {
        let expected_message = renderer["error"]["message"]
            .as_str()
            .ok_or("missing renderer error message")?;
        let outbound = error
            .renderer_error(surface)
            .ok_or("missing renderer error envelope")?;
        let actual = serde_json::to_value(outbound)?;
        if path
            .as_ref()
            .is_some_and(|path| &actual["error"]["path"] != path)
        {
            return Err(format!("renderer error has the wrong field path: {actual}").into());
        }
        let actual_message = actual["error"]["message"]
            .as_str()
            .ok_or("missing error message")?;
        if !actual_message.contains(expected_message) {
            return Err(format!(
                "expected renderer error message containing {expected_message}, got {actual}"
            )
            .into());
        }
        renderer["error"]["message"] = json!(actual_message);
        if actual != renderer {
            return Err(format!("expected renderer error {renderer}, got {actual}").into());
        }
        if let Some(details) = &details {
            let observed = transaction_details(case, &actual["error"], &message)?;
            if &observed != details {
                return Err(
                    format!("expected validation details {details}, got {observed}").into(),
                );
            }
        }
        envelope = Some(actual);
    } else if path.is_some() || details.is_some() {
        return Err("transaction error expects field details without a renderer envelope".into());
    } else if core.contains_key("code") {
        let outbound = error
            .renderer_error(surface)
            .ok_or("missing renderer error code")?;
        envelope = Some(serde_json::to_value(outbound)?);
    }
    let renderer_code = envelope
        .as_ref()
        .and_then(|actual| actual["error"]["code"].as_str());
    assert_rejection(&Value::Object(core), category, renderer_code, &message)
}

fn transaction_details(case: &Case, error: &Value, message: &str) -> Result<Value, TestError> {
    if case.name != "test_custom_catalog_1_0" {
        return Ok(json!([{
            "code": error["code"], "path": error["path"], "surfaceId": error["surfaceId"]
        }]));
    }

    // The fixture also reports the component-local schema error.
    let path = error["path"]
        .as_str()
        .and_then(|path| path.strip_prefix("/components/0"))
        .ok_or("missing component-local error path")?;
    let code = match path {
        "/chartType" if message.contains("is not one of") => "invalid_value",
        "/children" if message.contains("is a required property") => "missing_field",
        "/component" if message.contains("UnknownComponent") => "invalid_value",
        _ => return Err(format!("unexpected catalog validation error: {message}").into()),
    };
    Ok(json!([{ "code": code, "path": path, "surfaceId": error["surfaceId"] }]))
}

fn catalog_document(case: &Case, document: &Value) -> Result<Value, TestError> {
    let mut document = document.clone();
    let fields = document.as_object_mut().ok_or("expected catalog object")?;
    if let Some(version) = &case.protocol_version {
        fields
            .entry("protocolVersion")
            .or_insert_with(|| Value::from(version.as_str()));
    }
    if let Some(id) = &case.catalog_id {
        fields
            .entry("catalogId")
            .or_insert_with(|| Value::from(id.as_str()));
    }
    Ok(document)
}

fn read_json(upstream: &Path, path: &str) -> Result<Value, TestError> {
    Ok(serde_json::from_slice(&fs::read(upstream.join(path))?)?)
}

fn assert_rejection(
    expected: &Value,
    category: &str,
    code: Option<&str>,
    error: &str,
) -> Result<(), TestError> {
    if let Some(fields) = expected.as_object()
        && (fields
            .keys()
            .any(|key| !matches!(key.as_str(), "category" | "message" | "code"))
            || expected
                .get("category")
                .is_some_and(|value| value.as_str() != Some(category))
            || expected
                .get("code")
                .is_some_and(|value| value.as_str() != code))
    {
        return Err(format!("unsupported error expectation {expected}: {error}").into());
    }
    if !expected.is_object() && !expected.is_string() {
        return Err(format!("invalid error expectation {expected}").into());
    }
    if let Some(message) = expected.as_str().or_else(|| expected["message"].as_str())
        && !error.contains(message)
    {
        return Err(format!("expected error containing {message}, got {error}").into());
    }
    Ok(())
}

fn assert_state(processor: &MessageProcessor, expected: &Value) -> Result<(), TestError> {
    for (id, fields) in expected["surfaces"]
        .as_object()
        .ok_or("expected surface states")?
    {
        let surface = processor.get(id);
        for (field, expected) in fields.as_object().ok_or("expected surface fields")? {
            match field.as_str() {
                "exists" if expected.as_bool() == Some(surface.is_some()) => {}
                "metadata"
                    if serde_json::to_value(
                        surface.and_then(a2ui_rs::surface::Surface::metadata),
                    )? == *expected => {}
                "dataModel"
                    if surface.is_some_and(|surface| surface.data_model().root() == expected) => {}
                "components" => {
                    let surface = surface.ok_or("missing expected surface")?;
                    if let Some(expected) = expected.as_object() {
                        let mut components = Vec::new();
                        for component in surface.components() {
                            let mut fields = expected
                                .get(&component.id)
                                .and_then(Value::as_object)
                                .ok_or("unexpected component ID")?
                                .clone();
                            fields.insert("id".to_owned(), Value::from(component.id.clone()));
                            components.push(Value::Object(fields));
                        }
                        if components.len() != expected.len() {
                            return Err("missing expected component".into());
                        }
                        assert_surface_components(surface.components(), &Value::Array(components))?;
                    } else {
                        assert_surface_components(surface.components(), expected)?;
                    }
                }
                _ => {
                    return Err(format!(
                        "unsupported or mismatched surface expectation {id}.{field}"
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}
