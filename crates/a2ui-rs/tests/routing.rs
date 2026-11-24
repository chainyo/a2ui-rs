//! Tests reactive routing across failed sibling calls and data updates.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{
    AgentToRendererMessage, CallAgentFunction, RendererToAgent, RendererToAgentMessage,
};
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{RoutingSession, RoutingUpdate};
use a2ui_rs::rpc::RpcHandler;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Fixture {
    create: AgentToRendererMessage,
    binding: Value,
}

#[test]
fn failed_later_argument_stops_earlier_sibling_until_arguments_change() -> Result<(), Box<dyn Error>>
{
    let common = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/upstream/specification/v1_0/json/common_types.json");
    let common = serde_json::from_slice(&fs::read(common)?)?;
    let fixture: Fixture =
        serde_json::from_str(include_str!("../../../fixtures/local/agent-routing.json"))?;
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    session.add_catalog(Catalog::from_json(
        json!({"protocolVersion": "1.0", "catalogId": "example:shipping"}),
        &common,
    )?)?;
    let now = Instant::now();
    let scope = DataScope::default();
    session.receive(fixture.create, now)?;
    let binding = session.mount_binding("shipment", scope.clone(), fixture.binding);
    let (update, calls) = advance(&mut session, now)?;
    assert!(update.errors.is_empty());
    let [address, stock] = calls.try_into().map_err(|_| "expected two sibling calls")?;
    respond(
        &mut session,
        now,
        &json!({"functionCallId": stock.function_call_id,
        "error": {"code": "UNAVAILABLE", "message": "Stock service offline"}}),
    )?;
    let (update, calls) = advance(&mut session, now)?;
    assert!(calls.is_empty());
    assert_eq!(update.errors.len(), 1);
    assert!(session.next_deadline().is_none());

    respond(
        &mut session,
        now,
        &json!({"functionCallId": address.function_call_id, "value": "late address"}),
    )?;
    session.write_binding("shipment", &scope, "/unrelated", json!(true))?;
    let (update, calls) = advance(&mut session, now)?;
    assert!(calls.is_empty());
    assert!(update.errors.is_empty());
    assert_eq!(session.value(binding), Some(Value::Null));

    session.write_binding("shipment", &scope, "/sku", json!("SKU-43"))?;
    let (update, mut calls) = advance(&mut session, now)?;
    assert!(update.errors.is_empty());
    calls.sort_by(|left, right| left.call_function.name.cmp(&right.call_function.name));
    let [address, stock] = calls.try_into().map_err(|_| "expected new sibling calls")?;
    assert_eq!(address.call_function.name, "fetchAddress");
    assert_eq!(stock.call_function.name, "fetchStock");
    respond(
        &mut session,
        now,
        &json!({"functionCallId": address.function_call_id, "value": "Seattle"}),
    )?;
    respond(
        &mut session,
        now,
        &json!({"functionCallId": stock.function_call_id, "value": 3}),
    )?;
    let (_, calls) = advance(&mut session, now)?;
    let [quote] = calls
        .try_into()
        .map_err(|_| "expected one shipment quote")?;
    assert_eq!(quote.call_function.name, "quoteShipment");
    assert_eq!(
        serde_json::to_value(quote.call_function.args)?,
        json!({"address": "Seattle", "stock": 3})
    );
    Ok(())
}

fn respond(
    session: &mut RoutingSession<'_>,
    now: Instant,
    response: &Value,
) -> Result<(), Box<dyn Error>> {
    let message = json!({"version": "1.0", "agentFunctionResponse": response});
    assert!(
        session
            .receive(serde_json::from_value(message)?, now)?
            .is_none()
    );
    Ok(())
}

fn advance(
    session: &mut RoutingSession<'_>,
    now: Instant,
) -> Result<(RoutingUpdate, Vec<CallAgentFunction>), Box<dyn Error>> {
    let mut messages = Vec::new();
    let update = session.advance(
        now,
        Some(&mut |message| {
            messages.push(message);
            Ok(())
        }),
    );
    let calls = messages
        .into_iter()
        .map(|message| match message {
            RendererToAgentMessage::V1(RendererToAgent::CallAgentFunction(call)) => Ok(call),
            RendererToAgentMessage::V1(_) => Err("unexpected outbound message"),
        })
        .collect::<Result<_, _>>()?;
    Ok((update, calls))
}

#[test]
fn mounted_rule_updates_message_and_preserves_structured_metadata() -> Result<(), Box<dyn Error>> {
    let (mut session, now, _) = effects_session()?;
    let scope = DataScope::default().nested("draft");
    let check = session.mount_check_rule("shipment", scope.clone(), "root", 0);
    session.advance(now, None);
    assert_eq!(
        serde_json::to_value(session.validation_result(check))?,
        json!({"valid": false, "message": "Enter the full postal code", "severity": "error"})
    );

    let update = json!({"version": "1.0", "updateComponents": {
        "surfaceId": "shipment",
        "components": [{
            "id": "root", "component": "TextField", "label": "Postal code", "value": "E1",
            "@checks": [{"condition": {"@path": "postalCodeCheck"}, "message": "Check the spelling"}]
        }]
    }});
    session.receive(serde_json::from_value(update)?, now)?;
    assert_eq!(session.advance(now, None).changed, [check]);
    assert_eq!(
        serde_json::to_value(session.validation_result(check))?,
        json!({"valid": false, "message": "Check the spelling", "severity": "error"})
    );

    let metadata = json!({
        "valid": false, "code": "PARTIAL_POSTAL_CODE", "message": "Add the last three characters",
        "severity": "warning", "completion": "E1 6AN"
    });
    session.write_binding("shipment", &scope, "postalCodeCheck", metadata.clone())?;
    assert_eq!(session.advance(now, None).changed, [check]);
    assert_eq!(
        serde_json::to_value(session.validation_result(check))?,
        metadata
    );
    Ok(())
}

#[test]
fn scoped_action_effects_run_once_and_stop_after_an_error() -> Result<(), Box<dyn Error>> {
    let (mut session, now, action) = effects_session()?;
    let scope = DataScope::default().nested("draft");
    let attempts = session.mount_binding("shipment", scope.clone(), json!({"@path": "attempts"}));
    let action = session.run_action("shipment", scope.clone(), action, true);

    let (update, calls) = advance(&mut session, now)?;
    assert!(update.errors.is_empty());
    let [validation] = calls
        .try_into()
        .map_err(|_| "expected one remote validation")?;
    assert_eq!(validation.call_function.name, "validateAddress");
    assert_eq!(session.value(attempts), Some(json!(1)));
    let (_, calls) = advance(&mut session, now)?;
    assert!(calls.is_empty());
    assert_eq!(session.value(attempts), Some(json!(1)));

    respond(
        &mut session,
        now,
        &json!({
            "functionCallId": validation.function_call_id,
            "error": {"code": "UNAVAILABLE", "message": "Address service offline"}
        }),
    )?;
    let (update, calls) = advance(&mut session, now)?;
    assert_eq!(update.errors.len(), 1);
    assert!(update.changed.contains(&action));
    assert!(calls.is_empty());
    let root = || -> Result<Value, Box<dyn Error>> {
        Ok(session
            .processor()
            .get("shipment")
            .ok_or("missing shipment")?
            .data_model()
            .root()
            .clone())
    };
    assert_eq!(
        root()?,
        json!({"draft": {"attempts": 1, "postalCodeCheck": false}})
    );

    session.run_action("shipment", scope, json!({"@call": "failSave"}), true);
    let update = session.advance(now, None);
    assert_eq!(update.errors.len(), 1);
    assert_eq!(
        serde_json::to_value(&update.errors[0])?["error"]["code"],
        "EXECUTION_ERROR"
    );
    let surface = session
        .processor()
        .get("shipment")
        .ok_or("missing shipment")?;
    assert_eq!(surface.data_model().get("/draft/saved")?, None);
    assert_eq!(
        surface.data_model().get("/draft/attempts")?,
        Some(&json!(1))
    );
    Ok(())
}

#[derive(Deserialize)]
struct EffectsFixture {
    create: AgentToRendererMessage,
    action: Value,
}

fn effects_session() -> Result<(RoutingSession<'static>, Instant, Value), Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let catalog_id = "example:shipping";
    catalog["catalogId"] = json!(catalog_id);
    let mut functions = LocalFunctions::default();
    for name in ["recordAttempt", "saveDraft", "failSave"] {
        catalog["functions"][name] = json!({
            "returnType": "boolean", "properties": {"args": {"type": "object"}}
        });
    }
    functions.insert_action(catalog_id, "recordAttempt", |args, context| {
        let attempts = args["attempts"].as_u64().ok_or("missing attempt count")?;
        context
            .set("attempts", json!(attempts + 1))
            .map_err(|error| error.to_string())?;
        Ok(json!(true))
    });
    functions.insert_action(catalog_id, "saveDraft", |_, context| {
        context
            .set("saved", json!(true))
            .map_err(|error| error.to_string())?;
        Ok(json!(true))
    });
    functions.insert_action(catalog_id, "failSave", |_, context| {
        context
            .set("saved", json!(true))
            .map_err(|error| error.to_string())?;
        Err("Could not save draft".to_owned())
    });
    let fixture: EffectsFixture = serde_json::from_str(include_str!(
        "../../../fixtures/local/check-rules-action-effects.json"
    ))?;
    let mut session = RoutingSession::new(RpcHandler::default(), functions);
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    let now = Instant::now();
    session.receive(fixture.create, now)?;
    Ok((session, now, fixture.action))
}
