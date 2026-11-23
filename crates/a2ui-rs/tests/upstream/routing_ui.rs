use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::RoutingSession;
use a2ui_rs::rpc::RpcHandler;
use a2ui_rs::validation::{MessageProcessor, ValidationConfig};
use serde_json::{Value, json};

use super::super::{Case, TestError};
use super::receive;

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    suite == "core/rpc_functions.yaml"
        && case.action == "handle_rpc"
        && matches!(
            case.name.as_str(),
            "test_rpc_ui_binding_triggers_call_agent_function_async_resolution"
                | "test_rpc_ui_action_missing_local_function_fallback_to_agent"
                | "test_rpc_ui_binding_agent_error_resolves_null"
        )
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let args = case.args.as_ref().ok_or("missing UI scenario arguments")?;
    let expected = case
        .expect
        .as_ref()
        .ok_or("missing UI scenario expectations")?;
    let action = args.get("uiActionFallbackScenario");
    let scenario = action
        .or_else(|| args.get("uiBindingScenario"))
        .ok_or("missing UI scenario")?;
    let surface = scenario["surfaceId"].as_str().ok_or("missing surface ID")?;
    let catalog_id = scenario["catalogId"].as_str().ok_or("missing catalog ID")?;
    let now = Instant::now();
    let mut session = create_session(upstream, scenario, surface, catalog_id, now)?;

    let scope = DataScope::default();
    let observation = if action.is_some() {
        session.run_action(
            surface,
            scope.clone(),
            scenario["actionPayload"]["functionCall"].clone(),
            true,
        )
    } else {
        let component = scenario["observe"]["componentId"]
            .as_str()
            .unwrap_or("root");
        let property = scenario["observe"]["property"].as_str().unwrap_or("text");
        session.mount_property(surface, scope.clone(), component, property)
    };
    let mut siblings = Vec::new();
    for (id, expectation) in expected["siblings"].as_object().into_iter().flatten() {
        for (name, value) in expectation
            .as_object()
            .ok_or("missing sibling properties")?
        {
            siblings.push((
                session.mount_property(surface, scope.clone(), id, name),
                value,
            ));
        }
    }

    let mut messages = Vec::new();
    let update = session.advance(
        now,
        Some(&mut |message| {
            messages.push(message);
            Ok(())
        }),
    );
    if !update.errors.is_empty() || messages.len() != 1 {
        return Err(format!("expected one outbound call and no errors, got {messages:?}").into());
    }
    if let Some(initial) = expected.get("initialValue")
        && session.value(observation).as_ref() != Some(initial)
    {
        return Err("unexpected initial binding value".into());
    }
    let mut message = serde_json::to_value(messages.pop().ok_or("missing outbound message")?)?;
    let id = message["callAgentFunction"]
        .as_object_mut()
        .and_then(|call| call.remove("functionCallId"))
        .and_then(|id| id.as_str().filter(|id| !id.is_empty()).map(str::to_owned))
        .ok_or("missing correlation ID")?;
    if message != expected["outboundCall"] {
        return Err(format!("expected {}, got {message}", expected["outboundCall"]).into());
    }
    let response = if let Some(error) = scenario.get("inboundResponseError") {
        json!({"functionCallId": id, "error": error})
    } else {
        json!({"functionCallId": id, "value": scenario["inboundResponseValue"]})
    };
    receive(
        &mut session,
        json!({"version": "1.0", "agentFunctionResponse": response}),
        now,
    )?;
    let update = session.advance(
        now,
        Some(&mut |message| {
            messages.push(message);
            Ok(())
        }),
    );
    let result = expected
        .get("resolvedValue")
        .or_else(|| expected.get("result"))
        .ok_or("missing expected settled value")?;
    if session.value(observation).as_ref() != Some(result) || !messages.is_empty() {
        return Err("unexpected settled value or extra outbound message".into());
    }
    for (sibling, value) in siblings {
        if session.value(sibling).as_ref() != Some(value) {
            return Err("agent error modified a sibling binding".into());
        }
    }
    assert_surface_error(&update.errors, expected.get("surfaceError"), &id)
}

fn create_session(
    upstream: &Path,
    scenario: &Value,
    surface: &str,
    catalog_id: &str,
    now: Instant,
) -> Result<RoutingSession<'static>, TestError> {
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common: Value = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    catalog["catalogId"] = Value::from(catalog_id);
    let config = ValidationConfig {
        allow_unknown_elements: true,
        ..ValidationConfig::default()
    };
    let mut session = RoutingSession::new(
        RpcHandler::new(MessageProcessor::new(config)),
        LocalFunctions::default(),
    );
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    let mut create = json!({"surfaceId": surface, "catalogId": catalog_id});
    if !scenario["components"].is_null() {
        create["components"] = scenario["components"].clone();
    } else if !scenario["component"].is_null() {
        create["components"] = json!([scenario["component"]]);
    }
    receive(
        &mut session,
        json!({"version": "1.0", "createSurface": create}),
        now,
    )?;

    Ok(session)
}

fn assert_surface_error(
    errors: &[a2ui_rs::protocol::RendererToAgentMessage],
    expected: Option<&Value>,
    id: &str,
) -> Result<(), TestError> {
    if let Some(error) = expected {
        let actual = errors
            .first()
            .map(serde_json::to_value)
            .transpose()?
            .ok_or("missing surface error")?;
        if errors.len() != 1
            || actual["error"]["code"] != error["code"]
            || error["functionCallId"] != "$outboundCallId"
            || actual["error"]["functionCallId"] != id
        {
            return Err(format!("unexpected correlated error {actual}").into());
        }
    } else if !errors.is_empty() {
        return Err("unexpected routing errors".into());
    }
    Ok(())
}
