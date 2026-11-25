use std::fs;
use std::path::Path;

use a2ui_rs::protocol::AgentToRendererMessage;
use a2ui_rs::schema::{agent_to_renderer_schema, common_types_schema};
use jsonschema::{Draft, Registry};
use serde_json::{Value, json};

use super::{Case, TestError};

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    case.protocol_version.as_deref() == Some("1.0")
        && matches!(
            (suite, case.action.as_str()),
            ("core/common_types.yaml", "common_types_schema")
                | ("core/agent_to_renderer.yaml", "agent_to_renderer_schema")
        )
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let actual = match case.action.as_str() {
        "common_types_schema" => common_types_schema(),
        "agent_to_renderer_schema" => agent_to_renderer_schema(),
        _ => return Err("unsupported schema case".into()),
    };
    let file = case
        .expect_file
        .as_ref()
        .ok_or("missing schema reference")?;
    let expected: Value = serde_json::from_slice(&fs::read(upstream.join(file))?)?;
    if actual == expected {
        return if case.action == "agent_to_renderer_schema" {
            validate_sdk_envelopes(upstream, &actual)
        } else {
            Ok(())
        };
    }
    let expected_defs = expected["$defs"]
        .as_object()
        .ok_or("missing expected definitions")?;
    let actual_defs = actual["$defs"]
        .as_object()
        .ok_or("missing generated definitions")?;
    if let Some(name) = expected_defs
        .keys()
        .chain(actual_defs.keys())
        .find(|name| expected_defs.get(*name) != actual_defs.get(*name))
    {
        return Err(format!(
            "generated definition {name} differs from {file}. Expected {}, got {}",
            expected["$defs"][name], actual["$defs"][name]
        )
        .into());
    }
    Err(format!("generated schema envelope differs from {file}").into())
}

fn validate_sdk_envelopes(upstream: &Path, schema: &Value) -> Result<(), TestError> {
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    catalog
        .as_object_mut()
        .ok_or("expected catalog object")?
        .remove("$id");
    let base = "https://a2ui.org/specification/v1_0/";
    let registry = Registry::new()
        .draft(Draft::Draft202012)
        .extend([
            (format!("{base}catalog.json"), catalog),
            (format!("{base}common_types.json"), common_types_schema()),
        ])?
        .prepare()?;
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(schema)?;
    let valid = [
        json!({"version":"1.0","createSurface":{"surfaceId":"form","metadata":{"extensions":{"audit":1}}}}),
        json!({"version":"1.0","updateComponents":{"surfaceId":"form","components":[{"id":"root","component":"Text","text":{"@path":"/label"}}]}}),
        json!({"version":"1.0","updateDataModel":{"surfaceId":"form","value":null}}),
        json!({"version":"1.0","deleteSurface":{"surfaceId":"form"}}),
        json!({"version":"1.0","callRendererFunction":{"functionCallId":"call","callFunction":{"@call":"formatString","catalogId":"basic","args":{"value":"Hello"}}}}),
        json!({"version":"1.0","agentFunctionResponse":{"functionCallId":"call","error":{"code":"E","message":"Failed"}}}),
    ];
    for input in valid {
        let message: AgentToRendererMessage = serde_json::from_value(input)?;
        let output = serde_json::to_value(message)?;
        validator
            .validate(&output)
            .map_err(|error| format!("SDK message {output} failed validation: {error}"))?;
    }
    let invalid = [
        json!({"version":"1.0","createSurface":{"surfaceId":"form","catalogId":null}}),
        json!({"version":"1.0","updateDataModel":{"surfaceId":"form"}}),
        json!({"version":"1.0","callRendererFunction":{"functionCallId":"call","callFunction":{"@call":"formatString","args":{"value":"Hello"}}}}),
        json!({"version":"1.0","agentFunctionResponse":{"functionCallId":"call","value":null,"error":{"code":"E","message":"Failed"}}}),
    ];
    for input in invalid {
        if validator.is_valid(&input)
            || serde_json::from_value::<AgentToRendererMessage>(input.clone()).is_ok()
        {
            return Err(format!("accepted invalid SDK envelope {input}").into());
        }
    }
    Ok(())
}
