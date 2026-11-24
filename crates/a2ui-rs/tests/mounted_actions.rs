//! Tests action routing from mounted collection items.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::Metadata;
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{
    ActionDispatchOptions, EvaluationState, NodeActionError, NodeId, RoutingSession,
};
use a2ui_rs::rpc::RpcHandler;
use serde_json::{Value, json};

const SURFACE: &str = "shipments";
const TIMESTAMP: &str = "2026-01-01T00:00:00Z";

#[test]
fn mounted_event_reads_current_item_data_and_sends_metadata() -> Result<(), Box<dyn Error>> {
    let (mut session, now, rows) = session()?;
    let row = rows[1];
    let scope = DataScope::default();
    let metadata = json!({"extensions": {"com_example_confirmation": "confirmed"}});
    let dispatch_options = options(true, Some(serde_json::from_value(metadata.clone())?));
    let id = session.dispatch_node_action(SURFACE, row, "action", dispatch_options)?;
    session.write_binding(SURFACE, &scope, "/shipments/1/name", json!("Bath"))?;
    session.write_binding(
        SURFACE,
        &scope,
        "/shipments/1/message",
        json!("Requested Bath shipment"),
    )?;

    let (messages, errors) = advance(&mut session, now)?;
    assert_eq!(
        messages,
        vec![json!({"version": "1.0", "action": {
            "name": "requestShipment", "surfaceId": SURFACE, "sourceComponentId": "request",
            "timestamp": TIMESTAMP, "context": {"shipment": "Bath"},
            "userMessage": "Requested Bath shipment", "metadata": metadata
        }})]
    );
    assert!(errors.is_empty());
    assert!(matches!(
        session.status(id),
        Some(EvaluationState::Ready(_))
    ));
    assert!(advance(&mut session, now)?.0.is_empty());

    assert!(matches!(
        session.dispatch_node_action(SURFACE, row, "child", options(true, None)),
        Err(NodeActionError::NotAction(property)) if property == "child"
    ));
    session.write_binding(SURFACE, &scope, "/shipments", json!([{"name": "London"}]))?;
    assert!(matches!(
        session.dispatch_node_action(SURFACE, row, "action", options(true, None)),
        Err(NodeActionError::UnknownNode(id)) if id == row
    ));
    Ok(())
}

#[test]
fn mounted_function_uses_latest_definition_scope_and_activation() -> Result<(), Box<dyn Error>> {
    let (mut session, now, rows) = session()?;
    let update = json!({"version": "1.0", "updateComponents": {
        "surfaceId": SURFACE,
        "components": [{
            "id": "request", "component": "Button", "child": "label",
            "action": {"functionCall": {"@call": "saveShipment", "args": {"name": {"@path": "name"}}}}
        }]
    }});
    session.receive(serde_json::from_value(update)?, now)?;
    let row = rows[1];

    let denied = session.dispatch_node_action(SURFACE, row, "action", options(false, None))?;
    let (messages, errors) = advance(&mut session, now)?;
    assert!(messages.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        session.status(denied),
        Some(EvaluationState::Failed(_))
    ));
    assert_eq!(data(&session, "/shipments/1/saved")?, Some(&json!(false)));

    let invoked = session.dispatch_node_action(SURFACE, row, "action", options(true, None))?;
    let (messages, errors) = advance(&mut session, now)?;
    assert!(messages.is_empty());
    assert!(errors.is_empty());
    assert_eq!(session.value(invoked), Some(json!("Bristol")));
    assert_eq!(data(&session, "/shipments/0/saved")?, Some(&json!(false)));
    assert_eq!(data(&session, "/shipments/1/saved")?, Some(&json!(true)));
    Ok(())
}

#[test]
fn a_literal_in_an_action_union_cannot_be_dispatched() -> Result<(), Box<dyn Error>> {
    let (mut session, now, rows) = session()?;
    let literal = json!({"event": "selected"});
    let update = json!({"version": "1.0", "updateComponents": {
        "surfaceId": SURFACE,
        "components": [{
            "id": "request", "component": "Button", "child": "label", "action": literal
        }]
    }});
    session.receive(serde_json::from_value(update)?, now)?;
    assert!(session.advance(now, None).errors.is_empty());
    let row = rows[1];
    let node = session
        .node_tree(SURFACE)
        .and_then(|tree| tree.node(row))
        .ok_or("missing row")?;
    assert!(!node.properties["action"].is_action);
    assert_eq!(node.properties["action"].value, literal);
    assert!(matches!(
        session.dispatch_node_action(SURFACE, row, "action", options(true, None)),
        Err(NodeActionError::NotAction(property)) if property == "action"
    ));
    Ok(())
}

fn session() -> Result<(RoutingSession<'static>, Instant, Vec<NodeId>), Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common: Value = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    catalog["components"]["Button"]["allOf"][1]["properties"]["action"] = json!({
        "oneOf": [
            {"$ref": "common_types.json#/$defs/Action"},
            {"type": "object", "properties": {"event": {"type": "string"}}, "required": ["event"]}
        ]
    });
    catalog["functions"]["saveShipment"] = json!({
        "returnType": "string", "requiresUserActivation": true,
        "properties": {"args": {
            "type": "object", "properties": {"name": {"$ref": "common_types.json#/$defs/DynamicString"}},
            "required": ["name"]
        }}
    });
    let catalog = Catalog::from_json(catalog, &common)?;
    let mut functions = LocalFunctions::default();
    functions.insert_action(catalog.id(), "saveShipment", |args, context| {
        let name = args.get("name").ok_or("missing shipment name")?.clone();
        context
            .set("saved", Value::Bool(true))
            .map_err(|error| error.to_string())?;
        Ok(name)
    });
    let mut session = RoutingSession::new(RpcHandler::default(), functions);
    session.add_catalog(catalog)?;
    session.mount_tree(SURFACE);
    let now = Instant::now();
    session.receive(
        serde_json::from_str(include_str!("../../../fixtures/local/mounted-actions.json"))?,
        now,
    )?;
    assert!(session.advance(now, None).errors.is_empty());
    let tree = session.node_tree(SURFACE).ok_or("missing tree")?;
    let root = tree
        .node(tree.root().ok_or("missing root")?)
        .ok_or("missing root node")?;
    let rows = root.children.iter().map(|child| child.id).collect();
    Ok((session, now, rows))
}

fn options(user_activated: bool, metadata: Option<Metadata>) -> ActionDispatchOptions {
    ActionDispatchOptions {
        timestamp: TIMESTAMP.to_owned(),
        user_activated,
        metadata,
    }
}

fn advance(
    session: &mut RoutingSession<'_>,
    now: Instant,
) -> Result<(Vec<Value>, Vec<Value>), Box<dyn Error>> {
    let mut sent = Vec::new();
    let update = session.advance(
        now,
        Some(&mut |message| {
            sent.push(message);
            Ok(())
        }),
    );
    let messages = sent
        .into_iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    let errors = update
        .errors
        .into_iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    Ok((messages, errors))
}

fn data<'a>(
    session: &'a RoutingSession<'_>,
    path: &str,
) -> Result<Option<&'a Value>, Box<dyn Error>> {
    Ok(session
        .processor()
        .get(SURFACE)
        .ok_or("missing surface")?
        .data_model()
        .get(path)?)
}
