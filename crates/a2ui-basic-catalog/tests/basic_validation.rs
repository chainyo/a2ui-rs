//! Tests Basic Catalog validation through mounted nodes.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_basic_catalog::register_validation_functions;
use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{NodeChange, NodeChangeKind, NodeId, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use serde_json::{Value, json};

#[test]
fn mounted_email_and_composed_checks_recover_after_a_write() -> Result<(), Box<dyn Error>> {
    let mut session = session()?;
    session.mount_tree("billing");
    let now = Instant::now();
    session.receive(
        serde_json::from_str(include_str!(
            "../../../fixtures/local/basic-validation.json"
        ))?,
        now,
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    let tree = session.node_tree("billing").ok_or("missing tree")?;
    let root = tree.root().ok_or("missing root")?;
    let node = tree.node(root).ok_or("missing node")?;
    let results = &node.properties["validationResults"].value;
    assert_eq!(node.properties["isValid"].value, Value::Bool(false));
    assert_eq!(results[0]["code"], "INVALID_EMAIL");
    assert_eq!(results[1]["message"], "Confirm a valid billing email");

    session.write_node_property(
        "billing",
        root,
        "value",
        json!("billing+eu@accounts.example.com"),
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert_eq!(update.nodes.len(), 1);
    assert_eq!(update.nodes[0].id, root);
    assert_eq!(update.nodes[0].kind, NodeChangeKind::Updated);
    let tree = session.node_tree("billing").ok_or("missing tree")?;
    let node = tree.node(root).ok_or("missing node")?;
    assert_eq!(node.properties["isValid"].value, Value::Bool(true));
    assert_eq!(node.properties["validationErrors"].value, json!([]));
    assert_eq!(node.properties["validationResults"].value, json!([]));
    Ok(())
}

#[test]
fn required_recovers_after_a_scoped_binding_write() -> Result<(), Box<dyn Error>> {
    let mut session = session()?;
    let surface = "deliveries";
    let now = Instant::now();
    session.mount_tree(surface);
    session.receive(
        serde_json::from_str(include_str!(
            "../../../fixtures/local/required-validation.json"
        ))?,
        now,
    )?;
    assert!(session.advance(now, None).errors.is_empty());

    let tree = session.node_tree(surface).ok_or("missing tree")?;
    let root = tree
        .root()
        .and_then(|id| tree.node(id))
        .ok_or("missing root")?;
    let [first, second] = root.children.as_slice() else {
        return Err("expected two deliveries".into());
    };
    let (first, second) = (first.id, second.id);
    let original_sibling = tree.node(second).ok_or("missing second delivery")?.clone();
    assert_eq!(
        original_sibling.properties["isValid"].value,
        Value::Bool(true)
    );
    assert_required_failure(&session, surface, first)?;

    session.write_node_property(surface, first, "value", json!("Sam"))?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert_eq!(
        update.nodes,
        [NodeChange {
            id: first,
            surface_id: surface.to_owned(),
            kind: NodeChangeKind::Updated,
        }]
    );
    let tree = session.node_tree(surface).ok_or("missing tree")?;
    let updated = tree.node(first).ok_or("missing first delivery")?;
    assert_eq!(updated.properties["value"].value, json!("Sam"));
    assert_eq!(updated.properties["isValid"].value, Value::Bool(true));
    assert_eq!(updated.properties["validationErrors"].value, json!([]));
    assert_eq!(updated.properties["validationResults"].value, json!([]));
    assert_eq!(tree.node(second), Some(&original_sibling));
    let data = session
        .processor()
        .get(surface)
        .ok_or("missing surface")?
        .data_model();
    assert_eq!(data.get("/deliveries/0/contact")?, Some(&json!("Sam")));

    for empty in [json!(""), json!([]), Value::Null] {
        session.write_node_property(surface, first, "value", empty)?;
        let update = session.advance(now, None);
        assert!(update.errors.is_empty());
        assert_required_failure(&session, surface, first)?;
    }
    Ok(())
}

fn session() -> Result<RoutingSession<'static>, Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let catalog =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let catalog = Catalog::from_json(catalog, &common)?;
    let mut functions = LocalFunctions::default();
    register_validation_functions(&mut functions, catalog.id());
    let mut session = RoutingSession::new(RpcHandler::default(), functions);
    session.add_catalog(catalog)?;
    Ok(session)
}

fn assert_required_failure(
    session: &RoutingSession<'_>,
    surface: &str,
    id: NodeId,
) -> Result<(), Box<dyn Error>> {
    let node = session
        .node_tree(surface)
        .and_then(|tree| tree.node(id))
        .ok_or("missing delivery")?;
    assert_eq!(node.properties["isValid"].value, Value::Bool(false));
    assert_eq!(
        node.properties["validationErrors"].value,
        json!(["Enter a delivery contact"])
    );
    assert_eq!(
        node.properties["validationResults"].value,
        json!([
            {"valid": false, "message": "Enter a delivery contact", "severity": "error"}
        ])
    );
    Ok(())
}

#[test]
fn bounds_and_regex_checks_recover_on_scoped_writes() -> Result<(), Box<dyn Error>> {
    let mut session = session()?;
    let now = Instant::now();
    let surface = "orders";
    session.mount_tree(surface);
    session.receive(
        serde_json::from_str(include_str!(
            "../../../fixtures/local/catalog-bounds-validation.json"
        ))?,
        now,
    )?;
    assert!(session.advance(now, None).errors.is_empty());
    let tree = session.node_tree(surface).ok_or("missing tree")?;
    let root = tree
        .root()
        .and_then(|id| tree.node(id))
        .ok_or("missing root")?;
    let [order, sibling] = root.children.as_slice() else {
        return Err("expected two orders".into());
    };
    let order = tree.node(order.id).ok_or("missing order")?;
    let [quantity, code] = order.children.as_slice() else {
        return Err("expected quantity and code".into());
    };
    let (quantity, code) = (quantity.id, code.id);
    let sibling = tree.node(sibling.id).ok_or("missing sibling")?.clone();
    assert_eq!(
        tree.node(quantity).ok_or("missing quantity")?.properties["validationErrors"].value,
        json!(["Use a quantity from 1 to 10"])
    );
    assert_eq!(
        tree.node(code).ok_or("missing code")?.properties["validationErrors"].value,
        json!([
            "Use 3 to 5 characters",
            "Use capital letters and a final digit"
        ])
    );

    session.write_node_property(surface, quantity, "value", json!("10"))?;
    session.write_node_property(surface, code, "value", json!("US7"))?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty());
    assert_eq!(update.nodes.len(), 2);
    let tree = session.node_tree(surface).ok_or("missing tree")?;
    for id in [quantity, code] {
        assert!(update.nodes.contains(&NodeChange {
            surface_id: surface.to_owned(),
            id,
            kind: NodeChangeKind::Updated
        }));
        let node = tree.node(id).ok_or("missing order field")?;
        assert_eq!(node.properties["isValid"].value, Value::Bool(true));
        assert_eq!(node.properties["validationResults"].value, json!([]));
    }
    assert_eq!(tree.node(sibling.id), Some(&sibling));
    Ok(())
}
