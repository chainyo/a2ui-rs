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
