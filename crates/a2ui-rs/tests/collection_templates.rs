//! Tests writable bindings and instance lifetimes in nested collections.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{MountedNode, NodeChangeKind, NodeId, NodeWriteError, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use serde_json::{Value, json};

const SURFACE: &str = "shipments";

#[test]
fn nested_templates_keep_item_scopes_separate() -> Result<(), Box<dyn Error>> {
    let (mut session, now) = session()?;
    let rows = children(&session, None)?;
    let [london, bristol] = rows.as_slice() else {
        return Err("expected two shipments".into());
    };
    let london_fields = children(&session, Some(*london))?;
    let bristol_fields = children(&session, Some(*bristol))?;
    let [title, london_lines] = london_fields.as_slice() else {
        return Err("expected title and lines".into());
    };
    let [_, bristol_lines] = bristol_fields.as_slice() else {
        return Err("expected title and lines".into());
    };
    let items = children(&session, Some(*london_lines))?;
    let [labels, boxes] = items.as_slice() else {
        return Err("expected two London items".into());
    };
    let tape = *children(&session, Some(*bristol_lines))?
        .first()
        .ok_or("missing tape row")?;
    assert_eq!(node(&session, *boxes)?.scope.path(), "/shipments/0/lines/1");
    assert_eq!(
        node(&session, *boxes)?.properties["label"]
            .binding_path
            .as_deref(),
        Some("/lineLabel")
    );
    assert_eq!(
        node(&session, *title)?.properties["text"].value,
        json!("London")
    );

    session.write_node_property(SURFACE, *boxes, "value", json!("Small boxes"))?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert_eq!(update.nodes.len(), 1);
    assert_eq!(update.nodes[0].id, *boxes);
    assert_eq!(update.nodes[0].kind, NodeChangeKind::Updated);
    assert_eq!(
        node(&session, *boxes)?.properties["value"].value,
        json!("Small boxes")
    );
    assert_eq!(
        node(&session, *labels)?.properties["value"].value,
        json!("Labels")
    );
    assert_eq!(
        node(&session, tape)?.properties["value"].value,
        json!("Tape")
    );
    assert!(matches!(
        session.write_node_property(SURFACE, *london_lines, "children", json!([])),
        Err(NodeWriteError::ReadOnly(_))
    ));

    session.write_binding(
        SURFACE,
        &DataScope::default(),
        "/shipments/0/lines",
        json!([{"name": "Padded envelopes"}]),
    )?;
    assert!(matches!(
        session.write_node_property(SURFACE, *boxes, "value", json!("Stale write")),
        Err(NodeWriteError::UnknownNode(id)) if id == *boxes
    ));
    let update = session.advance(now, None);
    assert_eq!(children(&session, Some(*london_lines))?, [*labels]);
    assert_eq!(
        node(&session, *labels)?.properties["value"].value,
        json!("Padded envelopes")
    );
    assert_eq!(children(&session, Some(*bristol_lines))?, [tape]);
    assert!(
        update
            .nodes
            .iter()
            .any(|change| change.id == *boxes && change.kind == NodeChangeKind::Disposed)
    );
    assert!(session.advance(now, None).nodes.is_empty());

    session.write_binding(
        SURFACE,
        &DataScope::default(),
        "/shipments/0/lines/1",
        json!({"name": "Bags"}),
    )?;
    let update = session.advance(now, None);
    let grown = children(&session, Some(*london_lines))?;
    assert_eq!(grown.len(), 2);
    assert_eq!(grown[0], *labels);
    assert_ne!(grown[1], *boxes);
    assert!(
        update
            .nodes
            .iter()
            .any(|change| change.id == grown[1] && change.kind == NodeChangeKind::Mounted)
    );
    assert_eq!(
        node(&session, grown[1])?.properties["value"].value,
        json!("Bags")
    );
    Ok(())
}

#[test]
fn an_action_can_change_collection_membership_in_one_advance() -> Result<(), Box<dyn Error>> {
    let (mut session, now) = session()?;
    let shipments = children(&session, None)?;
    let removed = shipments[1];

    session.run_action(
        SURFACE,
        DataScope::default(),
        json!({"@call": "dropLastShipment"}),
        true,
    );
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert_eq!(children(&session, None)?, [shipments[0]]);
    assert!(
        update
            .nodes
            .iter()
            .any(|change| change.id == removed && change.kind == NodeChangeKind::Disposed)
    );
    assert!(session.advance(now, None).nodes.is_empty());
    Ok(())
}

fn session() -> Result<(RoutingSession<'static>, Instant), Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common: Value = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    catalog["functions"]["dropLastShipment"] = json!({"returnType": "boolean"});
    let catalog = Catalog::from_json(catalog, &common)?;
    let mut functions = LocalFunctions::default();
    functions.insert_action(catalog.id(), "dropLastShipment", |_, context| {
        let mut shipments = context
            .get("/shipments")
            .map_err(|error| error.to_string())?
            .and_then(Value::as_array)
            .ok_or("missing shipments array")?
            .clone();
        shipments.pop();
        context
            .set("/shipments", Value::Array(shipments))
            .map_err(|error| error.to_string())?;
        Ok(Value::Bool(true))
    });
    let mut session = RoutingSession::new(RpcHandler::default(), functions);
    session.add_catalog(catalog)?;
    session.mount_tree(SURFACE);
    let now = Instant::now();
    session.receive(
        serde_json::from_str(include_str!(
            "../../../fixtures/local/collection-templates.json"
        ))?,
        now,
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    Ok((session, now))
}

fn node<'a>(
    session: &'a RoutingSession<'_>,
    id: NodeId,
) -> Result<&'a MountedNode, Box<dyn Error>> {
    session
        .node_tree(SURFACE)
        .and_then(|tree| tree.node(id))
        .ok_or_else(|| "missing node".into())
}

fn children(
    session: &RoutingSession<'_>,
    parent: Option<NodeId>,
) -> Result<Vec<NodeId>, Box<dyn Error>> {
    let tree = session.node_tree(SURFACE).ok_or("missing tree")?;
    let id = parent.or_else(|| tree.root()).ok_or("missing parent")?;
    Ok(node(session, id)?
        .children
        .iter()
        .map(|child| child.id)
        .collect())
}
