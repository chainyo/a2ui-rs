//! Runs a form through the headless SDK.

use std::error::Error;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{RendererToAgent, RendererToAgentMessage};
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{ActionDispatchOptions, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use a2ui_rs::schema::common_types_schema;
use serde_json::json;

const SURFACE: &str = "contact";

fn main() -> Result<(), Box<dyn Error>> {
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    let catalog = serde_json::from_str(include_str!("catalog.json"))?;
    session.add_catalog(Catalog::from_json(catalog, &common_types_schema())?)?;

    session.mount_tree(SURFACE);
    session.receive(
        serde_json::from_str(include_str!("surface.json"))?,
        Instant::now(),
    )?;
    let initial = session.advance(Instant::now(), None);
    if !initial.errors.is_empty() {
        return Err(format!("Initial evaluation failed: {:?}", initial.errors).into());
    }

    let tree = session
        .node_tree(SURFACE)
        .ok_or("The mounted tree is absent")?;
    let root = tree.root().ok_or("The tree has no root")?;
    let form = tree.node(root).ok_or("The root node is absent")?;
    let [field, submit] = form.children.as_slice() else {
        return Err("The form must have a field and a submit button".into());
    };
    let (field, submit) = (field.id, submit.id);
    println!(
        "Mounted {} with {} children",
        form.component,
        form.children.len()
    );
    let preview = form
        .properties
        .get("preview")
        .ok_or("The initial preview is absent")?;
    println!("Initial name: {}", preview.value);

    session.write_node_property(SURFACE, field, "value", json!("Riley"))?;
    session.dispatch_node_action(
        SURFACE,
        submit,
        "action",
        ActionDispatchOptions {
            timestamp: "2026-01-01T12:00:00Z".to_owned(),
            user_activated: true,
            metadata: None,
        },
    )?;

    let mut messages = Vec::new();
    let update = session.advance(
        Instant::now(),
        Some(&mut |message| {
            messages.push(message);
            Ok(())
        }),
    );
    if !update.errors.is_empty() {
        return Err(format!("Form update failed: {:?}", update.errors).into());
    }

    let preview = session
        .node_tree(SURFACE)
        .and_then(|tree| tree.node(root))
        .and_then(|node| node.properties.get("preview"))
        .ok_or("The updated preview is absent")?;
    println!("Updated name: {}", preview.value);
    let [event @ RendererToAgentMessage::V1(RendererToAgent::Action(_))] = messages.as_slice()
    else {
        return Err("Submitting the form must send one action".into());
    };
    println!("{}", serde_json::to_string(event)?);
    Ok(())
}
