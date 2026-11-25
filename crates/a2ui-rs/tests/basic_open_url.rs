//! Tests Basic Catalog URL actions from mounted nodes.

use std::cell::RefCell;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::basic_catalog::register_open_url;
use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{RendererToAgent, RendererToAgentMessage};
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::routing::{ActionDispatchOptions, EvaluationState, NodeTree, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use serde_json::{Value, json};

#[test]
fn mounted_open_url_calls_the_host_only_for_an_allowed_url() -> Result<(), Box<dyn Error>> {
    let opened = RefCell::new(Vec::new());
    let (mut session, _) = session(|url| {
        opened.borrow_mut().push(url.to_owned());
        Ok(())
    })?;
    let surface = "shipment";
    let now = Instant::now();
    session.mount_tree(surface);
    session.receive(
        serde_json::from_str(include_str!("../../../fixtures/local/basic-open-url.json"))?,
        now,
    )?;
    assert!(session.advance(now, None).errors.is_empty());
    let root = session
        .node_tree(surface)
        .and_then(NodeTree::root)
        .ok_or("missing root")?;
    let options = || ActionDispatchOptions {
        timestamp: "2026-01-01T00:00:00Z".into(),
        user_activated: true,
        metadata: None,
    };
    let mut outbound = Vec::new();
    let mut send = |message| {
        outbound.push(message);
        Ok(())
    };

    let allowed = session.dispatch_node_action(surface, root, "action", options())?;
    let update = session.advance(now, Some(&mut send));
    assert!(update.errors.is_empty(), "{:?}", update.errors);
    assert_eq!(session.value(allowed), Some(Value::Null));
    assert_eq!(*opened.borrow(), ["https://example.com/shipments/123"]);

    session.write_binding(
        surface,
        &DataScope::default(),
        "/trackingUrl",
        json!("file:///tmp/example.txt"),
    )?;
    let rejected = session.dispatch_node_action(surface, root, "action", options())?;
    let update = session.advance(now, Some(&mut send));
    assert_eq!(update.errors.len(), 1);
    assert!(matches!(
        session.status(rejected),
        Some(EvaluationState::Failed(error))
            if error.code == "EXECUTION_ERROR" && error.message.contains("URL scheme")
    ));
    assert!(session.advance(now, Some(&mut send)).errors.is_empty());
    assert_eq!(*opened.borrow(), ["https://example.com/shipments/123"]);
    assert!(outbound.is_empty(), "{outbound:?}");
    Ok(())
}

#[test]
fn open_url_cannot_use_activation_after_an_agent_response() -> Result<(), Box<dyn Error>> {
    let opened = RefCell::new(Vec::new());
    let (mut session, common) = session(|url| {
        opened.borrow_mut().push(url.to_owned());
        Ok(())
    })?;
    session.add_catalog(Catalog::from_json(
        json!({"protocolVersion": "1.0", "catalogId": "example:shipping"}),
        &common,
    )?)?;
    let now = Instant::now();
    session.receive(
        serde_json::from_str(include_str!("../../../fixtures/local/basic-open-url.json"))?,
        now,
    )?;
    let action = session.run_action(
        "shipment",
        DataScope::default(),
        json!({"@call": "openUrl", "args": {"url": {
            "@call": "lookupTrackingUrl", "catalogId": "example:shipping"
        }}}),
        true,
    );
    let mut outbound = Vec::new();
    let update = session.advance(
        now,
        Some(&mut |message| {
            outbound.push(message);
            Ok(())
        }),
    );
    assert!(update.errors.is_empty(), "{:?}", update.errors);
    assert_eq!(session.status(action), Some(&EvaluationState::Pending));
    let [RendererToAgentMessage::V1(RendererToAgent::CallAgentFunction(call))] =
        outbound.as_slice()
    else {
        return Err("expected one URL lookup".into());
    };
    session.receive(
        serde_json::from_value(json!({
            "version": "1.0",
            "agentFunctionResponse": {
                "functionCallId": call.function_call_id,
                "value": "https://example.com/shipments/123"
            }
        }))?,
        now,
    )?;
    let update = session.advance(now, None);
    assert_eq!(update.errors.len(), 1);
    assert!(matches!(
        session.status(action),
        Some(EvaluationState::Failed(error)) if error.message.contains("requires user activation")
    ));
    assert!(opened.borrow().is_empty());
    Ok(())
}

fn session<'a>(
    open: impl Fn(&str) -> Result<(), String> + 'a,
) -> Result<(RoutingSession<'a>, Value), Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let catalog =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let catalog = Catalog::from_json(catalog, &common)?;
    let mut functions = LocalFunctions::default();
    register_open_url(&mut functions, catalog.id(), open);
    let mut session = RoutingSession::new(RpcHandler::default(), functions);
    session.add_catalog(catalog)?;
    Ok((session, common))
}
