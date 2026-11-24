use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::basic_catalog::register_validation_functions;
use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{EvaluationId, EvaluationState, RoutingSession};
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
                | "test_rpc_ui_check_agent_error_invalidates_rule"
                | "test_rpc_ui_action_agent_error_halts_pipeline"
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
    let observation = mount_observation(&mut session, surface, scenario, expected);
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
    if !messages.is_empty() {
        return Err("unexpected extra outbound message".into());
    }
    assert_observation(&session, surface, observation, expected)?;
    for (sibling, value) in siblings {
        if session.value(sibling).as_ref() != Some(value) {
            return Err("agent error modified a sibling binding".into());
        }
    }
    assert_surface_error(&update.errors, expected.get("surfaceError"), &id)
}

fn mount_observation(
    session: &mut RoutingSession<'_>,
    surface: &str,
    scenario: &Value,
    expected: &Value,
) -> EvaluationId {
    let scope = DataScope::default();
    let component = scenario["observe"]["componentId"]
        .as_str()
        .unwrap_or("root");
    if let Some(call) = scenario["actionPayload"].get("functionCall") {
        session.run_action(surface, scope, call.clone(), true)
    } else if expected.get("validationResult").is_some() {
        session.mount_check_rule(surface, scope, component, 0)
    } else {
        let property = scenario["observe"]["property"].as_str().unwrap_or("text");
        session.mount_property(surface, scope, component, property)
    }
}

fn assert_observation(
    session: &RoutingSession<'_>,
    surface: &str,
    observation: EvaluationId,
    expected: &Value,
) -> Result<(), TestError> {
    if let Some(result) = expected.get("validationResult") {
        let actual = serde_json::to_value(session.validation_result(observation))?;
        if &actual != result {
            return Err(format!("expected validation result {result}, got {actual}").into());
        }
    } else if let Some(data) = expected.get("dataModel") {
        let actual = session
            .processor()
            .get(surface)
            .ok_or("missing surface")?
            .data_model();
        if actual.root() != data
            || !matches!(
                session.status(observation),
                Some(EvaluationState::Failed(_))
            )
        {
            return Err(format!(
                "expected failed action with data {data}, got {}",
                actual.root()
            )
            .into());
        }
    } else {
        let result = expected
            .get("resolvedValue")
            .or_else(|| expected.get("result"))
            .ok_or("missing expected settled value")?;
        if session.value(observation).as_ref() != Some(result) {
            return Err("unexpected settled value".into());
        }
    }
    Ok(())
}

fn create_session(
    upstream: &Path,
    scenario: &Value,
    surface: &str,
    catalog_id: &str,
    now: Instant,
) -> Result<RoutingSession<'static>, TestError> {
    let (mut catalog, common) = basic_catalog(upstream, catalog_id)?;
    let mut functions = LocalFunctions::default();
    for (name, function) in scenario["localFunctions"].as_object().into_iter().flatten() {
        catalog["functions"][name] = json!({
            "returnType": function["returnType"],
            "properties": {"args": {"type": "object"}}
        });
        let effect = &function["effect"]["setData"];
        let path = effect["path"]
            .as_str()
            .ok_or("missing effect path")?
            .to_owned();
        let value = effect.get("value").ok_or("missing effect value")?.clone();
        functions.insert_action(catalog_id, name, move |_, context| {
            context
                .set(&path, value.clone())
                .map_err(|error| error.to_string())?;
            Ok(Value::Bool(true))
        });
    }
    let config = ValidationConfig {
        allow_unknown_elements: true,
        ..ValidationConfig::default()
    };
    let mut session =
        RoutingSession::new(RpcHandler::new(MessageProcessor::new(config)), functions);
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

fn basic_catalog(upstream: &Path, catalog_id: &str) -> Result<(Value, Value), TestError> {
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    catalog["catalogId"] = Value::from(catalog_id);
    Ok((catalog, common))
}

pub(super) fn supports_check_metadata(suite: &str, case: &Case) -> bool {
    suite == "core/validation_result.yaml"
        && case.action == "validate"
        && case
            .catalog
            .as_ref()
            .and_then(|catalog| catalog["protocolVersion"].as_str())
            == Some("1.0")
        && matches!(
            case.name.as_str(),
            "test_validation_result_warning_and_info_non_blocking"
                | "test_validation_result_dynamic_object_return"
                | "test_validation_result_reactive_recovery_on_data_update"
                | "test_validation_result_coercion_in_logical_and_or_not"
        )
}

pub(super) fn execute_check_metadata(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let first = case.steps.first().ok_or("missing check steps")?;
    let create = &first["payload"][0]["createSurface"];
    let surface = create["surfaceId"].as_str().ok_or("missing surface ID")?;
    let catalog_id = create["catalogId"].as_str().ok_or("missing catalog ID")?;
    let (catalog, common) = basic_catalog(upstream, catalog_id)?;
    let mut functions = LocalFunctions::default();
    register_validation_functions(&mut functions, catalog_id);
    let mut session = RoutingSession::new(RpcHandler::default(), functions);
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    let now = Instant::now();
    session.mount_tree(surface);

    for step in &case.steps {
        for message in step["payload"].as_array().ok_or("missing check messages")? {
            receive(&mut session, message.clone(), now)?;
            let update = session.advance(now, None);
            if !update.errors.is_empty() {
                return Err(format!("unexpected check errors: {:?}", update.errors).into());
            }
        }
        if let Some(expected) = step.get("expect") {
            assert_check_properties(&session, surface, expected)?;
        }
    }
    if let Some(expected) = &case.expect {
        assert_check_properties(&session, surface, expected)?;
    }
    Ok(())
}

fn assert_check_properties(
    session: &RoutingSession<'_>,
    surface: &str,
    expected: &Value,
) -> Result<(), TestError> {
    let expected = &expected["surfaces"][surface];
    let tree = session.node_tree(surface).ok_or("missing mounted tree")?;
    let mut pending = vec![tree.root().ok_or("missing mounted root")?];
    let mut nodes = std::collections::BTreeMap::new();
    while let Some(id) = pending.pop() {
        let node = tree.node(id).ok_or("missing mounted node")?;
        nodes.insert(node.component_id.as_str(), node);
        pending.extend(node.children.iter().map(|child| child.id));
    }
    for name in [
        "isValid",
        "validationErrors",
        "validationResults",
        "validationResult",
    ] {
        for (id, expected) in expected[name].as_object().into_iter().flatten() {
            let node = nodes.get(id.as_str()).ok_or("missing check component")?;
            let value = if name == "validationResult" {
                &node
                    .properties
                    .get("validationResults")
                    .ok_or("missing check results")?
                    .value[0]
            } else {
                &node
                    .properties
                    .get(name)
                    .ok_or("missing validation property")?
                    .value
            };
            if value != expected {
                return Err(format!("unexpected {name} on {id}: {value}").into());
            }
        }
    }
    Ok(())
}

pub(super) fn supports_local_action(suite: &str, case: &Case) -> bool {
    suite == "core/actions.yaml"
        && case.action == "dispatch_action"
        && case.protocol_version.as_deref() == Some("1.0")
        && matches!(
            case.name.as_str(),
            "test_action_function_call_local_mutation_no_event_dispatched"
                | "test_action_function_call_execution_error"
        )
}

pub(super) fn execute_local_action(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let surface = case.surface_id.as_deref().ok_or("missing surface ID")?;
    let call = case
        .action_payload
        .as_ref()
        .and_then(|payload| payload.get("functionCall"))
        .ok_or("missing action call")?;
    let catalog_id = "action_conformance";
    let mut functions = LocalFunctions::default();
    functions.insert_action(catalog_id, "incrementCounter", |args, context| {
        let path = args["path"].as_str().ok_or("missing counter path")?;
        let step = args["step"].as_i64().ok_or("invalid counter step")?;
        let count = context
            .get(path)
            .map_err(|error| error.to_string())?
            .and_then(Value::as_i64)
            .ok_or("invalid counter value")?;
        context
            .set(
                path,
                json!(count.checked_add(step).ok_or("counter overflow")?),
            )
            .map_err(|error| error.to_string())?;
        Ok(Value::Null)
    });
    functions.insert_action(catalog_id, "failingAction", |_, _| {
        Err("Action failed".to_owned())
    });
    let catalog = json!({
        "protocolVersion": "1.0", "catalogId": catalog_id,
        "functions": {
            "incrementCounter": {"properties": {"args": {"type": "object"}}},
            "failingAction": {"properties": {"args": {"type": "object"}}}
        }
    });
    let mut session = RoutingSession::new(RpcHandler::default(), functions);
    let common: Value = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    let now = Instant::now();
    let mut create = json!({"surfaceId": surface, "catalogId": catalog_id});
    if let Some(data) = &case.data_model {
        create["dataModel"] = data.clone();
    }
    receive(
        &mut session,
        json!({"version": "1.0", "createSurface": create}),
        now,
    )?;

    let action = session.run_action(surface, DataScope::default(), call.clone(), true);
    let mut dispatched = Vec::new();
    let update = session.advance(
        now,
        Some(&mut |message| {
            dispatched.push(message);
            Ok(())
        }),
    );
    if !dispatched.is_empty() {
        return Err(format!("unexpected outbound messages: {dispatched:?}").into());
    }
    if let Some(expected) = &case.expect_error {
        let [error] = update.errors.as_slice() else {
            return Err("expected one action error".into());
        };
        let actual = serde_json::to_value(error)?;
        if actual["error"]["code"] != expected["code"]
            || actual["error"]["surfaceId"] != expected["surfaceId"]
        {
            return Err(format!("unexpected action error: {actual}").into());
        }
    } else if !update.errors.is_empty()
        || !matches!(session.status(action), Some(EvaluationState::Ready(_)))
    {
        return Err(format!("action did not complete: {:?}", update.errors).into());
    }
    if let Some(expected) = &case.expect_data_model {
        let data = session
            .processor()
            .get(surface)
            .ok_or("missing surface")?
            .data_model();
        if data.root() != expected {
            return Err(format!("expected action data {expected}, got {}", data.root()).into());
        }
    }
    Ok(())
}
