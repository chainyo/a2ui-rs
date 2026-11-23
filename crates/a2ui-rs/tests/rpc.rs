//! Tests RPC state and transport integration through the public API.

use std::error::Error;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{AgentToRendererMessage, CallAgentFunction, RendererToAgentMessage};
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::rpc::{CallOptions, RpcError, RpcHandler};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    catalog: Value,
    surfaces: [AgentToRendererMessage; 2],
    request: CallAgentFunction,
    remote_confirmation: Value,
}

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn setup(now: Instant) -> Result<(RpcHandler, Fixture), Box<dyn Error>> {
    let common = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/upstream/specification/v1_0/json/common_types.json");
    let common = serde_json::from_slice(&fs::read(common)?)?;
    let fixture: Fixture =
        serde_json::from_str(include_str!("../../../fixtures/local/rpc-session.json"))?;
    let mut handler = RpcHandler::default();
    handler.add_catalog(Catalog::from_json(fixture.catalog.clone(), &common)?)?;
    for create in fixture.surfaces.clone() {
        assert!(
            handler
                .receive(create, now, &LocalFunctions::default())?
                .is_none()
        );
    }
    Ok((handler, fixture))
}

#[test]
fn settles_concurrent_calls_and_cancels_only_the_deleted_surface() -> Result<(), Box<dyn Error>> {
    let now = Instant::now();
    let (mut handler, fixture) = setup(now)?;
    let functions = LocalFunctions::default();
    let mut outbox = Vec::new();
    let mut send = |message: RendererToAgentMessage| {
        outbox.push(message);
        Ok(())
    };
    let mut north = handler.call_agent_function(
        fixture.request.clone(),
        CallOptions::default(),
        now,
        &functions,
        Some(&mut send),
    )?;
    let south = CallAgentFunction {
        surface_id: "warehouse-south".to_owned(),
        function_call_id: "south-stock-1".to_owned(),
        ..fixture.request.clone()
    };
    let mut south = handler.call_agent_function(
        south,
        CallOptions::default(),
        now,
        &functions,
        Some(&mut send),
    )?;

    let counter = Arc::new(WakeCounter::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut context = Context::from_waker(&waker);
    assert!(Pin::new(&mut north).poll(&mut context).is_pending());
    handler.receive_json(
        &json!({"version": "1.0", "deleteSurface": {"surfaceId": "warehouse-north"}}),
        now,
        &functions,
    )?;
    assert!(matches!(
        Pin::new(&mut north).poll(&mut context),
        Poll::Ready(Err(RpcError::Cancelled))
    ));
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert!(south.try_result().is_none());

    let response = json!({"version": "1.0", "agentFunctionResponse": {
        "functionCallId": "south-stock-1", "error": {"code": "OUT_OF_STOCK", "message": "No units remain"}
    }});
    assert!(handler.receive_json(&response, now, &functions)?.is_none());
    assert!(
        matches!(south.try_result(), Some(Err(RpcError::Agent { code, message })) if code == "OUT_OF_STOCK" && message == "No units remain")
    );
    assert!(handler.receive_json(&response, now, &functions)?.is_none());
    assert!(south.try_result().is_none());
    assert!(handler.next_deadline().is_none());

    assert_eq!(
        serde_json::to_value(&outbox)?,
        json!([
            {"version": "1.0", "callAgentFunction": {"surfaceId": "warehouse-north",
                "functionCallId": "north-stock-1",
                "callFunction": {"@call": "queryStock", "catalogId": "example:warehouse",
                    "args": {"sku": "ITEM-19", "options": {"limit": 3}}}}},
            {"version": "1.0", "callAgentFunction": {"surfaceId": "warehouse-south",
                "functionCallId": "south-stock-1",
                "callFunction": {"@call": "queryStock", "catalogId": "example:warehouse",
                    "args": {"sku": "ITEM-47", "options": {"limit": 3}}}}}
        ])
    );
    Ok(())
}

#[test]
fn releases_dropped_calls_and_enforces_activation_at_dispatch() -> Result<(), Box<dyn Error>> {
    let now = Instant::now();
    let (mut handler, fixture) = setup(now)?;
    let functions = LocalFunctions::default();
    let mut failed_delivery = |_| Err("offline".to_owned());
    assert!(matches!(
        handler.call_agent_function(
            fixture.request.clone(),
            CallOptions::default(),
            now,
            &functions,
            Some(&mut failed_delivery)
        ),
        Err(RpcError::Delivery(_))
    ));
    assert!(handler.next_deadline().is_none());

    let confirmation = serde_json::from_value::<CallAgentFunction>(json!({
        "surfaceId": "warehouse-north", "functionCallId": "confirm-2",
        "callFunction": {"@call": "confirmRestock"}
    }))?;
    let mut outbox = Vec::new();
    let mut send = |message| {
        outbox.push(message);
        Ok(())
    };
    assert!(matches!(
        handler.call_agent_function(confirmation.clone(), CallOptions::default(), now, &functions, Some(&mut send)),
        Err(RpcError::InvalidCall(message)) if message.contains("user activation")
    ));
    let remote = handler
        .receive_json(&fixture.remote_confirmation, now, &functions)?
        .ok_or("missing refusal")?;
    assert_eq!(
        serde_json::to_value(remote)?["rendererFunctionResponse"]["error"]["code"],
        "INVALID_FUNCTION_CALL"
    );
    let timeout = Duration::from_millis(50);
    let mut handle = handler.call_agent_function(
        confirmation,
        CallOptions {
            timeout: Some(timeout),
            user_activated: true,
        },
        now,
        &functions,
        Some(&mut send),
    )?;
    assert_eq!(handler.next_deadline(), Some(now + timeout));
    let response = json!({"version": "1.0", "agentFunctionResponse": {
        "functionCallId": "confirm-2", "value": null
    }});
    handler.receive_json(&response, now + timeout, &functions)?;
    assert!(matches!(handle.try_result(), Some(Err(RpcError::Timeout))));

    let request = CallAgentFunction {
        function_call_id: "drop-1".to_owned(),
        ..fixture.request
    };
    let pending = handler.call_agent_function(
        request.clone(),
        CallOptions::default(),
        now + timeout,
        &functions,
        Some(&mut send),
    )?;
    drop(pending);
    assert!(handler.next_deadline().is_none());
    assert!(matches!(
        handler.call_agent_function(
            request,
            CallOptions::default(),
            now + timeout,
            &functions,
            Some(&mut send)
        ),
        Err(RpcError::Duplicate(_))
    ));
    assert_eq!(outbox.len(), 2);
    Ok(())
}
