use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{NodeChangeKind, NodeId, NodeTree, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Case, TestError};

#[derive(Deserialize)]
struct Fixture {
    catalog: PathBuf,
    surface: bool,
    data: Value,
    components: Vec<Value>,
}

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    suite == "core/node_resolution.yaml"
        && case.action == "resolve_nodes"
        && matches!(
            case.name.as_str(),
            "inline_create_applies_data_before_components"
                | "separate_data_message_keeps_arrival_order"
        )
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let path = case.fixture.as_ref().ok_or("missing node fixture")?;
    let conformance = upstream.join("conformance");
    let fixture: Fixture = serde_saphyr::from_str(&fs::read_to_string(conformance.join(path))?)?;
    if fixture.surface || fixture.data != json!({}) || !fixture.components.is_empty() {
        return Err("expected a fixture with no initial surface".into());
    }
    let catalog: Value = serde_json::from_slice(&fs::read(conformance.join(fixture.catalog))?)?;
    let common: Value = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    session.mount_tree("s");
    let now = Instant::now();
    if !session.advance(now, None).nodes.is_empty() {
        return Err("the fixture mounted a node before a surface arrived".into());
    }
    assert_nodes(
        &session,
        &case.expect.as_ref().ok_or("missing initial expectation")?["nodes"],
    )?;

    for step in &case.steps {
        if step["op"] != "process_messages" {
            return Err("expected process_messages".into());
        }
        process_messages(&mut session, now, step)?;
    }
    Ok(())
}

fn process_messages(
    session: &mut RoutingSession<'_>,
    now: Instant,
    step: &Value,
) -> Result<(), TestError> {
    let initial = session.node_tree("s").and_then(NodeTree::root);
    let mut root = initial;
    let mut updates = 0;
    let mut mounts = 0;
    for message in step["messages"].as_array().ok_or("missing messages")? {
        if session
            .receive(serde_json::from_value(message.clone())?, now)?
            .is_some()
        {
            return Err("unexpected renderer response".into());
        }
        let update = session.advance(now, None);
        if !update.errors.is_empty() {
            return Err(format!("unexpected expression errors: {:?}", update.errors).into());
        }
        for change in update.nodes {
            if change.surface_id != "s" {
                return Err("node change used the wrong surface".into());
            }
            match change.kind {
                NodeChangeKind::Mounted if root.is_none() => {
                    mounts += 1;
                    root = Some(change.id);
                }
                NodeChangeKind::Updated if root == Some(change.id) => updates += 1,
                _ => return Err(format!("unexpected node change: {change:?}").into()),
            }
        }
    }

    let expected = &step["expect"];
    assert_nodes(session, &expected["nodes"])?;
    if root != session.node_tree("s").and_then(NodeTree::root) {
        return Err("the final node ID differs from the ID in its notification".into());
    }
    assert_emissions(expected, initial, mounts, updates)?;
    let surface = session.processor().get("s").ok_or("missing surface")?;
    for (path, value) in expected["data"]
        .as_object()
        .ok_or("missing expected data")?
    {
        if surface.data_model().get(path)? != Some(value) {
            return Err(format!("unexpected data at {path}").into());
        }
    }
    Ok(())
}

fn assert_emissions(
    expected: &Value,
    initial: Option<NodeId>,
    mounts: u64,
    updates: u64,
) -> Result<(), TestError> {
    let retained = if initial.is_some() && updates > 0 {
        json!({"root": updates})
    } else {
        json!({})
    };
    if retained != expected["emissions"]
        || (initial.is_none() && (mounts != 1 || expected["created_emissions"]["root"] != updates))
    {
        return Err(format!("unexpected mount and update counts: {mounts} and {updates}").into());
    }
    Ok(())
}

fn assert_nodes(session: &RoutingSession<'_>, expected: &Value) -> Result<(), TestError> {
    let tree = session.node_tree("s").ok_or("missing mounted tree")?;
    if expected == &json!({}) {
        return if tree.root().is_none() {
            Ok(())
        } else {
            Err("expected an empty tree".into())
        };
    }
    if expected.as_object().map_or(0, serde_json::Map::len) != 1 {
        return Err("expected only a root node in this fixture".into());
    }
    let node = tree
        .node(tree.root().ok_or("missing root")?)
        .ok_or("missing root node")?;
    let root = &expected["root"];
    if root["component_id"] != node.component_id
        || root["type"] != node.component
        || root["state"] != "resolved"
        || !node.children.is_empty()
    {
        return Err(format!("unexpected root: {node:?}").into());
    }
    for (name, expected) in root["props"]
        .as_object()
        .ok_or("missing expected properties")?
    {
        let property = node.properties.get(name).ok_or("missing root property")?;
        let actual = json!({"value": property.value, "writable": property.binding_path.is_some()});
        if &actual != expected {
            return Err(format!("unexpected {name}: {actual}").into());
        }
    }
    Ok(())
}
