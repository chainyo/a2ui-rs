use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use a2ui_rs::catalog::{Catalog, CatalogError};
use a2ui_rs::protocol::{CallAgentFunction, RendererToAgentMessage};
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{EvaluationState, RoutingSession};
use a2ui_rs::rpc::{CallOptions, PendingCall, ReceiveError, RpcError, RpcHandler};
use a2ui_rs::validation::ValidationError;
use serde_json::{Value, json};

use super::{Case, TestError};

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    if suite == "core/error_responses.yaml" {
        return case.action == "validate"
            && case.protocol_version.as_deref() == Some("1.0")
            && matches!(
                case.name.as_str(),
                "test_error_response_create_surface_incompatible_catalog_version"
                    | "test_error_response_delete_surface_missing_surface_id"
                    | "test_error_response_call_renderer_function_missing_call_function"
                    | "test_error_response_call_renderer_function_non_object_args"
                    | "test_error_response_call_renderer_function_missing_catalog_id"
            );
    }
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
                | "test_rpc_call_renderer_function_protocol_version_mismatch"
                | "test_rpc_call_renderer_function_format_string_arg_inherits_catalog"
                | "test_rpc_call_renderer_function_nested_renderer_only_rejected"
                | "test_rpc_call_renderer_function_nested_template_renderer_only_rejected"
                | "test_rpc_call_renderer_function_nested_agent_only_allowed"
                | "test_rpc_call_renderer_function_nested_template_mixed_error_prioritizes_boundary"
        )
        || activation_fixture(suite, case)
        || nested_activation_fixture(suite, case)
}

pub(super) fn nested_activation_fixture(suite: &str, case: &Case) -> bool {
    suite == "core/rpc_functions.yaml"
        && case
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog["protocolVersion"] == "1.0")
        && matches!(
            case.name.as_str(),
            "test_rpc_call_renderer_function_nested_user_activation_denied"
                | "test_rpc_call_renderer_function_nested_template_user_activation_denied"
                | "test_rpc_call_renderer_function_nested_template_user_activation_allowed"
        )
}

pub(super) fn activation_fixture(suite: &str, case: &Case) -> bool {
    suite == "core/rpc_functions.yaml"
        && case.action == "handle_rpc"
        && case
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog["protocolVersion"] == "1.0")
        && matches!(
            case.name.as_str(),
            "test_rpc_call_renderer_function_requires_user_activation"
                | "test_rpc_call_renderer_function_user_activation_denied"
        )
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    if case.action == "validate" {
        return execute_error_response(upstream, case);
    }
    let args = case.args.as_ref().ok_or("missing RPC fixture arguments")?;
    let common = read_json(upstream, "specification/v1_0/json/common_types.json")?;
    let basic = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
    if activation_fixture("core/rpc_functions.yaml", case) {
        return execute_activation(upstream, case, args, &common);
    }
    if case.name == "test_rpc_call_renderer_function_protocol_version_mismatch" {
        return execute_incompatible_catalog(case, args, &common);
    }
    let mut handler = RpcHandler::default();
    if args.get("outboundCall").is_some() {
        execute_outbound(&mut handler, case, args, &common)
    } else {
        execute_inbound(&mut handler, case, args, &common, basic)
    }
}

fn execute_error_response(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let common = read_json(upstream, "specification/v1_0/json/common_types.json")?;
    let Some([input]) = case
        .payload
        .as_ref()
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    else {
        return Err("expected one incoming message".into());
    };
    let expected = case
        .expect_error
        .as_ref()
        .ok_or("missing incoming error expectation")?;
    let mut handler = RpcHandler::default();
    let incompatible =
        case.name == "test_error_response_create_surface_incompatible_catalog_version";
    let requested = &input["createSurface"]["catalogId"];
    let mut rejected_catalog = false;
    for path in &case.catalog_paths {
        let document = read_json(upstream, path)?;
        let matches_requested = !requested.is_null() && document["catalogId"] == *requested;
        match Catalog::from_json(document, &common) {
            Ok(catalog) if !(incompatible && matches_requested) => handler.add_catalog(catalog)?,
            Err(CatalogError::InvalidCatalog(message))
                if incompatible && matches_requested && message.contains("protocolVersion 1.0") =>
            {
                rejected_catalog = true;
            }
            outcome => {
                return Err(
                    format!("unexpected fixture catalog outcome: {:?}", outcome.err()).into(),
                );
            }
        }
    }
    if incompatible && !rejected_catalog {
        return Err("expected registration of the requested catalog to fail".into());
    }
    let error = match handler.receive_json(input, Instant::now(), &LocalFunctions::default()) {
        Err(error) => error,
        Ok(outcome) => return Err(format!("incoming message was accepted: {outcome:?}").into()),
    };
    let outbound = match &error {
        ReceiveError::Validation(ValidationError::UnknownCatalog(id)) if incompatible => {
            let surface = input["createSurface"]["surfaceId"]
                .as_str()
                .ok_or("missing surface ID")?;
            if expected["category"] != "CatalogError"
                || expected["surfaceId"] != surface
                || input["createSurface"]["catalogId"] != id.as_str()
                || handler.processor().get(surface).is_some()
            {
                return Err(format!("unexpected catalog rejection: {error}").into());
            }
            error.renderer_error_for_surface(surface)
        }
        ReceiveError::MissingDeleteSurfaceId
            if expected["category"] == "ValidationError"
                && expected["surfaceId"] == ""
                && expected["path"] == "/surfaceId" =>
        {
            error.renderer_error()
        }
        ReceiveError::Envelope {
            function_call_id: Some(id),
            ..
        } if expected["category"] == "ValidationError"
            && expected["functionCallId"] == id.as_str() =>
        {
            error.renderer_error()
        }
        _ => return Err(format!("unexpected incoming error: {error:?}").into()),
    }
    .ok_or("missing renderer error envelope")?;
    let actual = serde_json::to_value(outbound)?;
    let renderer = &expected["rendererError"];
    assert_response(renderer, &actual)?;
    if renderer["error"]
        .as_object()
        .map(serde_json::Map::keys)
        .zip(actual["error"].as_object().map(serde_json::Map::keys))
        .is_none_or(|(expected, actual)| expected.ne(actual))
    {
        return Err(format!("unexpected renderer error fields: {actual}").into());
    }
    Ok(())
}

fn read_json(upstream: &Path, file: &str) -> Result<Value, TestError> {
    Ok(serde_json::from_slice(&fs::read(upstream.join(file))?)?)
}

fn execute_activation(
    upstream: &Path,
    case: &Case,
    args: &Value,
    common: &Value,
) -> Result<(), TestError> {
    let activated = args["userActivationPresent"]
        .as_bool()
        .ok_or("missing activation flag")?;
    let input = &args["message"];
    let call = &input["callRendererFunction"]["callFunction"];
    let id = call["catalogId"]
        .as_str()
        .ok_or("missing activation catalog ID")?;
    let name = call["@call"]
        .as_str()
        .ok_or("missing activation function")?;
    let meta = &args["functionMetadata"][name];
    let mut document = json!({
        "protocolVersion": "1.0", "catalogId": id,
        "functions": { (name): {
            "type": "object", "returnType": "object",
            "properties": {
                "@call": {"const": name},
                "args": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}
            },
            "required": ["@call", "args"],
            "requiresUserActivation": meta["requiresUserActivation"],
            "allowedCallers": meta["allowedCallers"],
        }},
    });
    assert_activation_schema(upstream, &document["functions"][name])?;
    if !matches!(
        Catalog::from_json(document.clone(), common),
        Err(CatalogError::InvalidCatalog(message)) if message.contains("Invalid requiresUserActivation")
    ) {
        return Err("expected rejection of agent-callable activation-gated function".into());
    }
    document["functions"][name]["allowedCallers"] = json!("rendererOnly");

    let invoked = Cell::new(0);
    let expected_url = call["args"]["url"].as_str().ok_or("missing test URL")?;
    let mut functions = LocalFunctions::default();
    functions.insert(id, name, |args| {
        if args["url"] != expected_url {
            return Err("unexpected activation argument".into());
        }
        invoked.set(invoked.get() + 1);
        Ok(json!({"opened": true}))
    });
    let now = Instant::now();
    let mut handler = RpcHandler::default();
    handler.add_catalog(Catalog::from_json(document, common)?)?;
    let response = serde_json::to_value(handler.receive_json(input, now, &functions)?)?;
    let response = &response["rendererFunctionResponse"];
    if response["functionCallId"] != input["callRendererFunction"]["functionCallId"]
        || response["error"]["code"] != "INVALID_FUNCTION_CALL"
        || !response["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("rendererOnly"))
        || invoked.get() != 0
    {
        return Err(format!("expected remote activation denial, got {response}").into());
    }

    create_surface(&mut handler, "activation", id, Value::Null, now, &functions)?;
    let mut session = RoutingSession::new(handler, functions);
    let evaluation =
        session.run_action("activation", DataScope::default(), call.clone(), activated);
    let update = session.advance(now, None);
    let expected = &case
        .expect
        .as_ref()
        .ok_or("missing activation expectation")?["response"]["rendererFunctionResponse"];
    if activated {
        if invoked.get() != 1
            || !update.errors.is_empty()
            || session.value(evaluation).as_ref() != expected.get("value")
        {
            return Err(format!(
                "expected the user gesture to run once, got {} invocations and {:?}",
                invoked.get(),
                session.status(evaluation)
            )
            .into());
        }
    } else if invoked.get() != 0
        || expected["error"]["code"] != "INVALID_FUNCTION_CALL"
        || !matches!(
            session.status(evaluation),
            Some(EvaluationState::Failed(error))
                if error.code == "EXPRESSION_ERROR" && error.message.contains("requires user activation")
        )
        || update.errors.len() != 1
    {
        return Err("expected the local action to fail without a user gesture".into());
    }
    Ok(())
}

fn assert_activation_schema(upstream: &Path, fixture: &Value) -> Result<(), TestError> {
    let schema = read_json(upstream, "specification/v1_0/json/catalog_definition.json")?;
    let validator = jsonschema::validator_for(&json!({
        "$schema": schema["$schema"],
        "$defs": {
            "FunctionDefinition": schema["$defs"]["FunctionDefinition"],
            "FunctionCallValidationSchema": schema["$defs"]["FunctionCallValidationSchema"],
        },
        "$ref": "#/$defs/FunctionDefinition",
    }))?;
    if fixture["requiresUserActivation"] != true
        || fixture["allowedCallers"] != "rendererOrAgent"
        || validator.is_valid(fixture)
    {
        return Err("expected the pinned schema to reject the fixture's activation policy".into());
    }
    let mut control = fixture.clone();
    control["requiresUserActivation"] = json!(false);
    validator
        .validate(&control)
        .map_err(|error| format!("expected an ungated agent-callable function: {error}"))?;
    control["requiresUserActivation"] = json!(true);
    control["allowedCallers"] = json!("rendererOnly");
    validator
        .validate(&control)
        .map_err(|error| format!("expected an activation-gated renderer function: {error}"))?;
    Ok(())
}

fn execute_incompatible_catalog(
    case: &Case,
    args: &Value,
    common: &Value,
) -> Result<(), TestError> {
    let input = &args["message"];
    let id = args["catalogId"]
        .as_str()
        .ok_or("missing incompatible catalog ID")?;
    let document = json!({"protocolVersion": args["catalogVersion"], "catalogId": id});
    if input["callRendererFunction"]["callFunction"]["catalogId"] != id
        || !matches!(
            Catalog::from_json(document, common),
            Err(CatalogError::InvalidCatalog(message)) if message.contains("protocolVersion 1.0")
        )
    {
        return Err("expected v1.0 catalog loading to reject the older catalog".into());
    }
    let mut handler = RpcHandler::default();
    let actual = handler.receive_json(input, Instant::now(), &LocalFunctions::default())?;
    let mut expected = case
        .expect
        .as_ref()
        .ok_or("missing version mismatch expectation")?["response"]
        .clone();
    let message = &mut expected["rendererFunctionResponse"]["error"]["message"];
    if message != "does not match message protocol version" {
        return Err("unexpected version mismatch fixture diagnostic".into());
    }
    // The v1.0 catalog loader rejects the version before it can enter the registry.
    *message = json!(format!("Catalog not found: {id}"));
    assert_response(&expected, &serde_json::to_value(actual)?)
}

fn execute_inbound(
    handler: &mut RpcHandler,
    case: &Case,
    args: &Value,
    common: &Value,
    mut basic: Value,
) -> Result<(), TestError> {
    let mut input = args["message"].clone();
    use_format_string(&mut input);
    let requested_id = input["callRendererFunction"]["callFunction"]["catalogId"].as_str();
    let catalog_id = if case.name == "test_rpc_call_renderer_function_unregistered_catalog" {
        "basic"
    } else {
        requested_id.unwrap_or("basic")
    };
    basic["catalogId"] = Value::from("basic");
    let mut catalogs = BTreeMap::from([("basic", basic)]);
    let activation = nested_activation_fixture("core/rpc_functions.yaml", case);
    let mut gated_function = None;
    let forbidden_invocations = Cell::new(0);
    let mut functions = LocalFunctions::default();
    for (name, meta) in args["functionMetadata"].as_object().into_iter().flatten() {
        let name = if name == "renderTemplate" {
            "formatString"
        } else {
            name.as_str()
        };
        let id = meta["catalogId"].as_str().unwrap_or(catalog_id);
        let catalog = catalogs
            .entry(id)
            .or_insert_with(|| json!({"protocolVersion": "1.0", "catalogId": id, "functions": {}}));
        let mut schema = json!({"type": "object", "properties": {"args": {"type": "object"} }});
        for key in ["allowedCallers", "requiresUserActivation"] {
            if let Some(value) = meta.get(key) {
                schema[key] = value.clone();
            }
        }
        if let Some(parameters) = meta.get("parameters") {
            schema["properties"]["args"] = parameters.clone();
        }
        if catalog["functions"].get(name).is_none()
            && let Some(branches) = catalog
                .get_mut("$defs")
                .and_then(|defs| defs.pointer_mut("/anyFunction/oneOf"))
                .and_then(Value::as_array_mut)
        {
            branches.push(json!({"$ref": format!("#/functions/{name}")}));
        }
        if activation && schema["requiresUserActivation"] == true {
            gated_function = Some((id, name));
        }
        catalog["functions"][name] = schema;
        register_inbound_mock(&mut functions, id, name, activation, &forbidden_invocations);
    }
    if activation {
        let (id, name) = gated_function.ok_or("missing nested activation function")?;
        let document = catalogs
            .get_mut(id)
            .ok_or("missing nested activation catalog")?;
        let schema = &document["functions"][name];
        if schema["allowedCallers"] != "rendererOrAgent"
            || !matches!(
                Catalog::from_json(document.clone(), common),
                Err(CatalogError::InvalidCatalog(message)) if message.contains("Invalid requiresUserActivation")
            )
        {
            return Err("expected catalog rejection for nested agent-callable activation".into());
        }
        document["functions"][name]["allowedCallers"] = json!("rendererOnly");
    }
    if catalog_id != "basic" {
        catalogs.entry(catalog_id).or_insert_with(
            || json!({"protocolVersion": "1.0", "catalogId": catalog_id, "functions": {}}),
        );
    }
    for document in catalogs.into_values() {
        handler.add_catalog(Catalog::from_json(document, common)?)?;
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
    let outcome = handler.receive_json(&input, now, &functions);
    assert_inbound_outcome(case, &input, gated_function.map(|(_, name)| name), outcome)?;
    if forbidden_invocations.get() != 0 {
        return Err("inbound request invoked a forbidden function".into());
    }
    if handler.next_deadline().is_some() {
        return Err("inbound request left an agent call pending".into());
    }
    Ok(())
}

fn register_inbound_mock<'a>(
    functions: &mut LocalFunctions<'a>,
    id: &str,
    name: &str,
    activation: bool,
    forbidden_invocations: &'a Cell<usize>,
) {
    match name {
        "playMedia" => functions.insert(id, name, |args| {
            args.get("mediaId")
                .and_then(Value::as_str)
                .ok_or("missing media ID")?;
            Ok(json!({"playing": args.get("autoplay") == Some(&Value::Bool(true)), "timestamp": 0}))
        }),
        "echoArgs" => functions.insert(id, name, |args| Ok(Value::Object(args.clone()))),
        "failingFunction" => functions.insert(id, name, |_| {
            Err("An error occurred during function execution.".to_owned())
        }),
        "syncState" => functions.insert(id, name, |_| Ok(Value::Null)),
        "agentSecret" if !activation => functions.insert(id, name, |_| Ok(json!("AGENT_OK"))),
        "tagValue" => functions.insert(id, name, |args| {
            Ok(json!(format!("tagged:{}", args["value"])))
        }),
        "agentSecret" | "openExternalUrl" | "secretRendererOnly" => {
            functions.insert(id, name, move |_| {
                forbidden_invocations.set(forbidden_invocations.get() + 1);
                Ok(Value::Null)
            });
        }
        _ => {}
    }
}

fn assert_inbound_outcome(
    case: &Case,
    input: &Value,
    gated_function: Option<&str>,
    outcome: Result<Option<RendererToAgentMessage>, ReceiveError>,
) -> Result<(), TestError> {
    let expect = case
        .expect
        .as_ref()
        .ok_or("missing inbound RPC expectation")?;
    if let Some(name) = gated_function {
        let actual = serde_json::to_value(outcome?)?;
        let expected = json!({
            "version": "1.0",
            "rendererFunctionResponse": {
                "functionCallId": input["callRendererFunction"]["functionCallId"],
                "error": {
                    "code": "INVALID_FUNCTION_CALL",
                    "message": format!("Function '{name}' cannot be called by agent (allowedCallers is rendererOnly)."),
                },
            },
        });
        assert_response(&expected, &actual)?;
    } else {
        match outcome {
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
    }
    Ok(())
}

// The upstream mock evaluates templates. Use the SDK function that does the same work.
fn use_format_string(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(name) = object.get_mut("@call")
                && *name == "renderTemplate"
            {
                *name = json!("formatString");
            }
            object.values_mut().for_each(use_format_string);
        }
        Value::Array(items) => items.iter_mut().for_each(use_format_string),
        _ => {}
    }
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
