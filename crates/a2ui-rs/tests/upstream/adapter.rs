use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{NodeChangeKind, NodeTree, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use a2ui_rs::validation::{MessageProcessor, ValidationConfig};
use serde_json::Value;

use super::{Case, TestError};

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    suite == "core/adapter_conformance.yaml"
        && case.protocol_version.as_deref() == Some("1.0")
        && case.action == "render_surface"
        && case.name == "progressive_arrival_placeholder_upgrade_in_place"
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let [initial, arrival] = case.steps.as_slice() else {
        return Err("expected two streaming steps".into());
    };
    let common = read_json(upstream, "specification/v1_0/json/common_types.json")?;
    let mut session = RoutingSession::new(
        RpcHandler::new(MessageProcessor::new(ValidationConfig::RELAXED)),
        LocalFunctions::default(),
    );
    for file in &case.catalog_paths {
        session.add_catalog(Catalog::from_json(read_json(upstream, file)?, &common)?)?;
    }
    let surface = initial["messages"][0]["createSurface"]["surfaceId"]
        .as_str()
        .ok_or("missing streaming surface ID")?;
    session.mount_tree(surface);
    let now = Instant::now();
    apply(&mut session, initial, now)?;
    let first = session.advance(now, None);
    let tree = session.node_tree(surface).ok_or("missing streaming tree")?;
    let (before_root, before_child) = inspect(tree, &initial["expectViewTree"])?;
    if !first.errors.is_empty()
        || !first
            .nodes
            .iter()
            .any(|change| change.id == before_child && change.kind == NodeChangeKind::Mounted)
    {
        return Err("expected a pending child mount".into());
    }

    apply(&mut session, arrival, now)?;
    let second = session.advance(now, None);
    let tree = session.node_tree(surface).ok_or("missing upgraded tree")?;
    let (after_root, after_child) = inspect(tree, &arrival["expectViewTree"])?;
    if arrival["expectSameViewIdentity"][0] != initial["expectViewTree"]["id"]
        || (before_root, before_child) != (after_root, after_child)
        || !second.errors.is_empty()
        || second.nodes.len() != 1
        || second.nodes[0].id != after_child
        || second.nodes[0].kind != NodeChangeKind::Updated
    {
        return Err("expected the child to resolve without remounting the tree".into());
    }
    Ok(())
}

fn inspect(
    tree: &NodeTree,
    expected: &Value,
) -> Result<(a2ui_rs::routing::NodeId, a2ui_rs::routing::NodeId), TestError> {
    let root = tree
        .root()
        .and_then(|id| tree.node(id))
        .ok_or("missing root")?;
    let [child] = root.children.as_slice() else {
        return Err("expected one child".into());
    };
    let child = tree.node(child.id).ok_or("missing child")?;
    let expected_child = &expected["children"][0];
    if expected["id"] != root.component_id
        || expected["type"] != root.component
        || expected_child["id"] != child.component_id
    {
        return Err(format!("unexpected streamed tree {root:?} {child:?}").into());
    }
    match expected_child["state"].as_str() {
        Some("pending") if child.is_pending() && child.properties.is_empty() => {}
        Some("resolved")
            if expected_child["type"] == child.component
                && child.properties.get("text").map(|property| &property.value)
                    == expected_child.get("text") => {}
        _ => return Err(format!("unexpected streamed child {child:?}").into()),
    }
    Ok((root.id, child.id))
}

fn apply(session: &mut RoutingSession<'_>, step: &Value, now: Instant) -> Result<(), TestError> {
    if step["op"] != "process_messages" {
        return Err("expected a process_messages step".into());
    }
    for message in step["messages"].as_array().ok_or("missing messages")? {
        if session
            .receive(serde_json::from_value(message.clone())?, now)?
            .is_some()
        {
            return Err("unexpected response for a surface message".into());
        }
    }
    Ok(())
}

fn read_json(upstream: &Path, file: &str) -> Result<Value, TestError> {
    Ok(serde_json::from_slice(&fs::read(upstream.join(file))?)?)
}
