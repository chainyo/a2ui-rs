//! Drives a headless routing session with an in-memory transport.

use std::collections::VecDeque;
use std::error::Error;
use std::time::{Duration, Instant};

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::{
    AgentToRenderer, AgentToRendererMessage, CallAgentFunction, ErrorTarget, FunctionResponse,
    RendererError, RendererToAgent, RendererToAgentMessage, UpdateDataModel, ValidationCode,
};
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{ActionDispatchOptions, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use a2ui_rs::schema::common_types_schema;
use serde_json::Value;

type DemoError = Box<dyn Error>;

const SURFACE: &str = "shipment";
const RPC_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Transport {
    to_renderer: VecDeque<AgentToRendererMessage>,
    to_agent: VecDeque<RendererToAgentMessage>,
}

impl Transport {
    fn tick(&mut self, session: &mut RoutingSession<'_>, now: Instant) -> Result<(), DemoError> {
        while let Some(message) = self.to_renderer.pop_front() {
            println!("Agent to renderer: {}", serde_json::to_string(&message)?);
            match session.receive(message, now) {
                Ok(Some(response)) => self.to_agent.push_back(response),
                Ok(None) => {}
                Err(error) => {
                    // This example tracks one surface. Use the message's surface ID in a host.
                    let Some(response) = error.renderer_error_for_surface(SURFACE) else {
                        return Err(error.into());
                    };
                    self.to_agent.push_back(response);
                }
            }
        }

        let update = session.advance(
            now,
            Some(&mut |message| {
                self.to_agent.push_back(message);
                Ok(())
            }),
        );
        self.to_agent.extend(update.errors);
        Ok(())
    }

    fn read_for_agent(&mut self) -> Result<RendererToAgent, DemoError> {
        let message = self
            .to_agent
            .pop_front()
            .ok_or("The agent queue is empty")?;
        println!("Renderer to agent: {}", serde_json::to_string(&message)?);
        let RendererToAgentMessage::V1(operation) = message;
        Ok(operation)
    }

    fn read_estimate_call(&mut self, destination: &str) -> Result<CallAgentFunction, DemoError> {
        match self.read_for_agent()? {
            RendererToAgent::CallAgentFunction(call)
                if call.surface_id == SURFACE
                    && call.call_function.name == "estimateDelivery"
                    && call
                        .call_function
                        .args
                        .as_ref()
                        .and_then(|args| args.get("destination"))
                        .and_then(Value::as_str)
                        == Some(destination) =>
            {
                Ok(call)
            }
            operation => Err(format!(
                "Expected an estimate call for {destination}, got {operation:?}"
            )
            .into()),
        }
    }
}

fn main() -> Result<(), DemoError> {
    let mut transport = Transport::default();
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    let catalog = serde_json::from_str(include_str!("catalog.json"))?;
    session.add_catalog(Catalog::from_json(catalog, &common_types_schema())?)?;
    session.set_timeout(Some(RPC_TIMEOUT));
    session.mount_tree(SURFACE);

    let start = Instant::now();
    transport
        .to_renderer
        .push_back(serde_json::from_str(include_str!("surface.json"))?);
    transport.tick(&mut session, start)?;
    let call = transport.read_estimate_call("London")?;
    transport.to_renderer.push_back(AgentToRendererMessage::V1(
        AgentToRenderer::AgentFunctionResponse(FunctionResponse::Value {
            function_call_id: call.function_call_id,
            value: Value::from("Arrives Friday"),
        }),
    ));

    let now = start + Duration::from_secs(1);
    transport.tick(&mut session, now)?;
    let tree = session.node_tree(SURFACE).ok_or("Missing shipment tree")?;
    let root = tree.root().ok_or("Missing shipment root")?;
    let estimate = tree
        .node(root)
        .and_then(|node| node.properties.get("estimate"))
        .and_then(|property| property.value.as_str())
        .ok_or("Missing delivery estimate")?;
    if estimate != "Arrives Friday" || session.next_deadline().is_some() {
        return Err("The agent reply must resolve the estimate and clear its deadline".into());
    }
    println!("Mounted estimate: {estimate}");

    session.dispatch_node_action(
        SURFACE,
        root,
        "action",
        ActionDispatchOptions {
            timestamp: "2026-01-01T12:00:00Z".to_owned(),
            user_activated: true,
            metadata: None,
        },
    )?;
    transport.tick(&mut session, now)?;
    match transport.read_for_agent()? {
        RendererToAgent::Action(event)
            if event.name == "confirmDestination"
                && event.context.get("destination").and_then(Value::as_str) == Some("London") => {}
        operation => return Err(format!("Expected a destination action, got {operation:?}").into()),
    }

    forward_example_errors(&mut transport, &mut session, now)
}

fn update_destination(path: &str, destination: &str) -> AgentToRendererMessage {
    AgentToRendererMessage::V1(AgentToRenderer::UpdateDataModel(UpdateDataModel {
        surface_id: SURFACE.to_owned(),
        path: Some(path.to_owned()),
        value: Value::from(destination),
    }))
}

fn forward_example_errors(
    transport: &mut Transport,
    session: &mut RoutingSession<'_>,
    now: Instant,
) -> Result<(), DemoError> {
    transport
        .to_renderer
        .push_back(update_destination("/destination~2", "Bristol"));
    transport.tick(session, now)?;
    match transport.read_for_agent()? {
        RendererToAgent::Error(RendererError::Validation(error))
            if error.code == ValidationCode::ValidationFailed
                && error.surface_id == SURFACE
                && error.path == "/path" => {}
        operation => return Err(format!("Expected a path error, got {operation:?}").into()),
    }

    transport
        .to_renderer
        .push_back(update_destination("/destination", "Bristol"));
    transport.tick(session, now)?;
    let unanswered = transport.read_estimate_call("Bristol")?;
    let deadline = session.next_deadline().ok_or("Missing RPC deadline")?;
    if deadline != now + RPC_TIMEOUT {
        return Err("The deadline must use the configured timeout".into());
    }
    // An application sets a timer for this deadline.
    println!("Advance the clock to the pending RPC deadline.");
    transport.tick(session, deadline)?;
    match transport.read_for_agent()? {
        RendererToAgent::Error(RendererError::Generic(error))
            if error.code == "EXPRESSION_ERROR"
                && error.message == "The agent call timed out"
                && error.target == ErrorTarget::FunctionCallId(unanswered.function_call_id)
                && session.next_deadline().is_none() =>
        {
            Ok(())
        }
        operation => Err(format!("Expected a correlated timeout error, got {operation:?}").into()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn runs_the_transport_example() -> Result<(), super::DemoError> {
        super::main()
    }
}
