use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::data_model::DataError;
use a2ui_rs::protocol::{AgentToRenderer, AgentToRendererMessage};
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::RoutingSession;
use a2ui_rs::rpc::RpcHandler;
use a2ui_rs::surface::SurfaceError;
use a2ui_rs::validation::{MessageProcessor, ValidationConfig, ValidationError};
use serde_json::{Value, json};

use super::{Case, TestError, assert_surface_components};

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
        && case.action == "process_messages"
        && case.protocol_version.as_deref() == Some("1.0")
        && matches!(
            case.name.as_str(),
            "test_batch_duplicate_component_ids_in_same_message_error"
                | "test_batch_atomic_rollback_on_candidate_topology_cycle"
                | "test_permissive_mode_allows_orphan_components"
                | "test_permissive_mode_allows_dangling_references"
                | "test_v10_call_validation_config_relaxes_strict_processor"
                | "test_v10_call_validation_config_tightens_permissive_processor"
                | "test_v10_call_validation_config_leaves_out_rules_as_strict"
                | "test_composition_constraints_preserved_on_partial_parent_update"
                | "test_v10_permissive_incremental_streaming_out_of_order_before_root"
                | "test_v10_component_without_catalog_id_on_surface_without_default_errors"
                | "test_v10_component_without_catalog_id_errors_with_a_single_catalog"
                | "test_v10_function_call_naming_unknown_catalog_errors"
                | "test_v10_call_naming_unknown_catalog_errors_without_strict_mode"
                | "test_v10_function_call_with_empty_catalog_id_errors"
        )
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
    if suite == "core/catalog.yaml" && case.action == "from_json" {
        return matches!(
            case.name.as_str(),
            "test_v10_catalog_from_json_uax31_validation"
                | "test_v10_catalog_from_json_v1_0_attributes"
                | "test_v10_catalog_from_json_user_activation_agent_only_error"
                | "test_v10_catalog_from_json_user_activation_renderer_or_agent_error"
                | "test_v10_catalog_from_json_user_activation_renderer_only"
                | "test_v10_catalog_from_json_user_activation_error_in_function_left_out_of_union"
                | "test_v10_catalog_from_json_rejects_at_prefixed_function_name"
                | "test_v10_catalog_from_json_invalid_uax31_identifier_error"
                | "test_v10_catalog_from_json_rejects_ref_to_another_catalog"
                | "test_v10_catalog_from_json_rejects_absolute_uri_ref"
                | "test_v10_catalog_from_json_rejects_external_ref_in_function_args"
                | "test_v10_catalog_from_json_accepts_local_and_common_types_refs"
                | "test_v10_uax31_invalid_component_name"
                | "test_v10_uax31_invalid_property_name"
                | "test_v10_uax31_invalid_function_name"
                | "test_v10_uax31_invalid_argument_name"
                | "test_v10_uax31_valid_non_ascii_identifier"
                | "test_v10_uax31_invalid_en_dash_component_name"
                | "test_v10_uax31_invalid_em_dash_property_name"
                | "test_v10_uax31_invalid_armenian_hyphen_function_name"
                | "test_v10_catalog_from_json_component_metadata"
        );
    }
    suite == "core/validator_v1_0.yaml"
        && case.action == "validate"
        && case.protocol_version.as_deref() == Some("1.0")
        && matches!(
            case.name.as_str(),
            "test_v10_topology_circular_reference_error"
                | "test_v10_topology_dangling_child_reference_error"
                | "test_v10_topology_self_reference_error"
                | "test_v10_topology_orphaned_component_error"
                | "test_v10_topology_missing_root_component_error"
                | "test_v10_topology_template_child_reachable"
                | "test_v10_topology_template_dangling_reference_error"
                | "test_v10_component_nesting_depth_limit_exceeded_error"
                | "test_v10_component_nesting_depth_within_limit"
                | "test_v10_multi_surface_independent_roots"
                | "test_v10_multi_surface_missing_root_error"
                | "test_v10_incremental_update_without_root"
                | "test_v10_incremental_update_self_reference_error"
                | "test_v10_incremental_update_circular_reference_error"
                | "test_v10_incremental_update_duplicate_component_id_error"
                | "test_v10_topology_structured_array_item_dangling_child_error"
                | "test_v10_topology_plain_string_property_not_treated_as_child_ref"
                | "test_v10_topology_component_id_array_dangling_reference_error"
                | "test_v10_topology_component_id_array_child_reachable"
                | "test_v10_topology_component_id_array_partial_dangling_reference_error"
                | "test_v10_topology_component_id_array_circular_reference_error"
                | "test_v10_child_list_template_missing_path_error"
                | "test_v10_child_list_template_rejects_at_path_error"
                | "test_v10_incremental_update_same_component_id_across_messages"
                | "test_v10_component_nesting_depth_at_limit"
                | "test_v10_component_nesting_depth_one_past_limit_error"
                | "test_v10_recursion_limit_function_call_depth_at_limit"
                | "test_v10_recursion_limit_function_call_depth_one_past_limit_error"
                | "test_v10_validate_at_prefixed_component_envelope_mixins"
        )
}

pub(super) fn execute(upstream: &Path, suite: &str, case: &Case) -> Result<(), TestError> {
    let common = read_json(upstream, "specification/v1_0/json/common_types.json")?;
    if case.action == "from_json" {
        return execute_catalog(upstream, case, &common);
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
    for path in &case.catalog_paths {
        let document = read_json(upstream, path)?;
        processor.add_catalog(Catalog::from_json(document.clone(), &common)?)?;
        if path == "catalogs/basic/v1/catalog.json" {
            // Some pinned fixtures use the published catalog's former URL.
            let mut alias = document;
            alias["catalogId"] = Value::from("https://a2ui.org/catalogs/basic/v1/catalog.json");
            processor.add_catalog(Catalog::from_json(alias, &common)?)?;
        }
    }
    let transaction = supports_transaction(suite, case);
    if transaction && case.catalog_paths.is_empty() && case.catalog.is_none() {
        let mut basic = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
        for id in ["basic", "test-catalog-v10"] {
            basic["catalogId"] = json!(id);
            processor.add_catalog(Catalog::from_json(basic.clone(), &common)?)?;
        }
    }
    if transaction && let Some(catalogs) = &case.catalogs {
        for document in catalogs.as_array().ok_or("expected transaction catalogs")? {
            processor.add_catalog(Catalog::from_json(
                catalog_document(case, document)?,
                &common,
            )?)?;
        }
    }
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
            transaction,
        )
        .and_then(|()| {
            case.expect
                .as_ref()
                .map_or(Ok(()), |expect| assert_state(&processor, expect))
        });
    }
    execute_validation_steps(&mut processor, case, transaction)
}

fn execute_validation_steps(
    processor: &mut MessageProcessor,
    case: &Case,
    transaction: bool,
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
        apply_messages(
            processor,
            case,
            messages,
            rules,
            expected_error,
            transaction,
        )?;
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
        true,
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
            apply_messages(processor, case, &messages, None, expected, true)?;
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
    transaction: bool,
) -> Result<(), TestError> {
    let messages = messages
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or("missing validation messages")?;
    for (index, message) in messages.iter().enumerate() {
        let decoded: AgentToRendererMessage = serde_json::from_value(message.clone())?;
        let surface = if transaction {
            Some(transaction_surface(&decoded)?.to_owned())
        } else {
            None
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
                return assert_transaction_error(case, expected, &surface, &error);
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
        ValidationError::ComponentDepth | ValidationError::FunctionDepth
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
        | ValidationError::UnknownComponent { .. }
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
            let error = &actual["error"];
            let observed = json!([{
                "code": error["code"], "path": error["path"], "surfaceId": error["surfaceId"]
            }]);
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

fn execute_catalog(upstream: &Path, case: &Case, common: &Value) -> Result<(), TestError> {
    let document = match (&case.catalog, &case.catalog_path) {
        (Some(document), None) => catalog_document(case, document)?,
        (None, Some(path)) => read_json(upstream, path)?,
        _ => return Err("expected one catalog input".into()),
    };
    match (Catalog::from_json(document, common), &case.expect_error) {
        (Ok(catalog), None) => {
            if let Some(expected) = &case.expect {
                assert_catalog(catalog.as_json(), expected)?;
            }
            Ok(())
        }
        (Err(error), Some(expected)) => {
            assert_rejection(expected, "CatalogError", None, &error.to_string())
        }
        (Ok(_), Some(_)) => Err("expected catalog error".into()),
        (Err(error), None) => Err(error.into()),
    }
}

fn assert_catalog(document: &Value, expected: &Value) -> Result<(), TestError> {
    for (key, value) in expected.as_object().ok_or("expected catalog fields")? {
        let actual = match key.as_str() {
            "componentMetadata" => {
                let fields = value
                    .as_object()
                    .ok_or("expected component metadata fields")?;
                Value::Object(
                    fields
                        .keys()
                        .map(|name| {
                            (
                                name.clone(),
                                document["components"][name]["metadata"].clone(),
                            )
                        })
                        .collect(),
                )
            }
            "components" | "functions" if value.is_array() => {
                let actual = document[key].as_object().ok_or("missing catalog entries")?;
                let names = actual
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>();
                let expected_names = value
                    .as_array()
                    .ok_or("expected names")?
                    .iter()
                    .map(|name| name.as_str().ok_or("expected catalog entry name"))
                    .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
                if names != expected_names {
                    return Err(format!("expected {key} {expected_names:?}, got {names:?}").into());
                }
                continue;
            }
            _ => document[key].clone(),
        };
        if actual != *value {
            return Err(format!("expected {key} {value}, got {actual}").into());
        }
    }
    Ok(())
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
