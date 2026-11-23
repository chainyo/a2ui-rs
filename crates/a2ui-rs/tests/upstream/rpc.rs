use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{CallAgentFunction, RendererToAgentMessage};
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::rpc::{CallOptions, PendingCall, ReceiveError, RpcError, RpcHandler};
use serde_json::{Value, json};

use super::{Case, TestError};

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    suite == "core/rpc_functions.yaml"
        && case.action == "handle_rpc"
        && case
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog["protocolVersion"] == "1.0")
        && matches!(
            case.name.as_str(),
            "test_rpc_call_renderer_function_success"
                | "test_rpc_call_renderer_function_unauthorized_caller"
                | "test_rpc_call_agent_function_dispatch"
                | "test_rpc_call_renderer_function_execution_exception"
                | "test_rpc_call_renderer_function_agent_only_boundary"
                | "test_rpc_call_renderer_function_unregistered_function"
                | "test_rpc_call_renderer_function_unregistered_catalog"
                | "test_rpc_call_renderer_function_invalid_payload_missing_call_function"
                | "test_rpc_call_renderer_function_invalid_payload_non_object_args"
                | "test_rpc_call_renderer_function_unknown_surface_id"
                | "test_rpc_call_renderer_function_invalid_payload_missing_catalog_id"
                | "test_rpc_agent_function_response_unknown_call_id_ignored"
                | "test_rpc_agent_function_response_correlates_pending_call"
                | "test_rpc_call_renderer_function_invalid_argument_schema"
                | "test_rpc_call_agent_function_timeout_handling"
                | "test_rpc_call_agent_function_duplicate_call_id_error"
                | "test_rpc_call_agent_function_cancelled"
                | "test_rpc_call_agent_function_disposed"
                | "test_rpc_call_agent_function_no_listener"
                | "test_rpc_call_agent_function_late_response_after_timeout_ignored"
                | "test_rpc_call_agent_function_late_response_after_dispose_ignored"
                | "test_rpc_call_renderer_function_with_surface_id_and_dynamic_args"
                | "test_rpc_call_renderer_function_without_surface_id_isolated_data_model"
                | "test_rpc_call_renderer_function_index_at_root_execution_error"
        )
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let args = case.args.as_ref().ok_or("missing RPC fixture arguments")?;
    let common = read_json(upstream, "specification/v1_0/json/common_types.json")?;
    let basic = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
    let mut handler = RpcHandler::default();
    if args.get("outboundCall").is_some() {
        execute_outbound(&mut handler, case, args, &common)
    } else {
        execute_inbound(&mut handler, case, args, &common, basic)
    }
}

fn read_json(upstream: &Path, file: &str) -> Result<Value, TestError> {
    Ok(serde_json::from_slice(&fs::read(upstream.join(file))?)?)
}

fn execute_inbound(
    handler: &mut RpcHandler,
    case: &Case,
    args: &Value,
    common: &Value,
    mut basic: Value,
) -> Result<(), TestError> {
    let input = &args["message"];
    let requested_id = input["callRendererFunction"]["callFunction"]["catalogId"].as_str();
    let catalog_id = if case.name == "test_rpc_call_renderer_function_unregistered_catalog" {
        "basic"
    } else {
        requested_id.unwrap_or("basic")
    };
    basic["catalogId"] = Value::from("basic");
    let mut catalog = if catalog_id == "basic" {
        basic.clone()
    } else {
        json!({"protocolVersion": "1.0", "catalogId": catalog_id, "functions": {}})
    };
    let mut functions = LocalFunctions::default();
    for (name, meta) in args["functionMetadata"].as_object().into_iter().flatten() {
        let mut schema = json!({"type": "object", "properties": {"args": {"type": "object"} }});
        for key in ["allowedCallers", "requiresUserActivation"] {
            if let Some(value) = meta.get(key) {
                schema[key] = value.clone();
            }
        }
        if let Some(parameters) = meta.get("parameters") {
            schema["properties"]["args"] = parameters.clone();
        }
        catalog["functions"][name] = schema;
        match name.as_str() {
            "playMedia" => functions.insert(catalog_id, name, |args| {
                args.get("mediaId").and_then(Value::as_str).ok_or("missing media ID")?;
                Ok(json!({"playing": args.get("autoplay") == Some(&Value::Bool(true)), "timestamp": 0}))
            }),
            "echoArgs" => functions.insert(catalog_id, name, |args| Ok(Value::Object(args.clone()))),
            "failingFunction" => functions.insert(catalog_id, name, |_| Err("An error occurred during function execution.".to_owned())),
            "syncState" => functions.insert(catalog_id, name, |_| Ok(Value::Null)),
            _ => {}
        }
    }
    handler.add_catalog(Catalog::from_json(catalog, common)?)?;
    if catalog_id != "basic" {
        handler.add_catalog(Catalog::from_json(basic, common)?)?;
    }
    let now = Instant::now();
    if let Some(id) = args["surfaceId"].as_str() {
        create_surface(
            handler,
            id,
            catalog_id,
            args["dataModel"].clone(),
            now,
            &functions,
        )?;
    }
    let expect = case
        .expect
        .as_ref()
        .ok_or("missing inbound RPC expectation")?;
    match handler.receive_json(input, now, &functions) {
        Ok(response) if expect.get("response").is_some() => {
            assert_response(&expect["response"], &serde_json::to_value(response)?)?;
        }
        Err(error @ ReceiveError::Envelope { .. }) if expect.get("error").is_some() => {
            let outbound = error.renderer_error().ok_or("missing correlated error")?;
            let outbound = serde_json::to_value(outbound)?;
            assert_error(&expect["error"], &outbound["error"])?;
            if let Some(expect) = &case.expect_error {
                if expect["category"] != "ValidationError"
                    || expect["functionCallId"] != outbound["error"]["functionCallId"]
                {
                    return Err(format!("unexpected envelope error {error}").into());
                }
                assert_response(&expect["rendererError"], &outbound)?;
            }
        }
        actual => return Err(format!("unexpected inbound outcome {actual:?}").into()),
    }
    if handler.next_deadline().is_some() {
        return Err("inbound request left an agent call pending".into());
    }
    Ok(())
}

fn create_surface(
    handler: &mut RpcHandler,
    id: &str,
    catalog: &str,
    model: Value,
    now: Instant,
    functions: &LocalFunctions<'_>,
) -> Result<(), TestError> {
    let data = if model.is_null() { json!({}) } else { model };
    let input = json!({"version": "1.0", "createSurface": {"surfaceId": id, "catalogId": catalog, "dataModel": data}});
    if handler.receive_json(&input, now, functions)?.is_some() {
        return Err("createSurface sent an RPC message".into());
    }
    Ok(())
}

fn assert_response(expected: &Value, actual: &Value) -> Result<(), TestError> {
    if expected.is_null() && actual.is_null() {
        return Ok(());
    }
    let key = if expected.get("rendererFunctionResponse").is_some() {
        "rendererFunctionResponse"
    } else {
        "error"
    };
    let expected_payload = &expected[key];
    let actual_payload = &actual[key];
    if expected["version"] != actual["version"]
        || actual.as_object().is_none_or(|object| object.len() != 2)
    {
        return Err(format!("expected {expected}, got {actual}").into());
    }
    if key == "error" {
        return assert_error(expected_payload, actual_payload);
    }
    if expected_payload["functionCallId"] != actual_payload["functionCallId"]
        || actual_payload
            .as_object()
            .is_none_or(|object| object.len() != 2)
    {
        return Err(format!("expected {expected}, got {actual}").into());
    }
    if let Some(expected_error) = expected_payload.get("error") {
        assert_error(expected_error, &actual_payload["error"])
    } else if expected_payload.get("value") == actual_payload.get("value") {
        Ok(())
    } else {
        Err(format!("expected {expected}, got {actual}").into())
    }
}

fn assert_error(expected: &Value, actual: &Value) -> Result<(), TestError> {
    for (key, value) in expected.as_object().ok_or("missing expected error")? {
        let matches = if key == "message" {
            actual[key]
                .as_str()
                .zip(value.as_str())
                .is_some_and(|(actual, text)| actual.contains(text))
        } else {
            actual[key] == *value
        };
        if !matches {
            return Err(format!("expected error {expected}, got {actual}").into());
        }
    }
    Ok(())
}

fn execute_outbound(
    handler: &mut RpcHandler,
    case: &Case,
    args: &Value,
    common: &Value,
) -> Result<(), TestError> {
    let fixture = &args["outboundCall"];
    let request = outbound_request(fixture)?;
    let now = Instant::now();
    let functions = LocalFunctions::default();
    let catalog = request
        .call_function
        .catalog_id
        .as_deref()
        .ok_or("missing fixture catalog")?;
    handler.add_catalog(Catalog::from_json(
        json!({"catalogId": catalog, "protocolVersion": "1.0"}),
        common,
    )?)?;
    create_surface(
        handler,
        &request.surface_id,
        catalog,
        Value::Null,
        now,
        &functions,
    )?;
    let timeout = fixture["timeoutMs"].as_u64().map(Duration::from_millis);
    let mut sent = Vec::new();
    let mut pending = match submit(
        handler,
        request.clone(),
        timeout,
        now,
        &mut sent,
        args["outboundListener"] != false,
    ) {
        Ok(pending) => pending,
        Err(error) => {
            assert_rpc_error(
                &error,
                case.expect_error
                    .as_ref()
                    .ok_or("unexpected delivery error")?,
            )?;
            if !sent.is_empty() || handler.next_deadline().is_some() {
                return Err("refused call left an outbound request or pending deadline".into());
            }
            return Ok(());
        }
    };
    if sent.as_slice()
        != [RendererToAgentMessage::V1(
            a2ui_rs::protocol::RendererToAgent::CallAgentFunction(request.clone()),
        )]
    {
        return Err("outbound envelope does not match fixture".into());
    }

    let expect = case.expect.as_ref();
    if let Some(response) = args.get("inboundResponse") {
        return assert_correlated(
            handler,
            &mut pending,
            response,
            expect.ok_or("missing correlated result")?,
            now,
        );
    }

    let expected_error = case
        .expect_error
        .as_ref()
        .ok_or("missing expected RPC error")?;
    if let Some(duplicate) = args.get("secondOutboundCall") {
        let second = submit(
            handler,
            outbound_request(duplicate)?,
            timeout,
            now,
            &mut sent,
            true,
        );
        let error = second.err().ok_or("duplicate call was sent")?;
        assert_rpc_error(&error, expected_error)?;
        if sent.len() != 1 || handler.next_deadline().is_none() || pending.try_result().is_some() {
            return Err("duplicate changed the first call".into());
        }
        return Ok(());
    }

    assert_terminal(handler, &mut pending, request, case, args, now, timeout)?;
    if let Some(late) = args.get("lateInboundResponse")
        && (expect.is_none_or(|expect| expect["lateResponseIgnored"] != true)
            || handler
                .receive_json(late, now + timeout.unwrap_or_default(), &functions)?
                .is_some()
            || pending.try_result().is_some())
    {
        return Err("late response changed the outcome".into());
    }
    if sent.len() != 1 || handler.next_deadline().is_some() {
        return Err("settled call left an outbound request or pending deadline".into());
    }
    Ok(())
}

fn assert_terminal(
    handler: &mut RpcHandler,
    pending: &mut PendingCall,
    request: CallAgentFunction,
    case: &Case,
    args: &Value,
    now: Instant,
    timeout: Option<Duration>,
) -> Result<(), TestError> {
    let expected_error = case
        .expect_error
        .as_ref()
        .ok_or("missing expected RPC error")?;
    match args["afterCall"].as_str() {
        Some("cancel") => handler.cancel_surface_calls(&request.surface_id),
        Some("dispose") => handler.dispose(),
        None => handler.expire(now + timeout.ok_or("missing timeout")?),
        Some(action) => return Err(format!("unknown RPC action {action}").into()),
    }
    let error = pending
        .try_result()
        .ok_or("call was not settled")?
        .err()
        .ok_or("call unexpectedly succeeded")?;
    if args["afterCall"] == "dispose" {
        assert_rpc_error(
            &error,
            &case.expect.as_ref().ok_or("missing dispose expectation")?["inFlightError"],
        )?;
        let mut refused = Vec::new();
        let error = submit(handler, request, timeout, now, &mut refused, true)
            .err()
            .ok_or("disposed handler accepted a call")?;
        assert_rpc_error(&error, expected_error)?;
        if !refused.is_empty() {
            return Err("disposed handler sent an outbound request".into());
        }
    } else {
        assert_rpc_error(&error, expected_error)?;
    }
    Ok(())
}

fn assert_correlated(
    handler: &mut RpcHandler,
    pending: &mut PendingCall,
    response: &Value,
    expect: &Value,
    now: Instant,
) -> Result<(), TestError> {
    if handler
        .receive_json(response, now, &LocalFunctions::default())?
        .is_some()
    {
        return Err("agent response produced an outbound message".into());
    }
    if expect.get("correlatedCallId").is_some() && expect["correlatedCallId"] != pending.id() {
        return Err("wrong correlation ID".into());
    }
    let actual = pending
        .try_result()
        .ok_or("response did not settle the call")??;
    if actual != expect["result"] || handler.next_deadline().is_some() {
        return Err(format!("wrong correlated result {actual}").into());
    }
    Ok(())
}

fn outbound_request(fixture: &Value) -> Result<CallAgentFunction, TestError> {
    let mut input = fixture
        .as_object()
        .ok_or("invalid outbound fixture")?
        .clone();
    input.remove("timeoutMs");
    Ok(serde_json::from_value(Value::Object(input))?)
}

fn submit(
    handler: &mut RpcHandler,
    request: CallAgentFunction,
    timeout: Option<Duration>,
    now: Instant,
    sent: &mut Vec<RendererToAgentMessage>,
    enabled: bool,
) -> Result<PendingCall, RpcError> {
    let mut sink = |message| {
        sent.push(message);
        Ok(())
    };
    let outbound: Option<&mut a2ui_rs::rpc::Outbound<'_>> =
        if enabled { Some(&mut sink) } else { None };
    handler.call_agent_function(
        request,
        CallOptions {
            timeout,
            ..CallOptions::default()
        },
        now,
        &LocalFunctions::default(),
        outbound,
    )
}

fn assert_rpc_error(error: &RpcError, expected: &Value) -> Result<(), TestError> {
    if expected.as_object().is_none_or(|fields| fields.len() != 1)
        || expected["code"] != error.code()
    {
        return Err(format!("expected {expected}, got {}: {error}", error.code()).into());
    }
    Ok(())
}
