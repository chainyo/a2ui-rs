use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{NodeChange, NodeChangeKind, NodeId, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use serde_json::Value;

use super::{Case, TestError};

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    suite == "core/accessibility.yaml"
        && case.action == "accessibility_check"
        && case.protocol_version.as_deref() == Some("1.0")
        && matches!(
            case.name.as_str(),
            "test_accessibility_attributes_plumbing"
                | "test_accessibility_live_region_assertive"
                | "test_accessibility_dynamic_label_and_hidden_binding"
        )
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let catalog_path = case
        .catalog_path
        .as_ref()
        .ok_or("missing accessibility catalog")?;
    let catalog = serde_json::from_slice(&fs::read(upstream.join(catalog_path))?)?;
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;

    let messages = case.messages.as_ref().ok_or("missing messages")?;
    let surface = messages[0]["createSurface"]["surfaceId"]
        .as_str()
        .ok_or("missing surface ID")?;
    session.mount_tree(surface);
    let now = Instant::now();
    if case.steps.is_empty() {
        let expected = case.assertions.as_ref().ok_or("missing assertions")?;
        apply(&mut session, now, messages)?;
        assert_attributes(&session, surface, expected)?;
    } else {
        let mut previous = None;
        for step in &case.steps {
            let changes = apply(&mut session, now, &step["messages"])?;
            let current = assert_attributes(&session, surface, &step["assertions"])?;
            if let Some(previous) = previous {
                assert_updates(surface, &previous, &current, &changes)?;
            }
            previous = Some(current);
        }
    }
    Ok(())
}

fn apply(
    session: &mut RoutingSession<'_>,
    now: Instant,
    messages: &Value,
) -> Result<Vec<NodeChange>, TestError> {
    for message in messages
        .as_array()
        .ok_or("missing accessibility messages")?
    {
        if session
            .receive(serde_json::from_value(message.clone())?, now)?
            .is_some()
        {
            return Err("unexpected renderer response".into());
        }
    }
    let update = session.advance(now, None);
    if !update.errors.is_empty() {
        return Err(format!("unexpected expression errors: {:?}", update.errors).into());
    }
    Ok(update.nodes)
}

type Attributes = BTreeMap<String, (NodeId, Value)>;

fn assert_attributes(
    session: &RoutingSession<'_>,
    surface: &str,
    assertions: &Value,
) -> Result<Attributes, TestError> {
    let expected = assertions["accessibilityTree"]
        .as_object()
        .ok_or("missing accessibility assertions")?;
    let tree = session.node_tree(surface).ok_or("missing mounted tree")?;
    let mut pending = vec![tree.root().ok_or("missing root node")?];
    let mut observed = BTreeMap::new();
    while let Some(id) = pending.pop() {
        let node = tree.node(id).ok_or("missing mounted node")?;
        pending.extend(node.children.iter().map(|child| child.id));
        if let Some(expected) = expected.get(&node.component_id) {
            let actual = node
                .properties
                .get("@accessibility")
                .ok_or("missing accessibility property")?;
            if &actual.value != expected || actual.binding_path.is_some() || actual.is_action {
                return Err(format!(
                    "unexpected @accessibility on {}: {actual:?}",
                    node.component_id
                )
                .into());
            }
            if observed
                .insert(node.component_id.clone(), (id, actual.value.clone()))
                .is_some()
            {
                return Err("duplicate accessibility fixture component".into());
            }
        }
    }
    if observed.keys().ne(expected.keys()) {
        return Err("missing accessibility fixture components".into());
    }
    Ok(observed)
}

fn assert_updates(
    surface: &str,
    before: &Attributes,
    after: &Attributes,
    changes: &[NodeChange],
) -> Result<(), TestError> {
    let mut expected = Vec::new();
    for (component, (id, value)) in before {
        let (current_id, current_value) = after.get(component).ok_or("component disappeared")?;
        if current_id != id {
            return Err(format!("{component} changed node ID").into());
        }
        if value != current_value {
            expected.push(NodeChange {
                surface_id: surface.to_owned(),
                id: *id,
                kind: NodeChangeKind::Updated,
            });
        }
    }
    if expected.len() != changes.len() || !expected.iter().all(|change| changes.contains(change)) {
        return Err(format!("unexpected node updates: {changes:?}").into());
    }
    Ok(())
}
