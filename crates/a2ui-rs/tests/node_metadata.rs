//! Tests metadata and property notifications for collection nodes.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::AgentToRendererMessage;
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{NodeChange, NodeChangeKind, NodeId, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use serde_json::{Value, json};

const SURFACE: &str = "shipments";

#[test]
fn envelope_metadata_updates_each_instance_and_keeps_catalog_properties()
-> Result<(), Box<dyn Error>> {
    let mut session = session()?;
    let [create, metadata_update, property_update]: [AgentToRendererMessage; 3] =
        serde_json::from_str(include_str!("../../../fixtures/local/node-metadata.json"))?;
    let now = Instant::now();
    session.mount_tree(SURFACE);
    session.receive(create, now)?;
    let mounted = session.advance(now, None);
    assert!(mounted.errors.is_empty());
    assert_eq!(mounted.nodes.len(), 3);
    assert!(
        mounted
            .nodes
            .iter()
            .all(|change| change.kind == NodeChangeKind::Mounted)
    );
    let initial = nodes(&session)?;
    assert_rows(&session, &initial, "pending", "Standard delivery")?;

    session.receive(metadata_update, now)?;
    assert_changes(&mut session, now, &initial, NodeChangeKind::MetadataUpdated);
    assert_eq!(nodes(&session)?, initial);
    assert_rows(&session, &initial, "verified", "Standard delivery")?;

    session.receive(property_update, now)?;
    assert_changes(&mut session, now, &initial, NodeChangeKind::Updated);
    assert_eq!(nodes(&session)?, initial);
    assert_rows(&session, &initial, "verified", "Express delivery")?;
    assert!(session.advance(now, None).nodes.is_empty());
    Ok(())
}

fn session() -> Result<RoutingSession<'static>, Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common: Value = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    catalog["components"]["Text"]["properties"]["metadata"] = json!({"type": "string"});
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    Ok(session)
}

fn nodes(session: &RoutingSession<'_>) -> Result<Vec<NodeId>, Box<dyn Error>> {
    let tree = session.node_tree(SURFACE).ok_or("missing tree")?;
    let root = tree
        .node(tree.root().ok_or("missing root")?)
        .ok_or("missing root node")?;
    assert!(root.metadata.is_none());
    let children = root
        .children
        .iter()
        .map(|child| child.id)
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    Ok(children)
}

fn assert_rows(
    session: &RoutingSession<'_>,
    rows: &[NodeId],
    review: &str,
    delivery: &str,
) -> Result<(), Box<dyn Error>> {
    let tree = session.node_tree(SURFACE).ok_or("missing tree")?;
    for row in rows {
        let node = tree.node(*row).ok_or("missing row")?;
        assert_eq!(
            serde_json::to_value(&node.metadata)?,
            json!({"extensions": {"com_example_review": review}})
        );
        assert_eq!(node.properties["metadata"].value, json!(delivery));
        assert!(!node.properties.contains_key("@metadata"));
    }
    Ok(())
}

fn assert_changes(
    session: &mut RoutingSession<'_>,
    now: Instant,
    rows: &[NodeId],
    kind: NodeChangeKind,
) {
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert_eq!(update.nodes.len(), rows.len());
    for id in rows {
        assert!(update.nodes.contains(&NodeChange {
            surface_id: SURFACE.to_owned(),
            id: *id,
            kind
        }));
    }
}
