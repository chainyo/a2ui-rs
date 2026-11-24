use std::cell::Cell;
use std::fs;
use std::path::Path;

use a2ui_rs::basic_catalog::{
    FormattingOptions, register_formatting_functions, register_validation_functions,
};
use a2ui_rs::catalog::{Catalog, CatalogError};
use a2ui_rs::expression::{Expression, parse_template};
use a2ui_rs::resolution::{DataScope, LocalFunctions, ResolveError, ResolvedComponent};
use a2ui_rs::validation::{MessageProcessor, ValidationError};
use serde_json::{Value, json};

use super::{Case, TestError};

#[path = "multi_catalog.rs"]
mod multi_catalog;

const MULTI_CATALOG_SUITE: &str = "core/multi_catalog.yaml";

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    if suite == MULTI_CATALOG_SUITE {
        return match case.action.as_str() {
            "select_catalog" => case
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog["protocolVersion"] == "1.0"),
            "validate" => matches!(
                case.name.as_str(),
                "test_multi_catalog_function_call_catalog_id_override"
                    | "test_multi_catalog_format_string_index_and_catalog_call_without_default"
            ),
            _ => false,
        };
    }
    if suite == "core/expressions.yaml" && case.action == "parse_expression_template" {
        return true;
    }
    if suite == "core/functions.yaml" && case.action == "evaluate_function" {
        if case.protocol_version.as_deref() == Some("1.0")
            && (case.name.starts_with("test_evaluate_function_regex_v10_")
                || is_formatting_case(&case.name)
                || is_resolved_boolean_case(&case.name)
                || matches!(
                    case.name.as_str(),
                    "test_evaluate_function_v10_catalog_of_other_protocol_version_errors"
                        | "test_evaluate_function_v10_agent_only_function_errors"
                ))
        {
            return true;
        }
        return matches!(
            case.name.as_str(),
            "test_evaluate_function_format_string_basic"
                | "test_evaluate_function_format_string_null_coercion"
                | "test_evaluate_function_v10_runs_in_named_catalog"
                | "test_evaluate_function_v10_runs_in_named_catalog_on_surface_without_default"
                | "test_evaluate_function_v10_runs_in_surface_default"
                | "test_evaluate_function_v10_without_catalog_id_on_surface_without_default_errors"
                | "test_evaluate_function_v10_unknown_catalog_id_errors"
                | "test_evaluate_function_v10_nested_call_does_not_inherit_catalog"
                | "test_evaluate_function_v10_format_string_template_call_missing_in_inherited_catalog_errors"
                | "test_evaluate_function_v10_format_string_index_stays_catalogless_without_default"
                | "test_evaluate_function_v10_validation_result_coerced_in_not_operand_on_surface"
                | "test_evaluate_function_v10_validation_result_coerced_in_and_operands_on_surface"
        );
    }
    let version = case
        .catalog
        .as_ref()
        .and_then(|catalog| catalog["protocolVersion"].as_str());
    version == Some("1.0")
        && case.action == "validate"
        && (suite == "core/index_function.yaml"
            || (suite == "core/functions.yaml"
                && (matches!(
                    case.name.as_str(),
                    "test_function_invalid_argument_schema_error"
                        | "test_function_logical_and_or_not"
                ) || is_formatting_case(&case.name)))
            || (suite == "core/expressions.yaml"
                && matches!(
                    case.name.as_str(),
                    "test_expression_parser_string_interpolation_multiple_bindings"
                        | "test_expression_parser_escaped_interpolation_sequence"
                        | "test_expression_parser_data_type_coercion_matrix"
                        | "test_expression_parser_syntax_error_unclosed_brace"
                        | "test_expression_parser_max_depth_exceeded_error"
                        | "test_expression_parser_nested_function_call"
                )))
}

fn is_formatting_case(name: &str) -> bool {
    matches!(
        name,
        "test_function_format_date_tr35_tokens"
            | "test_expression_parser_nested_function_call"
            | "test_function_format_currency_locale_and_symbol"
            | "test_function_pluralize_categories"
            | "test_evaluate_function_format_date_keeps_written_offset"
            | "test_evaluate_function_v10_format_string_template_call_inherits_catalog_without_default"
            | "test_evaluate_function_v10_format_string_template_call_inherits_named_catalog_over_default"
            | "test_evaluate_function_v10_format_string_template_call_inherits_catalog_nested"
            | "test_evaluate_function_v10_validation_result_coerced_in_format_number_grouping"
            | "test_evaluate_function_v10_validation_result_coerced_in_format_number_grouping_valid"
    )
}

pub(super) fn execute(upstream: &Path, suite: &str, case: &Case) -> Result<(), TestError> {
    if case.action == "parse_expression_template" {
        return execute_parser(case);
    }
    let common = read_json(upstream, "specification/v1_0/json/common_types.json")?;
    if suite == MULTI_CATALOG_SUITE {
        return multi_catalog::execute(upstream, case, &common);
    }
    if case.action == "evaluate_function" {
        return execute_function(upstream, case, &common);
    }
    let mut catalog = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
    // These fixtures register the published catalog as "basic".
    catalog["catalogId"] = Value::from("basic");
    let mut processor = MessageProcessor::default();
    processor.add_catalog(Catalog::from_json(catalog, &common)?)?;
    let mut functions = LocalFunctions::default();
    if case.name == "test_function_logical_and_or_not" {
        register_validation_functions(&mut functions, "basic");
    }
    if is_formatting_case(&case.name) {
        register_formatting_functions(
            &mut functions,
            "basic",
            FormattingOptions::new("en-US", "UTC")?,
        );
    }

    for step in &case.steps {
        let messages = step["payload"]
            .as_array()
            .ok_or("missing resolution messages")?;
        let mut rejected = false;
        for (index, message) in messages.iter().enumerate() {
            let decoded = serde_json::from_value(message.clone())?;
            let result = processor.apply(decoded);
            if let Err(error) = result {
                let expected = step.get("expectError").ok_or_else(|| error.to_string())?;
                if index + 1 != messages.len()
                    || !matches!(
                        error,
                        ValidationError::Expression(_)
                            | ValidationError::Catalog(CatalogError::InvalidPayload { .. })
                    )
                {
                    return Err(error.into());
                }
                assert_error(expected, "ValidationError", &error.to_string())?;
                rejected = true;
            }
        }
        if let Some(expected) = step.get("expectError")
            && !rejected
        {
            let surface = messages
                .last()
                .and_then(|message| message["createSurface"]["surfaceId"].as_str())
                .ok_or("missing surface ID")?;
            match processor.resolve_components(surface, &functions) {
                Err(error @ ResolveError::IndexScope) => {
                    assert_error(expected, "ValidationError", &error.to_string())?;
                }
                Err(error) => return Err(error.into()),
                Ok(_) => return Err("expected resolution to fail".into()),
            }
        }
    }
    if let Some(expected) = &case.expect {
        if case.name == "test_function_logical_and_or_not" {
            assert_logical_text(&processor, &functions, expected)?;
        } else {
            assert_components(&processor, &functions, expected)?;
        }
        if suite == "core/index_function.yaml" {
            super::nodes::assert_mounted_indices(processor, expected)?;
        }
    }
    Ok(())
}

fn execute_function(upstream: &Path, case: &Case, common: &Value) -> Result<(), TestError> {
    let mut processor = MessageProcessor::default();
    let mut create = json!({"surfaceId": "main"});
    if case.catalog_paths.is_empty() {
        let catalog = Catalog::from_json(
            read_json(upstream, "catalogs/basic/v1/catalog.json")?,
            common,
        )?;
        create["catalogId"] = Value::from(catalog.id());
        processor.add_catalog(catalog)?;
    }
    for path in &case.catalog_paths {
        let catalog = Catalog::from_json(read_json(upstream, path)?, common);
        if case.name == "test_evaluate_function_v10_catalog_of_other_protocol_version_errors"
            && path == "catalogs/basic/v0_9/catalog.json"
        {
            if !matches!(
                catalog,
                Err(CatalogError::InvalidCatalog(ref message))
                    if message.contains("must declare protocolVersion 1.0")
            ) {
                return Err("expected catalog loading to reject the v0.9 catalog".into());
            }
            continue;
        }
        processor.add_catalog(catalog?)?;
    }
    if let Some(default_catalog) = case
        .surface
        .as_ref()
        .and_then(|surface| surface.get("catalogId"))
    {
        create["catalogId"] = default_catalog.clone();
    }
    if let Some(data) = &case.data_model {
        create["dataModel"] = data.clone();
    }
    let mut args = case.args.clone().ok_or("missing function arguments")?;
    if is_resolved_boolean_case(&case.name) {
        // Standalone fixtures supply resolved operands. Bind them to form valid calls.
        let data = args.clone();
        if let Some(values) = args.get_mut("values").and_then(Value::as_array_mut) {
            for (index, operand) in values.iter_mut().enumerate() {
                *operand = json!({"@path": format!("/values/{index}")});
            }
        } else {
            args["value"] = json!({"@path": "/value"});
        }
        create["dataModel"] = data;
    }
    let message = json!({"version": "1.0", "createSurface": create});
    processor.apply(serde_json::from_value(message)?)?;
    let mut call = json!({"@call": case.function, "args": args});
    if let Some(id) = &case.catalog_id {
        call["catalogId"] = Value::from(id.clone());
    }
    let called = Cell::new(false);
    let mut functions = LocalFunctions::default();
    let basic = "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";
    register_validation_functions(&mut functions, basic);
    if is_formatting_case(&case.name) {
        register_formatting_functions(
            &mut functions,
            basic,
            FormattingOptions::new("en-US", "UTC")?,
        );
    }
    if case.name == "test_evaluate_function_v10_agent_only_function_errors" {
        let id = create["catalogId"].as_str().ok_or("missing catalog ID")?;
        let name = call["@call"].as_str().ok_or("missing function name")?;
        functions.insert(id, name, |_| {
            called.set(true);
            Ok(Value::Null)
        });
    }
    let actual = processor.resolve_value("main", &DataScope::default(), &call, &functions);
    if called.get() {
        return Err("the renderer invoked an agent-only function".into());
    }
    match (actual, &case.expect, &case.expect_error) {
        (Ok(actual), Some(expected), None) if actual == *expected => Ok(()),
        (Err(error), None, Some(expected)) => {
            let category = match error {
                ResolveError::UnknownCatalog(_) | ResolveError::MissingCatalog => "CatalogError",
                ResolveError::Catalog(CatalogError::UnknownEntry(_))
                | ResolveError::AgentOnly(_)
                | ResolveError::IndexScope
                | ResolveError::LocalFunction(_) => "ExpressionError",
                _ => return Err(error.into()),
            };
            assert_error(expected, category, &error.to_string())
        }
        (actual, _, _) => Err(format!("unexpected function result {actual:?}").into()),
    }
}

fn is_resolved_boolean_case(name: &str) -> bool {
    matches!(
        name,
        "test_evaluate_function_and_validation_result_operands"
            | "test_evaluate_function_or_validation_result_operands"
            | "test_evaluate_function_not_validation_result_operand"
    )
}

fn assert_logical_text(
    processor: &MessageProcessor,
    functions: &LocalFunctions<'_>,
    expected: &Value,
) -> Result<(), TestError> {
    let mut rendered = expected.clone();
    let fields = rendered["surfaces"]["main"]["components"]
        .as_object_mut()
        .ok_or("missing logical component expectations")?;
    let surface = processor.get("main").ok_or("missing logical surface")?;
    for (id, expected) in fields {
        let component = surface
            .components()
            .iter()
            .find(|component| component.id == *id)
            .ok_or("missing logical component")?;
        let call = component
            .properties
            .get("text")
            .ok_or("missing logical expression")?;
        let boolean = &expected["text"];
        let actual = processor.resolve_value("main", &DataScope::default(), call, functions)?;
        if &actual != boolean || !boolean.is_boolean() {
            return Err(format!("expected {id} to return {boolean}, got {actual}").into());
        }
        // A Text property converts the evaluated boolean to its string form.
        expected["text"] = Value::String(boolean.to_string());
    }
    assert_components(processor, functions, &rendered)
}

fn execute_parser(case: &Case) -> Result<(), TestError> {
    let input = case
        .input
        .as_ref()
        .and_then(Value::as_str)
        .ok_or("missing template input")?;
    match (parse_template(input), &case.parse_error) {
        (Ok(parts), None) => {
            let actual = Value::Array(
                parts
                    .into_iter()
                    .map(|part| match part {
                        Expression::Literal(value) => value,
                        Expression::Binding(path) => json!({"path": path}),
                        Expression::Call(call) => {
                            let object = json!({"@call": call.name, "args": call.args});
                            fixture_expression(object)
                        }
                    })
                    .collect(),
            );
            let expected = case.expect.as_ref().ok_or("missing parser expectation")?;
            if !same_json(&actual, expected) {
                return Err(format!("expected {expected}, got {actual}").into());
            }
            Ok(())
        }
        (Err(error), Some(expected)) => assert_error(expected, "ParseError", &error.to_string()),
        (Err(error), None) => Err(error.into()),
        (Ok(_), Some(expected)) => Err(format!("expected parse error {expected}").into()),
    }
}

fn fixture_expression(value: Value) -> Value {
    if let Some(path) = value.get("@path") {
        return json!({"path": path});
    }
    if let Some(call) = value.get("@call") {
        let args = value["args"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), fixture_expression(value.clone())))
            .collect::<serde_json::Map<_, _>>();
        return json!({"call": call, "args": args, "returnType": "any"});
    }
    value
}

fn same_json(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(actual), Value::Number(expected)) => actual.as_f64() == expected.as_f64(),
        (Value::Array(actual), Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| same_json(actual, expected))
        }
        (Value::Object(actual), Value::Object(expected)) => {
            actual.len() == expected.len()
                && actual.iter().all(|(key, value)| {
                    expected
                        .get(key)
                        .is_some_and(|expected| same_json(value, expected))
                })
        }
        (actual, expected) => actual == expected,
    }
}

fn assert_components(
    processor: &MessageProcessor,
    functions: &LocalFunctions<'_>,
    expected: &Value,
) -> Result<(), TestError> {
    let surfaces = expected["surfaces"]
        .as_object()
        .ok_or("missing resolution surfaces")?;
    for (id, surface) in surfaces {
        let root = processor
            .resolve_components(id, functions)?
            .ok_or("missing resolved root")?;
        let mut nodes = Vec::new();
        collect_nodes(&root, &mut nodes);
        for (id, properties) in surface["components"]
            .as_object()
            .ok_or("missing component expectations")?
        {
            let node = nodes
                .iter()
                .find(|node| {
                    node.scope.index().map_or_else(
                        || node.id == *id,
                        |index| format!("{}_{index}", node.id) == *id,
                    )
                })
                .ok_or_else(|| format!("missing resolved component {id}"))?;
            for (key, expected) in properties
                .as_object()
                .ok_or("missing property expectations")?
            {
                let actual = if key == "component" {
                    Value::from(node.component.clone())
                } else {
                    node.properties
                        .get(key)
                        .cloned()
                        .ok_or_else(|| format!("missing property {id}.{key}"))?
                };
                if actual != *expected {
                    return Err(format!("expected {id}.{key} = {expected}, got {actual}").into());
                }
            }
        }
    }
    Ok(())
}

fn collect_nodes<'a>(root: &'a ResolvedComponent, nodes: &mut Vec<&'a ResolvedComponent>) {
    nodes.push(root);
    for child in &root.children {
        collect_nodes(child, nodes);
    }
}

fn read_json(upstream: &Path, path: &str) -> Result<Value, TestError> {
    Ok(serde_json::from_slice(&fs::read(upstream.join(path))?)?)
}

fn assert_error(expected: &Value, category: &str, error: &str) -> Result<(), TestError> {
    let fields = expected.as_object().ok_or("expected error fields")?;
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "category" | "message"))
        || expected["category"] != category
        || expected.get("message").is_some_and(|message| {
            !message
                .as_str()
                .is_some_and(|message| error.contains(message))
        })
    {
        return Err(format!("expected error {expected}, got {error}").into());
    }
    Ok(())
}
