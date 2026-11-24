//! Tests mounted trees with explicit component children.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::AgentToRendererMessage;
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{NodeChangeKind, NodeId, RoutingSession, RoutingUpdate};
use a2ui_rs::rpc::{ReceiveError, RpcHandler};
use a2ui_rs::validation::{MessageProcessor, ValidationConfig, ValidationError};
use serde_json::{Value, json};

#[test]
fn mixes_catalogs_available_when_the_surface_was_created() -> Result<(), Box<dyn Error>> {
    let mut session = session(false)?;
    let now = Instant::now();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/local/surface-catalog-availability.json"
    ))?;
    session.mount_tree("shipment");
    receive(
        &mut session,
        now,
        serde_json::from_value(fixture["create"].clone())?,
    )?;
    let earlier_layout = layout(&session)?;
    assert_eq!(earlier_layout.1.len(), 1);

    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    session.add_catalog(Catalog::from_json(fixture["catalog"].clone(), &common)?)?;
    let rejected = session.receive(serde_json::from_value(fixture["update"].clone())?, now);
    assert!(matches!(
        rejected,
        Err(ReceiveError::Validation(ValidationError::UnknownCatalog(id)))
            if id == "example/shipping"
    ));
    assert!(session.advance(now, None).nodes.is_empty());
    assert_eq!(layout(&session)?, earlier_layout);

    let mut create = fixture["create"].clone();
    create["createSurface"]["surfaceId"] = json!("later-shipment");
    let mut update = fixture["update"].clone();
    update["updateComponents"]["surfaceId"] = json!("later-shipment");
    session.mount_tree("later-shipment");
    receive(&mut session, now, serde_json::from_value(create)?)?;
    receive(&mut session, now, serde_json::from_value(update)?)?;
    let tree = session.node_tree("later-shipment").ok_or("missing tree")?;
    let root = tree
        .node(tree.root().ok_or("missing root ID")?)
        .ok_or("missing root")?;
    let [destination, badge] = root.children.as_slice() else {
        return Err("expected two mounted children".into());
    };
    let destination = tree.node(destination.id).ok_or("missing destination")?;
    let badge = tree.node(badge.id).ok_or("missing delivery badge")?;
    assert_eq!(destination.component, "Text");
    assert_eq!(destination.properties["text"].value, json!("London"));
    assert_eq!(badge.component, "DeliveryBadge");
    assert_eq!(badge.properties["label"].value, json!("Express delivery"));
    Ok(())
}

#[test]
fn retains_reordered_children_and_disposes_removed_instances() -> Result<(), Box<dyn Error>> {
    let mut session = session(false)?;
    let [create, reorder, remove, restore, _] = messages()?;
    let now = Instant::now();
    session.mount_tree("shipment");
    assert!(session.advance(now, None).nodes.is_empty());
    let mounted = receive(&mut session, now, create)?;
    let (root, children) = layout(&session)?;
    let [destination, delivery] = children.as_slice() else {
        return Err("expected two children".into());
    };
    assert_changes(
        &mounted,
        &[
            (root, NodeChangeKind::Mounted),
            (*destination, NodeChangeKind::Mounted),
            (*delivery, NodeChangeKind::Mounted),
        ],
    );

    let updated = receive(&mut session, now, reorder)?;
    assert_eq!(layout(&session)?, (root, vec![*delivery, *destination]));
    let tree = session.node_tree("shipment").ok_or("missing tree")?;
    let children = &tree.node(root).ok_or("missing root node")?.children;
    assert_eq!(children[0].field, "children[0]");
    assert_eq!(children[1].field, "children[1]");
    assert_eq!(
        tree.node(*delivery).ok_or("missing delivery")?.properties["text"].value,
        json!("Express delivery")
    );
    assert_changes(
        &updated,
        &[
            (root, NodeChangeKind::Updated),
            (*delivery, NodeChangeKind::Updated),
        ],
    );

    let updated = receive(&mut session, now, remove)?;
    assert_eq!(layout(&session)?, (root, vec![*destination]));
    assert!(
        session
            .node_tree("shipment")
            .and_then(|tree| tree.node(*delivery))
            .is_none()
    );
    assert_changes(
        &updated,
        &[
            (*delivery, NodeChangeKind::Disposed),
            (root, NodeChangeKind::Updated),
        ],
    );

    session.write_binding(
        "shipment",
        &DataScope::default(),
        "/destination",
        json!("Bristol"),
    )?;
    assert_changes(
        &session.advance(now, None),
        &[(*destination, NodeChangeKind::Updated)],
    );
    assert!(session.advance(now, None).nodes.is_empty());

    let updated = receive(&mut session, now, restore)?;
    let (_, new_children) = layout(&session)?;
    let [restored, retained] = new_children.as_slice() else {
        return Err("expected restored child".into());
    };
    assert_eq!(retained, destination);
    assert_ne!(restored, delivery);
    assert_changes(
        &updated,
        &[
            (root, NodeChangeKind::Updated),
            (*restored, NodeChangeKind::Mounted),
        ],
    );

    Ok(())
}

#[test]
fn recreating_a_surface_disposes_the_old_tree_once() -> Result<(), Box<dyn Error>> {
    let mut session = session(false)?;
    let [create, _, _, _, delete] = messages()?;
    let now = Instant::now();
    session.mount_tree("shipment");
    receive(&mut session, now, create.clone())?;
    let (root, children) = layout(&session)?;
    let [destination, delivery] = children.as_slice() else {
        return Err("expected two initial children".into());
    };
    session.receive(delete, now)?;
    session.receive(create, now)?;
    let updated = session.advance(now, None);
    let (new_root, new_children) = layout(&session)?;
    assert_ne!(root, new_root);
    let [new_destination, new_delivery] = new_children.as_slice() else {
        return Err("expected recreated children".into());
    };
    assert_changes(
        &updated,
        &[
            (*delivery, NodeChangeKind::Disposed),
            (*destination, NodeChangeKind::Disposed),
            (root, NodeChangeKind::Disposed),
            (new_root, NodeChangeKind::Mounted),
            (*new_destination, NodeChangeKind::Mounted),
            (*new_delivery, NodeChangeKind::Mounted),
        ],
    );
    session.dispose();
    assert!(session.node_tree("shipment").is_none());
    assert_changes(
        &session.advance(now, None),
        &[
            (*new_delivery, NodeChangeKind::Disposed),
            (*new_destination, NodeChangeKind::Disposed),
            (new_root, NodeChangeKind::Disposed),
        ],
    );
    assert!(session.advance(now, None).nodes.is_empty());
    Ok(())
}

#[test]
fn removing_a_child_cancels_its_pending_property_call() -> Result<(), Box<dyn Error>> {
    let mut session = session(true)?;
    let now = Instant::now();
    let mut messages: Vec<Value> = serde_json::from_str(include_str!(
        "../../../fixtures/local/reactive-node-tree.json"
    ))?;
    messages[0]["createSurface"]["components"][2]["text"] = json!({"@call": "lookupDeliveryTime"});

    session.mount_tree("shipment");
    session.receive(serde_json::from_value(messages[0].clone())?, now)?;
    let mut outbound = Vec::new();
    let update = session.advance(
        now,
        Some(&mut |message| {
            outbound.push(message);
            Ok(())
        }),
    );
    assert!(update.errors.is_empty());
    assert_eq!(outbound.len(), 1);
    assert!(session.next_deadline().is_some());
    let (root, children) = layout(&session)?;
    let delivery = children[1];

    let updated = receive(
        &mut session,
        now,
        serde_json::from_value(messages[2].clone())?,
    )?;
    assert_changes(
        &updated,
        &[
            (delivery, NodeChangeKind::Disposed),
            (root, NodeChangeKind::Updated),
        ],
    );
    assert!(session.next_deadline().is_none());
    assert!(session.advance(now, None).nodes.is_empty());
    Ok(())
}

fn messages() -> Result<[AgentToRendererMessage; 5], serde_json::Error> {
    serde_json::from_str(include_str!(
        "../../../fixtures/local/reactive-node-tree.json"
    ))
}

fn session(allow_unknown_elements: bool) -> Result<RoutingSession<'static>, Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let catalog =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let config = ValidationConfig {
        allow_orphan_components: true,
        allow_unknown_elements,
        ..ValidationConfig::default()
    };
    let mut session = RoutingSession::new(
        RpcHandler::new(MessageProcessor::new(config)),
        LocalFunctions::default(),
    );
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    Ok(session)
}

fn receive(
    session: &mut RoutingSession<'_>,
    now: Instant,
    message: AgentToRendererMessage,
) -> Result<RoutingUpdate, Box<dyn Error>> {
    assert!(session.receive(message, now)?.is_none());
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert!(update.changed.is_empty());
    Ok(update)
}

fn layout(session: &RoutingSession<'_>) -> Result<(NodeId, Vec<NodeId>), Box<dyn Error>> {
    let tree = session.node_tree("shipment").ok_or("missing tree")?;
    let root = tree.root().ok_or("missing root")?;
    let node = tree.node(root).ok_or("missing root node")?;
    Ok((root, node.children.iter().map(|child| child.id).collect()))
}

fn assert_changes(update: &RoutingUpdate, expected: &[(NodeId, NodeChangeKind)]) {
    let changes: Vec<_> = update
        .nodes
        .iter()
        .map(|change| {
            assert_eq!(change.surface_id, "shipment");
            (change.id, change.kind)
        })
        .collect();
    assert_eq!(changes, expected);
}
