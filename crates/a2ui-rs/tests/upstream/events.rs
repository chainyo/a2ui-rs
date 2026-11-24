use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::{Catalog, CatalogError};
use a2ui_rs::protocol::{AgentToRendererMessage, JsonObject, Metadata};
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{ActionDispatchOptions, EvaluationState, EventAction, RoutingSession};
use a2ui_rs::rpc::{ReceiveError, RpcHandler};
use a2ui_rs::validation::ValidationError;
use jiff::Timestamp;
use serde_json::{Value, json};

use super::{Case, TestError};

const TIMESTAMP: &str = "2026-01-01T00:00:00Z";

fn is_v1_action(suite: &str, case: &Case) -> bool {
    suite == "core/actions.yaml"
        && case.action == "dispatch_action"
        && case.protocol_version.as_deref() == Some("1.0")
}

pub(super) fn supports(suite: &str, case: &Case) -> bool {
    if is_v1_action(suite, case) {
        return matches!(
            case.name.as_str(),
            "test_action_event_missing_name_error"
                | "test_action_event_with_dispatch_metadata"
                | "test_action_event_without_dispatch_metadata_has_none"
                | "test_action_event_dispatch_metadata_invalid_uax31_key_error"
                | "test_action_v10_event_with_at_path_and_at_call_context"
                | "test_action_v10_event_context_literal_path_and_call_keys"
                | "test_action_event_non_string_user_message_error"
        );
    }
    suite == "core/actions.yaml"
        && case.action == "dispatch_action"
        && case.protocol_version.is_none()
        && matches!(
            case.name.as_str(),
            "test_action_event_basic"
                | "test_action_event_with_literal_context"
                | "test_action_event_context_literal_keys_path_and_call"
                | "test_action_event_with_data_binding"
                | "test_action_event_with_relative_scope"
                | "test_action_event_with_user_message"
        )
}

pub(super) fn execute(upstream: &Path, case: &Case) -> Result<(), TestError> {
    let surface = case.surface_id.as_deref().unwrap_or("actions");
    let source = case
        .source_component_id
        .as_deref()
        .unwrap_or(if case.scope.is_some() {
            "button"
        } else {
            "root"
        });
    let payload = case.action_payload.clone().ok_or("missing event payload")?;
    let payload = v1_payload(case, payload)?;
    let (components, index) = layout(source, case.scope.as_deref(), &payload)?;
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let document =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let catalog = Catalog::from_json(document, &common)?;
    let mut create = json!({
        "surfaceId": surface,
        "catalogId": catalog.id(),
        "dataModel": case.data_model.clone().unwrap_or_else(|| json!({})),
        "components": components
    });
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    session.add_catalog(catalog)?;
    session.mount_tree(surface);
    let now = Instant::now();
    let received = session.receive(create_message(&create)?, now);

    let missing_name = case.name == "test_action_event_missing_name_error";
    let invalid_message = case.name == "test_action_event_non_string_user_message_error";
    if missing_name || invalid_message {
        let expected = case
            .expect_error
            .as_ref()
            .ok_or("missing action error expectation")?;
        if !matches!(received, Err(ReceiveError::Validation(ValidationError::Catalog(
            CatalogError::InvalidPayload { ref name, .. }
        ))) if name == "Button")
            || case.expect_dispatched.is_some()
            || expected["surfaceId"] != surface
            || expected["code"] != "INVALID_ACTION"
            || session.processor().get(surface).is_some()
        {
            return Err(format!("unexpected result at catalog validation: {received:?}").into());
        }
        assert_no_event(&mut session, now)?;
        create
            .as_object_mut()
            .ok_or("missing create payload")?
            .remove("components");
        session.receive(create_message(&create)?, now)?;
        assert_invalid_event(&mut session, case, surface, source, &payload, now)?;
        return Ok(());
    }
    received?;
    if !session.advance(now, None).errors.is_empty() {
        return Err("unexpected errors at mount".into());
    }

    let metadata = case
        .metadata
        .clone()
        .map(serde_json::from_value::<Metadata>)
        .transpose();
    if let Some(expected) = &case.expect_error {
        if expected["category"] != "ValidationError"
            || case.expect_dispatched.is_some()
            || !matches!(&metadata, Err(error) if error.to_string().contains("invalid extension identifier"))
        {
            return Err(
                format!("unexpected dispatch metadata validation result: {metadata:?}").into(),
            );
        }
        return assert_no_event(&mut session, now);
    }

    dispatch_and_assert(&mut session, case, surface, source, index, metadata?, now)
}

fn dispatch_and_assert(
    session: &mut RoutingSession<'_>,
    case: &Case,
    surface: &str,
    source: &str,
    index: Option<usize>,
    metadata: Option<Metadata>,
    now: Instant,
) -> Result<(), TestError> {
    let tree = session.node_tree(surface).ok_or("missing mounted tree")?;
    let root = tree
        .root()
        .and_then(|id| tree.node(id))
        .ok_or("missing root node")?;
    let button = match index {
        Some(index) => root.children.get(index).ok_or("missing source button")?.id,
        None => root.id,
    };
    let evaluation = session.dispatch_node_action(
        surface,
        button,
        "action",
        ActionDispatchOptions {
            timestamp: TIMESTAMP.into(),
            user_activated: case.user_activation.unwrap_or(true),
            metadata,
        },
    )?;
    let mut messages = Vec::new();
    let update = session.advance(
        now,
        Some(&mut |message| {
            messages.push(message);
            Ok(())
        }),
    );
    let [message] = messages.as_slice() else {
        return Err(format!("expected one outbound message, got {messages:?}").into());
    };
    if !update.errors.is_empty() || session.value(evaluation) != Some(Value::Null) {
        return Err(format!("action evaluation did not complete: {:?}", update.errors).into());
    }
    let actual = serde_json::to_value(message)?;
    let action = actual["action"]
        .as_object()
        .ok_or("outbound message is not an action")?;
    if action.get("surfaceId") != Some(&json!(surface))
        || action.get("sourceComponentId") != Some(&json!(source))
        || action.get("timestamp") != Some(&json!(TIMESTAMP))
    {
        return Err(format!("unexpected action envelope: {actual}").into());
    }
    let expected = case
        .expect_dispatched
        .as_ref()
        .and_then(Value::as_object)
        .ok_or("missing action expectation")?;
    for (key, value) in expected {
        let matches = if key == "timestampIso8601" {
            let time = action["timestamp"].as_str().ok_or("missing timestamp")?;
            value.as_bool() == Some(time.parse::<Timestamp>().is_ok())
        } else {
            action.get(key).unwrap_or(&Value::Null) == value
        };
        if !matches {
            return Err(format!("expected action {key}={value}, got {actual}").into());
        }
    }
    if expected.get("metadata").is_some_and(Value::is_null) && action.contains_key("metadata") {
        return Err("outbound action must omit metadata".into());
    }
    assert_no_event(session, now)
}

fn create_message(create: &Value) -> Result<AgentToRendererMessage, TestError> {
    Ok(serde_json::from_value(
        json!({"version": "1.0", "createSurface": create}),
    )?)
}

fn assert_invalid_event(
    session: &mut RoutingSession<'_>,
    case: &Case,
    surface: &str,
    source: &str,
    payload: &Value,
    now: Instant,
) -> Result<(), TestError> {
    let event = payload["event"].as_object().ok_or("missing event")?;
    let context = event
        .get("context")
        .map(|context| serde_json::from_value::<JsonObject>(context.clone()))
        .transpose()?
        .unwrap_or_default();
    let evaluation = session.dispatch_event(
        surface,
        DataScope::default(),
        EventAction {
            name: event
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            source_component_id: source.to_owned(),
            timestamp: TIMESTAMP.to_owned(),
            context: context.into_iter().collect(),
            user_message: event.get("userMessage").cloned(),
        },
        case.user_activation.unwrap_or(true),
    );
    let mut outbound = false;
    let update = session.advance(
        now,
        Some(&mut |_| {
            outbound = true;
            Ok(())
        }),
    );
    let [error] = update.errors.as_slice() else {
        return Err("expected one event dispatch error".into());
    };
    let actual = serde_json::to_value(error)?;
    let expected = case.expect_error.as_ref().ok_or("missing expected error")?;
    if outbound
        || !matches!(session.status(evaluation), Some(EvaluationState::Failed(_)))
        || actual["error"]["code"] != expected["code"]
        || actual["error"]["surfaceId"] != expected["surfaceId"]
    {
        return Err(format!("unexpected event dispatch result: {actual}").into());
    }
    assert_no_event(session, now)
}

fn v1_payload(case: &Case, mut payload: Value) -> Result<Value, TestError> {
    if matches!(
        case.name.as_str(),
        "test_action_event_with_data_binding" | "test_action_event_with_relative_scope"
    ) {
        // The unversioned fixtures use the old binding key.
        let context = payload["event"]["context"]
            .as_object_mut()
            .ok_or("missing action context")?;
        for value in context.values_mut() {
            if let Some(path) = value.get("path") {
                *value = json!({"@path": path});
            }
        }
    }
    Ok(payload)
}

fn layout(
    source: &str,
    scope: Option<&str>,
    payload: &Value,
) -> Result<(Vec<Value>, Option<usize>), TestError> {
    let mut components = vec![
        json!({"id": source, "component": "Button", "child": "label", "action": payload}),
        json!({"id": "label", "component": "Text", "text": "Select"}),
    ];
    if let Some(scope) = scope {
        let (path, index) = scope
            .rsplit_once('/')
            .ok_or("expected collection item scope")?;
        components.push(json!({
            "id": "root", "component": "Column",
            "children": {"componentId": source, "path": path}
        }));
        Ok((components, Some(index.parse()?)))
    } else if source == "root" {
        Ok((components, None))
    } else {
        components.push(json!({"id": "root", "component": "Column", "children": [source]}));
        Ok((components, Some(0)))
    }
}

fn assert_no_event(session: &mut RoutingSession<'_>, now: Instant) -> Result<(), TestError> {
    let mut outbound = false;
    let update = session.advance(
        now,
        Some(&mut |_| {
            outbound = true;
            Ok(())
        }),
    );
    if outbound || !update.errors.is_empty() {
        return Err("unexpected action or error".into());
    }
    Ok(())
}
