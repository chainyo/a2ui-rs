use std::fs;
use std::path::Path;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{AgentToRenderer, AgentToRendererMessage};
use a2ui_rs::surface::SurfaceError;
use a2ui_rs::validation::{MessageProcessor, ValidationConfig, ValidationError};
use serde_json::{Value, json};

use super::{Case, TestError, assert_surface_components};

fn supports_transaction(suite: &str, case: &Case) -> bool {
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
        )
}

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    if supports_transaction(suite, case) {
        return true;
    }
    if suite == "core/composition_constraints.yaml" && case.action == "process_messages" {
        return matches!(
            case.name.as_str(),
            "test_composition_dropped_child_is_not_checked_against_surface"
                | "test_composition_dropped_child_is_not_checked_against_surface_when_references_may_dangle"
                | "test_composition_dropped_child_is_reported_as_orphan"
                | "test_composition_root_referenced_by_another_component_checked_against_surface"
                | "test_composition_multiple_parents_disallowed_first"
                | "test_composition_multiple_parents_disallowed_second"
        );
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
    if transaction && case.catalog_paths.is_empty() {
        let mut basic = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
        for id in ["basic", "test-catalog-v10"] {
            basic["catalogId"] = json!(id);
            processor.add_catalog(Catalog::from_json(basic.clone(), &common)?)?;
        }
    }
    if let Some(document) = &case.catalog {
        let mut document = catalog_document(case, document)?;
        if case.name.starts_with("test_composition_") {
            // These fixtures use a local alias for the standard child list.
            document["$defs"] =
                json!({"ChildList": {"$ref": "common_types.json#/$defs/ChildList"}});
        }
        processor.add_catalog(Catalog::from_json(document, &common)?)?;
    }

    if let Some(messages) = &case.messages {
        if !case.steps.is_empty() {
            return Err("validation fixture has both messages and steps".into());
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
    if case.steps.is_empty() {
        return Err("missing validation steps".into());
    }
    for step in &case.steps {
        let messages = step
            .get("messages")
            .or_else(|| step.get("payload"))
            .ok_or("missing validation messages")?;
        let rules = step
            .get("validationConfig")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?;
        apply_messages(
            &mut processor,
            case,
            messages,
            rules,
            step.get("expectError"),
            transaction,
        )?;
        if let Some(expect) = step.get("expect") {
            assert_state(&processor, expect)?;
        }
    }
    if let Some(expect) = &case.expect {
        assert_state(&processor, expect)?;
    }
    Ok(())
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
                return assert_transaction_error(expected, &surface, &error);
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
    expected: &Value,
    surface: &str,
    error: &ValidationError,
) -> Result<(), TestError> {
    let mut core = expected
        .as_object()
        .ok_or("missing transaction error fields")?
        .clone();
    let renderer = core.remove("rendererError");
    if core.remove("surfaceId").is_some_and(|id| id != surface) {
        return Err("transaction failed on the wrong surface".into());
    }
    let message = error.to_string();
    let category = match error {
        ValidationError::Surface(SurfaceError::DuplicateComponent(_))
        | ValidationError::CircularReference(_)
        | ValidationError::Orphan(_) => "IntegrityError",
        ValidationError::DanglingReference { .. } => "ValidationError",
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
    }
    assert_rejection(&Value::Object(core), category, None, &message)
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
