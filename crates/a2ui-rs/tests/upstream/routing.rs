use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{FunctionCall, RendererToAgent, RendererToAgentMessage};
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{EvaluationId, EventAction, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use serde::Deserialize;
use serde::de::{MapAccess, Visitor};
use serde_json::{Value, json};

use super::{Case, TestError};

#[path = "routing_ui.rs"]
mod ui;

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    ui::supports(suite, case)
        || ui::supports_check_metadata(suite, case)
        || ui::supports_local_action(suite, case)
        || ui::supports_open_url(suite, case)
        || (suite == "core/agent_function_routing.yaml"
            && case.action == "route_agent_function"
            && case.protocol_version.as_deref() == Some("1.0"))
}

pub(super) fn non_json_argument_fixture(suite: &str, case: &Case) -> bool {
    suite == "core/agent_function_routing.yaml"
        && case.name == "test_agent_routing_non_json_argument_fails_locally"
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    if case.action == "handle_rpc" {
        return ui::execute(upstream, case);
    }
    if case.action == "validate" {
        return ui::execute_check_metadata(upstream, case);
    }
    if case.action == "dispatch_action" {
        if ui::supports_open_url("core/actions.yaml", case) {
            return ui::execute_open_url(upstream, case);
        }
        return ui::execute_local_action(upstream, case);
    }
    let mut session = build_session(upstream, case)?;
    let now = Instant::now();
    let mut create = json!({"surfaceId": "main", "dataModel": case.data_model.clone().unwrap_or_else(|| json!({}))});
    if let Some(catalog) = case
        .surface
        .as_ref()
        .and_then(|surface| surface.get("catalogId"))
    {
        create["catalogId"] = catalog.clone();
    }
    receive(
        &mut session,
        json!({"version": "1.0", "createSurface": create}),
        now,
    )?;

    let ordered = if case.steps.iter().any(|step| step["as"] == "event") {
        let source =
            fs::read_to_string(upstream.join("conformance/core/agent_function_routing.yaml"))?;
        let mut options = serde_saphyr::Options::default();
        options.non_finite_float_policy = serde_saphyr::NonFiniteFloatPolicy::AsString;
        let cases: Vec<EventCase> = serde_saphyr::from_str_with_options(&source, options)?;
        Some(
            cases
                .into_iter()
                .find(|item| item.name == case.name)
                .ok_or("missing ordered fixture")?,
        )
    } else {
        None
    };
    let listener = case
        .surface
        .as_ref()
        .is_none_or(|surface| surface["outboundListener"] != false);
    let mut driver = Driver {
        session,
        listener,
        now,
        calls: Vec::new(),
        observed: Vec::new(),
    };
    if non_json_argument_fixture("core/agent_function_routing.yaml", case) {
        return assert_non_json_arguments(case, &mut driver);
    }
    for (index, step) in case.steps.iter().enumerate() {
        let context = ordered
            .as_ref()
            .and_then(|case| case.steps.get(index))
            .and_then(|step| step.evaluate.as_ref())
            .map(|entry| entry.context.clone());
        driver
            .step(step, context)
            .map_err(|error| format!("step {}: {error}", index + 1))?;
    }
    Ok(())
}

fn assert_non_json_arguments(case: &Case, driver: &mut Driver) -> Result<(), TestError> {
    let [binding, action] = case.steps.as_slice() else {
        return Err("expected a binding and a function action".into());
    };
    if binding["as"] != Value::Null
        || binding["expectValues"] != json!([null])
        || action["as"] != "action"
    {
        return Err("unexpected non-finite fixture steps".into());
    }
    for (step, literal, finite) in [(binding, "NaN", 123), (action, "Infinity", 124)] {
        let call = &step["evaluate"];
        let name = call["@call"].as_str().ok_or("missing function name")?;
        let token = call["args"]["amount"]
            .as_str()
            .ok_or("missing non-finite argument")?;
        let mut options = serde_saphyr::Options::default();
        options.non_finite_float_policy = serde_saphyr::NonFiniteFloatPolicy::PassThrough;
        let value: f64 = serde_saphyr::from_str_with_options(token, options)?;
        let expected = json!([{ "code": "EXPRESSION_ERROR", "message": name }]);
        if value.is_finite()
            || serde_json::Number::from_f64(value).is_some()
            || step["expectErrors"] != expected
            || (literal == "NaN") != value.is_nan()
        {
            return Err(format!("unexpected non-JSON fixture argument {token}").into());
        }
        let catalog = case.surface.as_ref().ok_or("missing surface")?["catalogId"].clone();
        let wire = json!({
            "version": "1.0",
            "callAgentFunction": {
                "surfaceId": "main", "functionCallId": "non-json",
                "callFunction": {"@call": name, "catalogId": catalog, "args": {"amount": 123}}
            }
        })
        .to_string();
        serde_json::from_str::<RendererToAgentMessage>(&wire)?;
        for invalid in [literal, "1e400"] {
            let invalid_wire = wire.replace(r#""amount":123"#, &format!(r#""amount":{invalid}"#));
            if serde_json::from_str::<RendererToAgentMessage>(&invalid_wire).is_ok() {
                return Err(
                    format!("accepted an RPC argument with no JSON form: {invalid}").into(),
                );
            }
        }
        driver.step(
            &json!({
                "evaluate": {"@call": name, "args": {"amount": finite}},
                "as": step["as"],
                "expectOutbound": [{"@call": name, "catalogId": catalog, "args": {"amount": finite}}],
            }),
            None,
        )?;
    }
    Ok(())
}

fn build_session(upstream: &Path, case: &Case) -> Result<RoutingSession<'static>, TestError> {
    let basic: Value =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common: Value = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let mut rpc = RpcHandler::default();
    let mut local = LocalFunctions::default();
    for spec in case
        .catalogs
        .as_ref()
        .and_then(Value::as_array)
        .ok_or("missing fixture catalogs")?
    {
        let id = spec["catalogId"].as_str().ok_or("missing catalog ID")?;
        let mut entries = serde_json::Map::new();
        for name in spec["localFunctions"].as_array().into_iter().flatten() {
            let name = name.as_str().ok_or("invalid fixture function name")?;
            let schema = basic["functions"]
                .get(name)
                .ok_or("unknown Basic Catalog function")?;
            entries.insert(name.to_owned(), schema.clone());
            if matches!(name, "regex" | "pluralize") {
                // These cases verify that the implementation is not reached.
                local.insert(id, name, |_| {
                    Err("The fixture's forbidden handler ran".to_owned())
                });
            }
        }
        for (name, spec) in spec["agentFunctions"].as_object().into_iter().flatten() {
            let mut schema = json!({
                "properties": {"@call": {"const": name}, "args": spec["args"]},
                "required": ["args"],
            });
            for key in ["returnType", "requiresUserActivation", "allowedCallers"] {
                if let Some(value) = spec.get(key) {
                    schema[key] = value.clone();
                }
            }
            entries.insert(name.clone(), schema);
        }
        rpc.add_catalog(Catalog::from_json(
            json!({"protocolVersion": "1.0", "catalogId": id, "functions": entries}),
            &common,
        )?)?;
    }
    Ok(RoutingSession::new(rpc, local))
}

struct Driver {
    session: RoutingSession<'static>,
    listener: bool,
    now: Instant,
    calls: Vec<String>,
    observed: Vec<EvaluationId>,
}

impl Driver {
    fn step(
        &mut self,
        step: &Value,
        ordered: Option<Vec<(String, Value)>>,
    ) -> Result<(), TestError> {
        if let Some(expression) = step.get("evaluate") {
            self.evaluate(
                expression.clone(),
                step["as"].as_str().unwrap_or("binding"),
                ordered,
            )?;
        } else if let Some(response) = step.get("respond") {
            self.respond(response)?;
        } else if let Some(update) = step.get("updateData") {
            let path = update["path"].as_str().ok_or("missing data path")?;
            self.session.write_binding(
                "main",
                &DataScope::default(),
                path,
                update["value"].clone(),
            )?;
        } else if step["cancelSurfaceCalls"] == true {
            self.session.cancel_surface_calls("main");
        } else if step["deleteSurface"] == true {
            receive(
                &mut self.session,
                json!({"version": "1.0", "deleteSurface": {"surfaceId": "main"}}),
                self.now,
            )?;
        } else {
            return Err(format!("unknown routing step {step}").into());
        }
        self.check_update(step)
    }

    fn evaluate(
        &mut self,
        expression: Value,
        mode: &str,
        ordered: Option<Vec<(String, Value)>>,
    ) -> Result<(), TestError> {
        let scope = DataScope::default();
        match mode {
            "binding" => self
                .observed
                .push(self.session.mount_binding("main", scope, expression)),
            "check" => self
                .observed
                .push(self.session.mount_check("main", scope, expression)),
            "action" => {
                self.session.run_action("main", scope, expression, true);
            }
            "event" => {
                self.session.dispatch_event(
                    "main",
                    scope,
                    EventAction {
                        name: expression["name"]
                            .as_str()
                            .ok_or("missing event name")?
                            .to_owned(),
                        source_component_id: "root".to_owned(),
                        timestamp: "2026-01-01T00:00:00Z".to_owned(),
                        context: ordered.ok_or("missing ordered event context")?,
                        user_message: expression.get("userMessage").cloned(),
                    },
                    true,
                );
            }
            _ => return Err(format!("unknown evaluation mode {mode}").into()),
        }
        Ok(())
    }

    fn respond(&mut self, response: &Value) -> Result<(), TestError> {
        let index = usize::try_from(response["call"].as_u64().ok_or("missing call index")?)?;
        let id = self
            .calls
            .get(index)
            .ok_or("attempt to settle an unsent call")?;
        let mut message = json!({"functionCallId": id});
        if let Some(error) = response.get("error") {
            message["error"] = error.clone();
        } else {
            message["value"] = response["value"].clone();
        }
        receive(
            &mut self.session,
            json!({"version": "1.0", "agentFunctionResponse": message}),
            self.now,
        )
    }

    fn check_update(&mut self, step: &Value) -> Result<(), TestError> {
        let mut outbox = Vec::new();
        let mut sink = |message| {
            outbox.push(message);
            Ok(())
        };
        let listener: Option<&mut a2ui_rs::rpc::Outbound<'_>> =
            if self.listener { Some(&mut sink) } else { None };
        let update = self.session.advance(self.now, listener);
        let mut calls = Vec::new();
        let mut actions = Vec::new();
        for RendererToAgentMessage::V1(message) in outbox {
            match message {
                RendererToAgent::CallAgentFunction(call) if call.surface_id == "main" => {
                    if call.function_call_id.is_empty()
                        || self.calls.contains(&call.function_call_id)
                    {
                        return Err("invalid or reused correlation ID".into());
                    }
                    self.calls.push(call.function_call_id);
                    calls.push(call.call_function);
                }
                RendererToAgent::Action(action) if action.surface_id == "main" => {
                    let mut output = json!({"name": action.name, "context": action.context});
                    if let Some(message) = action.user_message {
                        output["userMessage"] = Value::from(message);
                    }
                    actions.push(output);
                }
                _ => return Err(format!("unexpected outbound operation {message:?}").into()),
            }
        }
        assert_calls(calls, step)?;
        assert_list(
            "actions",
            &actions,
            step["expectActions"].as_array().map_or(&[], Vec::as_slice),
        )?;
        assert_errors(update.errors, &step["expectErrors"])?;
        if let Some(expected) = step["expectValues"].as_array() {
            let values = self
                .observed
                .iter()
                .map(|&id| self.session.value(id).ok_or("missing observation"))
                .collect::<Result<Vec<_>, _>>()?;
            assert_list("values", &values, expected)?;
        }
        Ok(())
    }
}

fn receive(session: &mut RoutingSession<'_>, input: Value, now: Instant) -> Result<(), TestError> {
    if session
        .receive(serde_json::from_value(input)?, now)?
        .is_some()
    {
        return Err("an inbound routing message returned an immediate response".into());
    }
    Ok(())
}

fn assert_calls(actual: Vec<FunctionCall>, step: &Value) -> Result<(), TestError> {
    let mut actual = actual
        .into_iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;
    let mut expected = step["expectOutbound"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if step["outboundInAnyOrder"] == true {
        actual.sort_by_key(Value::to_string);
        expected.sort_by_key(Value::to_string);
    }
    assert_list("calls", &actual, &expected)
}

fn assert_errors(actual: Vec<RendererToAgentMessage>, expected: &Value) -> Result<(), TestError> {
    let expected = expected.as_array().map_or(&[][..], Vec::as_slice);
    if expected.len() != actual.len() {
        return Err(format!("expected errors {expected:?}, got {actual:?}").into());
    }
    for (actual, expected) in actual.into_iter().zip(expected) {
        let actual = serde_json::to_value(actual)?;
        let error = &actual["error"];
        if error["code"] != expected["code"]
            || expected["message"].as_str().is_some_and(|expected| {
                error["message"]
                    .as_str()
                    .is_none_or(|message| !message.contains(expected))
            })
        {
            return Err(format!("expected error {expected}, got {error}").into());
        }
    }
    Ok(())
}

fn assert_list(label: &str, actual: &[Value], expected: &[Value]) -> Result<(), TestError> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("expected {label} {expected:?}, got {actual:?}").into())
    }
}

#[derive(Deserialize)]
struct EventCase {
    name: String,
    steps: Vec<EventStep>,
}

#[derive(Deserialize)]
struct EventStep {
    evaluate: Option<OrderedEvent>,
}

#[derive(Deserialize)]
struct OrderedEvent {
    #[serde(default, deserialize_with = "ordered_context")]
    context: Vec<(String, Value)>,
}

fn ordered_context<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<(String, Value)>, D::Error> {
    struct Entries;
    impl<'de> Visitor<'de> for Entries {
        type Value = Vec<(String, Value)>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("ordered event context entries")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut entries = Vec::new();
            while let Some(entry) = map.next_entry()? {
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    deserializer.deserialize_map(Entries)
}
