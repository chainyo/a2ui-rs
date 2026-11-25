//! Tests writable bindings and instance lifetimes in nested collections.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{ErrorTarget, RendererError, RendererToAgent, RendererToAgentMessage};
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{
    MountedNode, NodeChangeKind, NodeId, NodeWriteError, RoutingSession, RoutingUpdate,
};
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

#[test]
fn bounds_a_wide_list_and_recovers_at_the_exact_limit() -> Result<(), Box<dyn Error>> {
    let (mut session, now) = session()?;
    let initial_shipments = children(&session, None)?;
    let surface = "incoming";
    session.mount_tree_with_budget(surface, 3);
    session.mount_tree(surface);
    session.receive(
        serde_json::from_value(json!({
            "version": "1.0",
            "createSurface": {
                "surfaceId": surface,
                "catalogId": "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json",
                "dataModel": {"orders": vec![json!({"status": "Incoming"}); 20_000]},
                "components": [
                    {"id": "root", "component": "Column", "children": {"componentId": "order", "path": "/orders"}},
                    {"id": "order", "component": "Text", "text": {"@path": "status"}}
                ]
            }
        }))?,
        now,
    )?;
    let overflow = session.advance(now, None);
    assert_budget_error(&overflow, surface, 3)?;
    assert_eq!(overflow.nodes.len(), 3);
    let tree = session.node_tree(surface).ok_or("missing tree")?;
    assert!(tree.is_over_budget());
    let root = tree
        .root()
        .and_then(|id| tree.node(id))
        .ok_or("missing root")?;
    let [first, second] = root.children.as_slice() else {
        return Err("expected two order nodes".into());
    };
    let root_id = root.id;
    let order_ids = [first.id, second.id];
    assert_eq!(
        tree.node(second.id).ok_or("missing order")?.scope.path(),
        "/orders/1"
    );
    assert_eq!(children(&session, None)?, initial_shipments);
    assert!(session.advance(now, None).errors.is_empty());

    session.write_binding(
        surface,
        &DataScope::default(),
        "/orders",
        json!([{"status": "Packed"}, {"status": "Sent"}]),
    )?;
    assert!(session.advance(now, None).errors.is_empty());
    let tree = session.node_tree(surface).ok_or("missing recovered tree")?;
    assert!(!tree.is_over_budget());
    assert_eq!(tree.root(), Some(root_id));
    let root = tree.node(root_id).ok_or("missing recovered root")?;
    assert_eq!(
        root.children
            .iter()
            .map(|child| child.id)
            .collect::<Vec<_>>(),
        order_ids
    );
    assert_eq!(
        tree.node(order_ids[1]).ok_or("missing order")?.properties["text"].value,
        json!("Sent")
    );
    Ok(())
}

#[test]
fn nested_collections_share_one_budget_and_mount_omitted_nodes_after_shrink()
-> Result<(), Box<dyn Error>> {
    let (mut session, now) = session()?;
    let shipments = children(&session, None)?;
    let old_bristol_fields = children(&session, Some(shipments[1]))?;
    let removed_lines = old_bristol_fields[1];
    let removed_tape = children(&session, Some(removed_lines))?[0];
    session.mount_tree_with_budget(SURFACE, 8);
    let overflow = session.advance(now, None);
    assert_budget_error(&overflow, SURFACE, 8)?;
    assert_eq!(children(&session, None)?, shipments);
    assert_eq!(
        children(&session, Some(shipments[1]))?,
        old_bristol_fields[..1]
    );
    for id in [removed_lines, removed_tape] {
        assert!(node(&session, id).is_err());
        assert!(
            overflow
                .nodes
                .iter()
                .any(|change| change.id == id && change.kind == NodeChangeKind::Disposed)
        );
    }

    session.write_binding(
        SURFACE,
        &DataScope::default(),
        "/shipments",
        json!([{"name": "London", "lines": []}, {"name": "Bristol", "lines": [{"name": "Tape"}]}]),
    )?;
    let recovered = session.advance(now, None);
    assert!(recovered.errors.is_empty());
    assert!(
        !session
            .node_tree(SURFACE)
            .ok_or("missing tree")?
            .is_over_budget()
    );
    assert_eq!(children(&session, None)?, shipments);
    let bristol_fields = children(&session, Some(shipments[1]))?;
    assert_eq!(bristol_fields[0], old_bristol_fields[0]);
    let restored_lines = bristol_fields[1];
    assert_ne!(restored_lines, removed_lines);
    let restored_tape = children(&session, Some(restored_lines))?[0];
    assert_ne!(restored_tape, removed_tape);
    assert_eq!(
        node(&session, restored_tape)?.properties["value"].value,
        json!("Tape")
    );
    assert!(
        recovered
            .nodes
            .iter()
            .any(|change| change.id == restored_tape && change.kind == NodeChangeKind::Mounted)
    );

    session.write_binding(
        SURFACE,
        &DataScope::default(),
        "/shipments/0/lines",
        json!([{"name": "Labels"}]),
    )?;
    assert_budget_error(&session.advance(now, None), SURFACE, 8)?;
    Ok(())
}

fn assert_budget_error(
    update: &RoutingUpdate,
    surface: &str,
    limit: usize,
) -> Result<(), Box<dyn Error>> {
    let [RendererToAgentMessage::V1(RendererToAgent::Error(RendererError::Generic(error)))] =
        update.errors.as_slice()
    else {
        return Err(format!("expected one budget error, got {:?}", update.errors).into());
    };
    assert_eq!(error.code, "MOUNTED_NODE_BUDGET_EXCEEDED");
    assert_eq!(error.target, ErrorTarget::SurfaceId(surface.to_owned()));
    assert!(error.message.contains(&limit.to_string()));
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
    catalog["$defs"]["anyFunction"]["oneOf"]
        .as_array_mut()
        .ok_or("missing Basic Catalog function union")?
        .push(json!({"$ref": "#/functions/dropLastShipment"}));
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
