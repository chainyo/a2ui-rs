//! Tests scoped accessibility attributes on mounted nodes.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{
    MountedNode, NodeChange, NodeChangeKind, NodeId, NodeWriteError, RoutingSession,
};
use a2ui_rs::rpc::RpcHandler;
use serde_json::{Value, json};

const SURFACE: &str = "shipments";

#[test]
fn accessibility_resolves_per_item_and_keeps_node_ids() -> Result<(), Box<dyn Error>> {
    let (mut session, now) = session()?;
    let [london, bristol] = rows(&session)?;
    let previous_bristol = node(&session, bristol)?.clone();
    assert_eq!(
        previous_bristol.properties["@accessibility"].value,
        json!({
            "label": "Bristol", "description": "Status: Delivered", "live": "polite", "hidden": true
        })
    );
    assert_eq!(
        node(&session, london)?.properties["@accessibility"].value,
        json!({
            "label": "London", "description": "Status: In transit", "live": "polite", "hidden": false
        })
    );
    assert!(matches!(
        session.write_node_property(SURFACE, london, "@accessibility", json!({})),
        Err(NodeWriteError::ReadOnly(_))
    ));
    session.write_binding(
        SURFACE,
        &DataScope::default(),
        "/shipments/0",
        json!({
            "city": "Bath", "status": "Delayed", "hidden": true
        }),
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert_eq!(
        update.nodes,
        [NodeChange {
            surface_id: SURFACE.to_owned(),
            id: london,
            kind: NodeChangeKind::Updated,
        }]
    );
    assert_eq!(rows(&session)?, [london, bristol]);
    assert_eq!(
        node(&session, london)?.properties["@accessibility"].value,
        json!({
            "label": "Bath", "description": "Status: Delayed", "live": "polite", "hidden": true
        })
    );
    assert_eq!(
        node(&session, london)?.properties["accessibility"].value,
        json!("Catalog-specific text")
    );
    assert_eq!(node(&session, bristol)?, &previous_bristol);

    session.receive(
        serde_json::from_value(json!({
            "version": "1.0",
            "updateComponents": {"surfaceId": SURFACE, "components": [{
                "id": "status", "component": "Text", "text": "Shipment update",
                "accessibility": "Catalog-specific text"
            }]}
        }))?,
        now,
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert_eq!(update.nodes.len(), 2);
    assert_eq!(rows(&session)?, [london, bristol]);
    for id in [london, bristol] {
        assert!(
            !node(&session, id)?
                .properties
                .contains_key("@accessibility")
        );
        assert_eq!(
            node(&session, id)?.properties["accessibility"].value,
            json!("Catalog-specific text")
        );
        assert!(update.nodes.contains(&NodeChange {
            surface_id: SURFACE.to_owned(),
            id,
            kind: NodeChangeKind::Updated,
        }));
    }
    assert!(session.advance(now, None).nodes.is_empty());
    Ok(())
}

fn session() -> Result<(RoutingSession<'static>, Instant), Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    catalog["components"]["Text"]["properties"]["accessibility"] = json!({"type": "string"});
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    session.mount_tree(SURFACE);
    let now = Instant::now();
    session.receive(
        serde_json::from_str(include_str!(
            "../../../fixtures/local/node-accessibility.json"
        ))?,
        now,
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    Ok((session, now))
}

fn rows(session: &RoutingSession<'_>) -> Result<[NodeId; 2], Box<dyn Error>> {
    let tree = session.node_tree(SURFACE).ok_or("missing tree")?;
    let root = tree
        .node(tree.root().ok_or("missing root")?)
        .ok_or("missing root node")?;
    let [first, second] = root.children.as_slice() else {
        return Err("expected two shipments".into());
    };
    Ok([first.id, second.id])
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
