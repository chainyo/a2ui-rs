//! Routes renderer expressions through local catalogs and agent RPC.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::catalog::Catalog;
use crate::protocol::{
    AgentToRenderer, AgentToRendererMessage, ErrorTarget, GenericError, JsonObject, RendererError,
    RendererToAgent, RendererToAgentMessage,
};
use crate::resolution::{DataScope, LocalFunctions, to_boolean};
use crate::rpc::{Outbound, PendingCall, ReceiveError, RpcHandler};

mod check;
mod evaluation;

pub use check::{ValidationResult, ValidationSeverity};
use evaluation::Tick;

const CACHE_LIMIT: usize = 256;

/// Identifies a mounted binding, check, or action.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EvaluationId(u64);

/// The state of a mounted expression.
#[derive(Clone, Debug, PartialEq)]
pub enum EvaluationState {
    /// The expression waits for the agent.
    Pending,
    /// The expression has a value.
    Ready(Value),
    /// The expression failed.
    Failed(RoutingError),
    /// The caller cancelled the expression.
    Cancelled,
}

/// A failure during expression or action evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingError {
    /// The renderer error code.
    pub code: &'static str,
    /// The error description.
    pub message: String,
    /// The correlated agent call, if the failure came from the agent.
    pub function_call_id: Option<String>,
}

impl RoutingError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            function_call_id: None,
        }
    }

    fn expression(error: impl std::fmt::Display) -> Self {
        Self::new("EXPRESSION_ERROR", error.to_string())
    }

    fn wire(self, surface: &str) -> RendererToAgentMessage {
        // The generic envelope has one target. Prefer the call ID for RPC errors.
        let target = self.function_call_id.map_or_else(
            || ErrorTarget::SurfaceId(surface.to_owned()),
            ErrorTarget::FunctionCallId,
        );
        RendererToAgentMessage::V1(RendererToAgent::Error(RendererError::Generic(
            GenericError {
                code: self.code.to_owned(),
                message: self.message,
                target,
                extra: BTreeMap::new(),
            },
        )))
    }
}

/// An event action and its unresolved expressions.
pub struct EventAction {
    /// The event name.
    pub name: String,
    /// The component that produced the event.
    pub source_component_id: String,
    /// The event timestamp.
    pub timestamp: String,
    /// Context entries in evaluation order.
    pub context: Vec<(String, Value)>,
    /// An optional expression for the user message.
    pub user_message: Option<Value>,
}

/// The changes and errors from one evaluation pass.
#[derive(Default)]
pub struct RoutingUpdate {
    /// The evaluations whose state changed.
    pub changed: Vec<EvaluationId>,
    /// Renderer errors to send to the agent.
    pub errors: Vec<RendererToAgentMessage>,
}

/// One renderer session with agent routing for mounted expressions.
pub struct RoutingSession<'f> {
    rpc: RpcHandler,
    functions: LocalFunctions<'f>,
    state: RouterState,
    evaluations: BTreeMap<EvaluationId, Evaluation>,
    next_id: u64,
    timeout: Option<Duration>,
}

impl<'f> RoutingSession<'f> {
    /// Creates a session with the supplied RPC handler and local functions.
    #[must_use]
    pub fn new(rpc: RpcHandler, functions: LocalFunctions<'f>) -> Self {
        Self {
            rpc,
            functions,
            state: RouterState::default(),
            evaluations: BTreeMap::new(),
            next_id: 0,
            timeout: None,
        }
    }

    /// Sets the timeout for new routed calls. An absent value uses the RPC default.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// Returns the processor for surface queries.
    #[must_use]
    pub fn processor(&self) -> &crate::validation::MessageProcessor {
        self.rpc.processor()
    }

    /// Registers a catalog.
    ///
    /// # Errors
    /// Returns an error if the handler rejects the catalog.
    pub fn add_catalog(&mut self, catalog: Catalog) -> Result<(), ReceiveError> {
        self.rpc.add_catalog(catalog)
    }

    /// Applies an incoming message. Call `advance` to publish changes to expressions.
    ///
    /// # Errors
    /// Returns an error if the handler rejects the message.
    pub fn receive(
        &mut self,
        message: AgentToRendererMessage,
        now: Instant,
    ) -> Result<Option<RendererToAgentMessage>, ReceiveError> {
        let AgentToRendererMessage::V1(operation) = &message;
        let deleted = match operation {
            AgentToRenderer::DeleteSurface(delete) => Some(delete.surface_id.clone()),
            _ => None,
        };
        let modified = match operation {
            AgentToRenderer::UpdateDataModel(update) => Some(update.surface_id.clone()),
            _ => None,
        };
        let response = self.rpc.receive(message, now, &self.functions)?;
        if let Some(surface) = deleted {
            self.state.surfaces.remove(&surface);
            self.evaluations.retain(|_, entry| entry.surface != surface);
        }
        if let Some(surface) = modified {
            self.state.bump(&surface);
        }
        Ok(response)
    }

    /// Writes data from a renderer input.
    ///
    /// # Errors
    /// Returns an error if the handler rejects the write.
    pub fn write_binding(
        &mut self,
        surface: &str,
        scope: &DataScope,
        path: &str,
        value: Value,
    ) -> Result<(), ReceiveError> {
        self.rpc.write_binding(surface, scope, path, value)?;
        self.state.bump(surface);
        Ok(())
    }

    /// Mounts a reactive binding.
    pub fn mount_binding(
        &mut self,
        surface: &str,
        scope: DataScope,
        expression: Value,
    ) -> EvaluationId {
        self.mount(surface, scope, Task::Binding(expression))
    }

    /// Mounts a property of a component stored on the surface.
    ///
    /// Later component updates replace the expression used for this property.
    pub fn mount_property(
        &mut self,
        surface: &str,
        scope: DataScope,
        component: &str,
        property: &str,
    ) -> EvaluationId {
        self.mount(
            surface,
            scope,
            Task::Property {
                component: component.to_owned(),
                property: property.to_owned(),
            },
        )
    }

    /// Mounts a reactive check condition.
    pub fn mount_check(
        &mut self,
        surface: &str,
        scope: DataScope,
        condition: Value,
    ) -> EvaluationId {
        self.mount(surface, scope, Task::Check(condition))
    }

    /// Mounts one component check rule by its index in the component's check list.
    ///
    /// Use the data scope of the component instance. Later component updates replace the rule.
    pub fn mount_check_rule(
        &mut self,
        surface: &str,
        scope: DataScope,
        component: &str,
        index: usize,
    ) -> EvaluationId {
        self.mount(
            surface,
            scope,
            Task::CheckRule {
                component: component.to_owned(),
                index,
            },
        )
    }

    /// Queues one function action.
    pub fn run_action(
        &mut self,
        surface: &str,
        scope: DataScope,
        call: Value,
        user_activated: bool,
    ) -> EvaluationId {
        self.mount(
            surface,
            scope,
            Task::FunctionAction {
                call,
                user_activated,
            },
        )
    }

    /// Queues one event action with ordered context entries.
    pub fn dispatch_event(
        &mut self,
        surface: &str,
        scope: DataScope,
        event: EventAction,
        user_activated: bool,
    ) -> EvaluationId {
        self.mount(
            surface,
            scope,
            Task::Event {
                event,
                user_activated,
            },
        )
    }

    fn mount(&mut self, surface: &str, scope: DataScope, task: Task) -> EvaluationId {
        self.next_id += 1;
        let id = EvaluationId(self.next_id);
        self.evaluations.insert(
            id,
            Evaluation {
                surface: surface.to_owned(),
                scope,
                task,
                outcome: EvaluationState::Pending,
                validation_result: None,
                reported: false,
                slots: BTreeMap::new(),
                diagnostics: BTreeMap::new(),
                event: EventProgress::default(),
            },
        );
        id
    }

    /// Removes an evaluation. Its unshared pending calls are dropped.
    pub fn unmount(&mut self, id: EvaluationId) {
        self.evaluations.remove(&id);
    }

    /// Returns the state of an evaluation.
    #[must_use]
    pub fn status(&self, id: EvaluationId) -> Option<&EvaluationState> {
        self.evaluations.get(&id).map(|entry| &entry.outcome)
    }

    /// Returns the result for a mounted check rule after evaluation.
    #[must_use]
    pub fn validation_result(&self, id: EvaluationId) -> Option<&ValidationResult> {
        self.evaluations.get(&id)?.validation_result.as_ref()
    }

    /// Returns the current value. A pending check passes until its answer arrives.
    #[must_use]
    pub fn value(&self, id: EvaluationId) -> Option<Value> {
        let entry = self.evaluations.get(&id)?;
        Some(match (&entry.task, &entry.outcome) {
            (Task::CheckRule { .. }, _) => Value::Bool(
                entry
                    .validation_result
                    .as_ref()
                    .is_none_or(|result| result.valid),
            ),
            (Task::Check(_), EvaluationState::Ready(value)) => Value::Bool(to_boolean(value)),
            (_, EvaluationState::Ready(value)) => value.clone(),
            (Task::Check(_), EvaluationState::Pending | EvaluationState::Cancelled) => {
                Value::Bool(true)
            }
            (Task::Check(_), EvaluationState::Failed(_)) => Value::Bool(false),
            _ => Value::Null,
        })
    }

    /// Returns the earliest pending RPC deadline.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.rpc.next_deadline()
    }

    /// Cancels pending calls for the surface.
    pub fn cancel_surface_calls(&mut self, surface: &str) {
        self.rpc.cancel_surface_calls(surface);
        if let Some(state) = self.state.surfaces.get_mut(surface) {
            state.shared.clear();
            for evaluation in self
                .evaluations
                .values_mut()
                .filter(|entry| entry.surface == surface)
            {
                for slot in evaluation.slots.values_mut().filter(|slot| slot.waiting()) {
                    slot.generation = state.generation;
                }
            }
        }
    }

    /// Closes the session and removes its evaluations and caches.
    pub fn dispose(&mut self) {
        self.rpc.dispose();
        self.evaluations.clear();
        self.state.surfaces.clear();
    }

    /// Resolves mounted expressions, sends new requests, and returns state changes.
    ///
    /// Call after input changes, incoming responses, or a pending deadline.
    pub fn advance(&mut self, now: Instant, outbound: Option<&mut Outbound<'_>>) -> RoutingUpdate {
        self.rpc.expire(now);
        let mut tick = Tick {
            rpc: &mut self.rpc,
            functions: &self.functions,
            state: &mut self.state,
            now,
            timeout: self.timeout,
            outbound,
        };
        let mut update = RoutingUpdate::default();
        let mut reported_calls = BTreeSet::new();
        for imperative in [true, false] {
            for (&id, evaluation) in self
                .evaluations
                .iter_mut()
                .filter(|(_, entry)| entry.task.imperative() == imperative)
            {
                if evaluation.task.imperative()
                    && evaluation.reported
                    && evaluation.outcome != EvaluationState::Pending
                {
                    continue;
                }
                let previous = evaluation.outcome.clone();
                let previous_check = evaluation.validation_result.take();
                let (outcome, diagnostics) = tick.evaluate(evaluation);
                let outcome_changed = !evaluation.reported || previous != outcome;
                if outcome_changed || previous_check != evaluation.validation_result {
                    update.changed.push(id);
                }
                if outcome_changed
                    && let EvaluationState::Failed(error) = &outcome
                    && error
                        .function_call_id
                        .as_ref()
                        .is_none_or(|id| reported_calls.insert(id.clone()))
                {
                    update.errors.push(error.clone().wire(&evaluation.surface));
                }
                for (key, error) in &diagnostics {
                    if evaluation.diagnostics.get(key) != Some(error) {
                        update.errors.push(error.clone().wire(&evaluation.surface));
                    }
                }
                evaluation.diagnostics = diagnostics;
                evaluation.reported = true;
                evaluation.outcome = outcome;
            }
        }
        tick.state
            .surfaces
            .values_mut()
            .for_each(|surface| surface.shared.retain(|_, call| call.strong_count() > 0));
        update
    }
}

enum Task {
    Binding(Value),
    Property {
        component: String,
        property: String,
    },
    Check(Value),
    CheckRule {
        component: String,
        index: usize,
    },
    FunctionAction {
        call: Value,
        user_activated: bool,
    },
    Event {
        event: EventAction,
        user_activated: bool,
    },
}

impl Task {
    fn imperative(&self) -> bool {
        matches!(self, Self::FunctionAction { .. } | Self::Event { .. })
    }

    fn activated(&self) -> bool {
        matches!(
            self,
            Self::FunctionAction {
                user_activated: true,
                ..
            } | Self::Event {
                user_activated: true,
                ..
            }
        )
    }
}

type Path = Vec<usize>;

struct Evaluation {
    surface: String,
    scope: DataScope,
    task: Task,
    outcome: EvaluationState,
    validation_result: Option<ValidationResult>,
    reported: bool,
    slots: BTreeMap<Path, Slot>,
    diagnostics: BTreeMap<Path, RoutingError>,
    event: EventProgress,
}

#[derive(Default)]
struct EventProgress {
    completed: usize,
    context: JsonObject,
}

struct Slot {
    key: String,
    generation: u64,
    state: SlotState,
    diagnostics: BTreeMap<Path, RoutingError>,
}

enum SlotState {
    Local(Result<Value, RoutingError>),
    Remote(Rc<RefCell<Flight>>),
    Suppressed,
}

enum Flight {
    Waiting(PendingCall),
    Ready(Value),
    Failed(RoutingError),
    Cancelled,
}

#[derive(Default)]
struct RouterState {
    next_call: u64,
    surfaces: BTreeMap<String, SurfaceCalls>,
}

impl RouterState {
    fn bump(&mut self, surface: &str) {
        self.surfaces
            .entry(surface.to_owned())
            .or_default()
            .generation += 1;
    }
}

#[derive(Default)]
struct SurfaceCalls {
    generation: u64,
    shared: BTreeMap<String, Weak<RefCell<Flight>>>,
    cache: VecDeque<(String, Value)>,
}

impl SurfaceCalls {
    fn remember(&mut self, key: String, value: Value) {
        if self.cache.iter().any(|(entry, _)| *entry == key) {
            return;
        }
        if self.cache.len() == CACHE_LIMIT {
            self.cache.pop_front();
        }
        self.cache.push_back((key, value));
    }
}

impl Slot {
    fn waiting(&self) -> bool {
        matches!(&self.state, SlotState::Remote(flight) if matches!(*flight.borrow(), Flight::Waiting(_)))
    }
}
