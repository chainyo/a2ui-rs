//! Tests Basic Catalog validation through mounted nodes.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::basic_catalog::register_validation_functions;
use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{NodeChangeKind, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use serde_json::{Value, json};

#[test]
fn mounted_email_and_composed_checks_recover_after_a_write() -> Result<(), Box<dyn Error>> {
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
