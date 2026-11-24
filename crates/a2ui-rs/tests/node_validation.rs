//! Tests scoped validation on mounted v1.0 nodes.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{
    MountedNode, NodeChange, NodeChangeKind, NodeId, NodeWriteError, RoutingSession,
};
use a2ui_rs::rpc::RpcHandler;
use serde_json::{Value, json};

const SURFACE: &str = "shipments";

#[test]
fn writable_binding_clears_the_check_for_its_item() -> Result<(), Box<dyn Error>> {
    let (mut session, now) = session()?;
    let [london, bristol] = rows(&session)?;
    let blocking = json!({"valid": false, "message": "Confirm this shipment", "severity": "error"});
    let advisory = json!({
        "valid": false, "code": "STANDARD_ONLY", "severity": "warning", "message": "Use standard delivery"
    });
    assert_eq!(
        &node(&session, london)?.properties["isValid"].value,
        &json!(false)
    );
    assert_eq!(
        &node(&session, london)?.properties["validationErrors"].value,
        &json!(["Confirm this shipment"])
    );
    assert_eq!(
        &node(&session, london)?.properties["validationResults"].value,
        &json!([blocking])
    );
    let previous_bristol = node(&session, bristol)?.clone();
    assert_eq!(
        previous_bristol.properties["validationResults"].value,
        json!([blocking, advisory])
    );

    assert!(matches!(
        session.write_node_property(SURFACE, london, "isValid", json!(true)),
        Err(NodeWriteError::ReadOnly(_))
    ));
    session.write_node_property(SURFACE, london, "value", json!(true))?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert!(update.changed.is_empty());
    assert_eq!(
        update.nodes,
        [NodeChange {
            surface_id: SURFACE.to_owned(),
            id: london,
            kind: NodeChangeKind::Updated
        }]
    );
    assert_eq!(rows(&session)?, [london, bristol]);
    let node = node(&session, london)?;
    assert_eq!(node.properties["value"].value, Value::Bool(true));
    assert_eq!(node.properties["isValid"].value, Value::Bool(true));
    assert_eq!(node.properties["validationErrors"].value, json!([]));
    assert_eq!(node.properties["validationResults"].value, json!([]));
    assert_eq!(
        session
            .processor()
            .get(SURFACE)
            .ok_or("missing surface")?
            .data_model()
            .get("/shipments/0/confirmed")?,
        Some(&Value::Bool(true))
    );
    assert_eq!(&previous_bristol, self::node(&session, bristol)?);
    assert!(session.advance(now, None).nodes.is_empty());
    Ok(())
}

#[test]
fn removing_checks_clears_their_derived_properties() -> Result<(), Box<dyn Error>> {
    let (mut session, now) = session()?;
    let original = rows(&session)?;
    let update = json!({"version": "1.0", "updateComponents": {
        "surfaceId": SURFACE,
        "components": [{
            "id": "confirmation", "component": "CheckBox",
            "label": {"@path": "name"}, "value": {"@path": "confirmed"}
        }]
    }});
    session.receive(serde_json::from_value(update)?, now)?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert_eq!(update.nodes.len(), original.len());
    assert_eq!(rows(&session)?, original);
    for id in original {
        assert!(update.nodes.contains(&NodeChange {
            surface_id: SURFACE.to_owned(),
            id,
            kind: NodeChangeKind::Updated
        }));
        for property in ["isValid", "validationErrors", "validationResults"] {
            assert!(!node(&session, id)?.properties.contains_key(property));
        }
    }
    assert!(session.advance(now, None).nodes.is_empty());
    Ok(())
}

fn session() -> Result<(RoutingSession<'static>, Instant), Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let catalog =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    session.mount_tree(SURFACE);
    let now = Instant::now();
    session.receive(
        serde_json::from_str(include_str!("../../../fixtures/local/node-validation.json"))?,
        now,
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert!(update.changed.is_empty());
    Ok((session, now))
}

fn rows(session: &RoutingSession<'_>) -> Result<[NodeId; 2], Box<dyn Error>> {
    let tree = session.node_tree(SURFACE).ok_or("missing tree")?;
    let root = tree
        .node(tree.root().ok_or("missing root")?)
        .ok_or("missing root node")?;
    let [london, bristol] = root.children.as_slice() else {
        return Err("expected two shipments".into());
    };
    Ok([london.id, bristol.id])
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
