use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use super::NodeId;
use crate::protocol::{FunctionCall, JsonObject, Metadata};
use crate::routing::{EvaluationId, EventAction, RoutingSession, Task};

/// The host options for a mounted action invocation.
pub struct ActionDispatchOptions {
    /// The timestamp to include in an event.
    pub timestamp: String,
    /// Whether the invocation comes from a user gesture.
    pub user_activated: bool,
    /// The metadata to include in an event.
    pub metadata: Option<Metadata>,
}

/// The reason a mounted node action could not be queued.
#[derive(Debug, Error)]
pub enum NodeActionError {
    /// The surface does not have a mounted node with this ID.
    #[error("Node {0:?} is not mounted on this surface")]
    UnknownNode(NodeId),
    /// The node property has no catalog action.
    #[error("Node property '{0}' is not an action")]
    NotAction(String),
    /// The action does not have a valid v1.0 payload.
    #[error("Invalid mounted action: {0}")]
    InvalidPayload(#[from] serde_json::Error),
}

impl RoutingSession<'_> {
    /// Queues a catalog action from a mounted node's property.
    ///
    /// Call `advance` to resolve the action in the node's data scope.
    /// An event includes the node's component ID and the supplied dispatch metadata.
    ///
    /// # Errors
    /// Returns an error if the node or action is absent or the action payload is invalid.
    pub fn dispatch_node_action(
        &mut self,
        surface: &str,
        id: NodeId,
        property: &str,
        options: ActionDispatchOptions,
    ) -> Result<EvaluationId, NodeActionError> {
        self.reconcile_tree(surface);
        let entry = self
            .trees
            .active
            .get(surface)
            .and_then(|tree| tree.entries.get(&id))
            .ok_or(NodeActionError::UnknownNode(id))?;
        let payload = entry
            .evaluations
            .get(property)
            .filter(|binding| binding.is_action)
            .and_then(|_| self.processor().get(surface))
            .and_then(|model| model.component(&entry.node.component_id))
            .and_then(|component| component.properties.get(property))
            .ok_or_else(|| NodeActionError::NotAction(property.to_owned()))?;

        let scope = entry.node.scope.clone();
        let action = serde_json::from_value::<NodeAction>(payload.clone())?;
        let task = match action {
            NodeAction::Event { event } => Task::Event {
                event: EventAction {
                    name: event.name,
                    source_component_id: entry.node.component_id.clone(),
                    timestamp: options.timestamp,
                    context: event.context.into_iter().collect(),
                    user_message: event.user_message,
                },
                metadata: options.metadata,
                user_activated: options.user_activated,
            },
            NodeAction::Function { function_call } => {
                return Ok(self.run_action(
                    surface,
                    scope,
                    serde_json::to_value(function_call)?,
                    options.user_activated,
                ));
            }
        };
        Ok(self.mount(surface, scope, task))
    }
}

#[derive(Deserialize)]
#[serde(untagged, rename_all_fields = "camelCase", deny_unknown_fields)]
enum NodeAction {
    Event { event: EventPayload },
    Function { function_call: FunctionCall },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventPayload {
    name: String,
    #[serde(default)]
    context: JsonObject,
    user_message: Option<Value>,
}
