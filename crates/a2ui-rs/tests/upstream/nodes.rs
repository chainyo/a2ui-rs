use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{RendererToAgent, RendererToAgentMessage};
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{
    ActionDispatchOptions, EvaluationState, MountedNode, NodeChangeKind, NodeId, NodeTree,
    RoutingSession,
};
use a2ui_rs::rpc::RpcHandler;
use a2ui_rs::validation::MessageProcessor;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Case, TestError};

#[derive(Deserialize)]
struct Fixture {
    catalog: PathBuf,
    #[serde(default = "initial_surface")]
    surface: bool,
    data: Value,
    components: Vec<Value>,
}

fn initial_surface() -> bool {
    true
}

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    suite == "core/node_resolution.yaml"
        && case.action == "resolve_nodes"
        && matches!(
            case.name.as_str(),
            "inline_create_applies_data_before_components"
                | "separate_data_message_keeps_arrival_order"
                | "direct_data_binding_and_function_call_schemas"
                | "event_property_is_not_an_action"
                | "invoke_action_with_dispatch_metadata"
        )
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let path = case.fixture.as_ref().ok_or("missing node fixture")?;
    let conformance = upstream.join("conformance");
    let mut fixture: Fixture =
        serde_saphyr::from_str(&fs::read_to_string(conformance.join(path))?)?;
    if !fixture.surface && (fixture.data != json!({}) || !fixture.components.is_empty()) {
        return Err("expected a fixture with no initial surface".into());
    }
    let catalog: Value = serde_json::from_slice(&fs::read(conformance.join(fixture.catalog))?)?;
    let common: Value = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    let catalog = Catalog::from_json(catalog, &common)?;
    let id = catalog.id().to_owned();
    session.add_catalog(catalog)?;
    session.mount_tree("s");
    let now = Instant::now();
    if fixture.surface {
        for component in &mut fixture.components {
            let object = component
                .as_object_mut()
                .ok_or("invalid component definition")?;
            if let Some(metadata) = object.remove("metadata") {
                object.insert("@metadata".to_owned(), metadata);
            }
        }
        session.receive(
            serde_json::from_value(json!({
                "version": "1.0", "createSurface": {
                    "surfaceId": "s", "catalogId": id,
                    "dataModel": fixture.data, "components": fixture.components,
                }
            }))?,
            now,
        )?;
    }
    let update = session.advance(now, None);
    if !update.errors.is_empty()
        || update
            .nodes
            .iter()
            .any(|change| change.kind != NodeChangeKind::Mounted)
    {
        return Err("unexpected initial node changes or expression errors".into());
    }

    assert_nodes(
        &session,
        &case.expect.as_ref().ok_or("missing initial expectation")?["nodes"],
    )?;

    for step in &case.steps {
        if step["op"] == "process_messages" {
            process_messages(&mut session, now, step)?;
        } else {
            scoped_step(&mut session, now, step)?;
        }
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
    assert_data(session, expected)
}

fn scoped_step(
    session: &mut RoutingSession<'_>,
    now: Instant,
    step: &Value,
) -> Result<(), TestError> {
    let before = node_ids(session)?;
    let invocation = apply_scoped_step(session, &before, step)?;
    let mut events = Vec::new();
    let update = session.advance(
        now,
        Some(&mut |message| {
            let RendererToAgentMessage::V1(RendererToAgent::Action(action)) = message else {
                return Err("expected an action event".to_owned());
            };
            let mut observed = json!({
                "name": action.name,
                "source_component_id": action.source_component_id,
                "context": action.context,
            });
            if let Some(metadata) = action.metadata {
                observed["metadata"] = json!(metadata);
            }
            events.push(observed);
            Ok(())
        }),
    );
    if invocation.is_some_and(|id| !matches!(session.status(id), Some(EvaluationState::Ready(_)))) {
        return Err(format!("action did not complete: {:?}", update.errors).into());
    }
    let expected = &step["expect"];
    if expected["events"].as_array().map_or(&[][..], Vec::as_slice) != events {
        return Err(format!("unexpected emitted events: {events:?}").into());
    }
    assert_nodes(session, &expected["nodes"])?;
    assert_data(session, expected)?;
    let after = node_ids(session)?;
    for path in expected["same_nodes"].as_array().into_iter().flatten() {
        let path = path.as_str().ok_or("invalid node path")?;
        if before
            .get(path)
            .zip(after.get(path))
            .is_none_or(|(old, new)| old != new)
        {
            return Err(format!("node {path} changed identity").into());
        }
    }

    let mut counts = BTreeMap::<_, u64>::new();
    for change in update.nodes {
        if change.kind != NodeChangeKind::Updated || change.surface_id != "s" {
            return Err(format!("unexpected node change: {change:?}").into());
        }
        let (path, _) = after
            .iter()
            .find(|(_, id)| **id == change.id)
            .ok_or("unknown changed node")?;
        *counts.entry(path).or_default() += 1;
    }
    if !update.errors.is_empty() || serde_json::to_value(counts)? != expected["emissions"] {
        return Err(format!(
            "unexpected emissions or expression errors: {:?}",
            update.errors
        )
        .into());
    }
    Ok(())
}

fn apply_scoped_step(
    session: &mut RoutingSession<'_>,
    before: &BTreeMap<String, NodeId>,
    step: &Value,
) -> Result<Option<a2ui_rs::routing::EvaluationId>, TestError> {
    match step["op"].as_str() {
        Some("write" | "invoke") => {
            let node = step["node"].as_str().ok_or("missing node path")?;
            let property = step["property"].as_str().ok_or("missing property")?;
            let id = *before.get(node).ok_or("unknown node")?;
            if step["op"] == "invoke" {
                let metadata = step
                    .get("metadata")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?;
                let options = ActionDispatchOptions {
                    timestamp: "2026-01-01T00:00:00Z".to_owned(),
                    user_activated: true,
                    metadata,
                };
                return Ok(Some(
                    session.dispatch_node_action("s", id, property, options)?,
                ));
            }
            let value = step.get("value").ok_or("missing value")?.clone();
            session.write_node_property("s", id, property, value)?;
        }
        Some("set_data") => {
            let path = step["path"].as_str().ok_or("missing data path")?;
            let value = step.get("value").ok_or("missing value")?.clone();
            session.write_binding("s", &DataScope::default(), path, value)?;
        }
        _ => return Err("unsupported node fixture operation".into()),
    }
    Ok(None)
}

fn assert_data(session: &RoutingSession<'_>, expected: &Value) -> Result<(), TestError> {
    let surface = session.processor().get("s").ok_or("missing surface")?;
    for (path, value) in expected["data"].as_object().into_iter().flatten() {
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
    let nodes = mounted_nodes(session, "s")?;
    let expected = expected.as_object().ok_or("missing expected nodes")?;
    if nodes.keys().ne(expected.keys()) {
        return Err(format!("unexpected mounted node paths: {:?}", nodes.keys()).into());
    }
    for (path, expected) in expected {
        let node = nodes[path];
        let fields = [
            ("component_id", node.component_id.as_str()),
            ("type", node.component.as_str()),
            ("state", "resolved"),
            ("data_path", node.scope.path()),
        ];
        for (field, actual) in fields {
            if expected.get(field).is_some_and(|value| value != actual) {
                return Err(format!("unexpected {field} at {path}: {actual}").into());
            }
        }
        for (name, expected) in expected["props"].as_object().into_iter().flatten() {
            let actual = observed_property(node, path, name, expected)?;
            if &actual != expected {
                return Err(format!("unexpected {name} at {path}: {actual}").into());
            }
        }
    }
    Ok(())
}

fn observed_property(
    node: &MountedNode,
    path: &str,
    name: &str,
    expected: &Value,
) -> Result<Value, TestError> {
    if let Some(child) = node.children.iter().find(|child| child.field == name) {
        return Ok(json!({"node": child_path(path, &child.field)}));
    }
    let children: Vec<_> = node
        .children
        .iter()
        .filter(|child| {
            child
                .field
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with('['))
        })
        .map(|child| json!({"node": child_path(path, &child.field)}))
        .collect();
    if !children.is_empty() {
        return Ok(Value::Array(children));
    }
    let property = node.properties.get(name).ok_or("missing property")?;
    if property.is_action {
        return Ok(json!({"action": true}));
    }
    if expected.get("writable").is_some() {
        Ok(json!({"value": property.value, "writable": property.binding_path.is_some()}))
    } else {
        Ok(property.value.clone())
    }
}

fn node_ids(session: &RoutingSession<'_>) -> Result<BTreeMap<String, NodeId>, TestError> {
    Ok(mounted_nodes(session, "s")?
        .into_iter()
        .map(|(path, node)| (path, node.id))
        .collect())
}

fn mounted_nodes<'a>(
    session: &'a RoutingSession<'_>,
    surface: &str,
) -> Result<BTreeMap<String, &'a MountedNode>, TestError> {
    let tree = session.node_tree(surface).ok_or("missing mounted tree")?;
    let mut nodes = BTreeMap::new();
    let mut pending: Vec<_> = tree
        .root()
        .into_iter()
        .map(|id| ("root".to_owned(), id))
        .collect();
    while let Some((path, id)) = pending.pop() {
        let node = tree.node(id).ok_or("missing mounted node")?;
        pending.extend(
            node.children
                .iter()
                .map(|child| (child_path(&path, &child.field), child.id)),
        );
        if nodes.insert(path, node).is_some() {
            return Err("duplicate node path".into());
        }
    }
    Ok(nodes)
}

pub(super) fn assert_mounted_indices(
    processor: MessageProcessor,
    expected: &Value,
) -> Result<(), TestError> {
    let surfaces = expected["surfaces"]
        .as_object()
        .ok_or("missing index surfaces")?;
    let mut session = RoutingSession::new(RpcHandler::new(processor), LocalFunctions::default());
    for surface in surfaces.keys() {
        session.mount_tree(surface);
    }
    let update = session.advance(Instant::now(), None);
    if !update.errors.is_empty() {
        return Err(format!("unexpected mounted index errors: {:?}", update.errors).into());
    }
    for (surface, expected) in surfaces {
        let nodes = mounted_nodes(&session, surface)?;
        for (key, properties) in expected["components"]
            .as_object()
            .ok_or("missing expected components")?
        {
            let node = nodes
                .values()
                .find(|node| {
                    node.scope
                        .index()
                        .is_some_and(|index| format!("{}_{index}", node.component_id) == *key)
                })
                .ok_or_else(|| format!("missing mounted component {key}"))?;
            for (name, value) in properties
                .as_object()
                .ok_or("missing expected properties")?
            {
                if node
                    .properties
                    .get(name)
                    .is_none_or(|property| &property.value != value)
                {
                    return Err(format!("unexpected mounted value for {key}.{name}").into());
                }
            }
        }
    }
    Ok(())
}

fn child_path(parent: &str, field: &str) -> String {
    format!(
        "{parent}/{}",
        field.replace(['[', '.'], "/").replace(']', "")
    )
}
