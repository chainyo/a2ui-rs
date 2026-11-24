use std::fs;
use std::path::Path;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{Component, Extensions, FunctionCall, FunctionResponse};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{Case, TestError};

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    suite == "core/common_types.yaml"
        && case.protocol_version.as_deref() == Some("1.0")
        && case.action == "validate_common_type"
        && matches!(
            case.definition.as_deref(),
            Some(
                "FunctionResponse"
                    | "Extensions"
                    | "ComponentCommon"
                    | "FunctionCall"
                    | "IndexSystemFunction"
                    | "Surface"
                    | "DynamicValue"
                    | "ChildList"
            )
        )
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    if case.steps.is_empty() {
        return Err("supported case has no steps".into());
    }
    match case.definition.as_deref() {
        Some("FunctionResponse") => assert_steps(case, round_trip::<FunctionResponse>),
        Some("Extensions") => assert_steps(case, round_trip::<Extensions>),
        Some("FunctionCall") => assert_steps(case, round_trip::<FunctionCall>),
        Some("ComponentCommon") => assert_steps(case, |value| {
            let mut input = value.clone();
            input["component"] = json!("Text");
            let mut output = round_trip::<Component>(&input)?;
            output
                .as_object_mut()
                .ok_or("expected a serialized component")?
                .remove("component");
            Ok(output)
        }),
        Some(definition @ ("IndexSystemFunction" | "Surface" | "DynamicValue" | "ChildList")) => {
            let catalog = fixture_catalog(upstream, definition)?;
            assert_steps(case, |value| {
                let component = serde_json::from_value::<Component>(json!({
                    "id": "root", "component": "Fixture", "value": value
                }))?;
                catalog.validate_component(&component)?;
                Ok(serde_json::to_value(component)?["value"].clone())
            })
        }
        _ => Err("unsupported common type reached execution".into()),
    }
}

fn fixture_catalog(upstream: &Path, definition: &str) -> Result<Catalog, TestError> {
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let schema = format!("common_types.json#/$defs/{definition}");
    Ok(Catalog::from_json(
        json!({
            "protocolVersion": "1.0",
            "catalogId": "common-type-fixture",
            "components": {"Fixture": {
                "type": "object",
                "properties": {
                    "component": {"const": "Fixture"},
                    "value": {"$ref": schema}
                },
                "required": ["component", "value"]
            }},
            "functions": {"now": {
                "type": "object",
                "properties": {"@call": {"const": "now"}},
                "returnType": "number"
            }}
        }),
        &common,
    )?)
}

fn assert_steps(
    case: &Case,
    validate: impl Fn(&Value) -> Result<Value, TestError>,
) -> Result<(), TestError> {
    for (index, step) in case.steps.iter().enumerate() {
        let value = step.get("value").ok_or("step has no value")?;
        let actual = validate(value);
        if let Some(expected) = step.get("expectError") {
            if expected["category"] != "ValidationError" {
                return Err(format!("unsupported error expectation {expected}").into());
            }
            if actual.is_ok() {
                return Err(format!("step {} accepted invalid input {value}", index + 1).into());
            }
        } else if actual.map_err(|error| format!("step {}: {error}", index + 1))? != *value {
            return Err(format!("step {} changed the JSON value {value}", index + 1).into());
        }
    }
    Ok(())
}

fn round_trip<T: DeserializeOwned + Serialize>(value: &Value) -> Result<Value, TestError> {
    let native = serde_json::from_value::<T>(value.clone())?;
    Ok(serde_json::to_value(native)?)
}
