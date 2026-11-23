//! Coordinates v1.0 calls between a renderer and an agent.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use serde_json::Value;
use thiserror::Error;

use crate::catalog::Catalog;
use crate::data_model::DataModel;
use crate::protocol::{
    AgentToRenderer, AgentToRendererMessage, CallAgentFunction, CallRendererFunction, ErrorTarget,
    FunctionCall, FunctionError, FunctionResponse, GenericError, RendererError, RendererToAgent,
    RendererToAgentMessage,
};
use crate::resolution::{DataScope, LocalFunctions, ResolveError, Resolver, validate_call_shape};
use crate::validation::{MessageProcessor, ValidationError};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Options for a renderer-initiated call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallOptions {
    /// The time to wait for the agent. The default is 30 seconds.
    pub timeout: Option<Duration>,
    /// Whether this call comes from an active user gesture.
    pub user_activated: bool,
}

/// Submits an outbound message to the caller's transport.
pub type Outbound<'a> = dyn FnMut(RendererToAgentMessage) -> Result<(), String> + 'a;

/// Receives renderer messages and owns the pending agent calls for one session.
///
/// Use one handler for the lifetime of a transport session. Supply a fresh
/// correlation ID for each agent call. Use the same monotonic clock for all calls.
#[derive(Default)]
pub struct RpcHandler {
    processor: MessageProcessor,
    pending: Arc<Mutex<Pending>>,
    used_ids: BTreeSet<String>,
    disposed: bool,
}

impl RpcHandler {
    pub(crate) fn used_call_id(&self, id: &str) -> bool {
        self.used_ids.contains(id)
    }
    /// Moves a renderer's message processor into a new RPC session.
    #[must_use]
    pub fn new(processor: MessageProcessor) -> Self {
        Self {
            processor,
            pending: Arc::default(),
            used_ids: BTreeSet::new(),
            disposed: false,
        }
    }

    /// Returns the processor for surface queries and local resolution.
    #[must_use]
    pub fn processor(&self) -> &MessageProcessor {
        &self.processor
    }

    /// Registers a catalog with the processor.
    ///
    /// # Errors
    /// Returns an error if the session is closed or the catalog ID is already registered.
    pub fn add_catalog(&mut self, catalog: Catalog) -> Result<(), ReceiveError> {
        self.ensure_open()?;
        self.processor.add_catalog(catalog).map_err(Into::into)
    }

    /// Writes a scoped binding through the processor.
    ///
    /// # Errors
    /// Returns an error if the session is closed or the processor rejects the write.
    pub fn write_binding(
        &mut self,
        surface_id: &str,
        scope: &DataScope,
        path: &str,
        value: Value,
    ) -> Result<(), ReceiveError> {
        self.ensure_open()?;
        self.processor
            .write_binding(surface_id, scope, path, value)
            .map_err(ValidationError::from)?;
        Ok(())
    }

    /// Validates a JSON message and processes it.
    ///
    /// # Errors
    /// Returns an error if the envelope is invalid or the processor rejects the message.
    pub fn receive_json(
        &mut self,
        input: &Value,
        now: Instant,
        functions: &LocalFunctions<'_>,
    ) -> Result<Option<RendererToAgentMessage>, ReceiveError> {
        let message = serde_json::from_value(input.clone())
            .map_err(|error| invalid_envelope(input, &error))?;
        self.receive(message, now, functions)
    }

    /// Applies an agent message and returns a renderer response when required.
    ///
    /// A response at or after its deadline cannot settle an expired call.
    ///
    /// # Errors
    /// Returns an error if the processor rejects a surface message or the handler is disposed.
    pub fn receive(
        &mut self,
        message: AgentToRendererMessage,
        now: Instant,
        functions: &LocalFunctions<'_>,
    ) -> Result<Option<RendererToAgentMessage>, ReceiveError> {
        self.expire(now);
        let AgentToRendererMessage::V1(operation) = message;
        if let AgentToRenderer::AgentFunctionResponse(response) = operation {
            self.settle(response);
            return Ok(None);
        }
        self.ensure_open()?;
        match operation {
            AgentToRenderer::CallRendererFunction(call) => {
                let result = self.execute_renderer_call(&call, functions);
                let response = match result {
                    Ok(value) => FunctionResponse::Value {
                        function_call_id: call.function_call_id,
                        value,
                    },
                    Err(error) => FunctionResponse::Error {
                        function_call_id: call.function_call_id,
                        error: FunctionError {
                            code: error.code().to_owned(),
                            message: error.to_string(),
                        },
                    },
                };
                Ok(Some(RendererToAgentMessage::V1(
                    RendererToAgent::RendererFunctionResponse(response),
                )))
            }
            AgentToRenderer::DeleteSurface(delete) => {
                self.cancel_surface_calls(&delete.surface_id);
                self.processor.apply(AgentToRendererMessage::V1(
                    AgentToRenderer::DeleteSurface(delete),
                ))?;
                Ok(None)
            }
            operation => {
                self.processor
                    .apply(AgentToRendererMessage::V1(operation))?;
                Ok(None)
            }
        }
    }

    /// Resolves and sends a renderer-initiated call. Returns its completion handle.
    ///
    /// The data context is the root of the request's surface.
    /// The transport must report a delivery failure to the caller.
    ///
    /// # Errors
    /// Returns an error if the handler cannot validate, register, or send the call.
    pub fn call_agent_function(
        &mut self,
        mut request: CallAgentFunction,
        options: CallOptions,
        now: Instant,
        functions: &LocalFunctions<'_>,
        outbound: Option<&mut Outbound<'_>>,
    ) -> Result<PendingCall, RpcError> {
        self.expire(now);
        if self.disposed {
            return Err(RpcError::Disposed);
        }
        let outbound = outbound.ok_or(RpcError::NoListener)?;
        if self.used_ids.contains(&request.function_call_id) {
            return Err(RpcError::Duplicate(request.function_call_id));
        }
        if request.function_call_id.is_empty() {
            return Err(invalid("A function call ID must be nonempty"));
        }
        let timeout = options.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let deadline = now
            .checked_add(timeout)
            .ok_or_else(|| invalid("Timeout is out of range"))?;
        if deadline <= now {
            return Err(RpcError::Timeout);
        }
        let surface = self
            .processor
            .get(&request.surface_id)
            .ok_or_else(|| invalid(format!("Unknown surface: {}", request.surface_id)))?;
        let call = &mut request.call_function;
        validate_call_shape(call).map_err(invalid_error)?;
        if call.name.starts_with('@') {
            return Err(invalid("A system function cannot run on the agent"));
        }
        let catalog_id = call
            .catalog_id
            .as_deref()
            .or(surface.catalog_id())
            .ok_or_else(|| invalid("Surface has no default catalogId"))?;
        let catalog = self.catalog(catalog_id)?;
        call.catalog_id = Some(catalog_id.to_owned());
        let declared = !catalog.function_schema(&call.name).is_null();
        if declared {
            if catalog.is_agent_only(&call.name) {
                return Err(invalid(format!("Function '{}' is agentOnly", call.name)));
            }
            if catalog.requires_activation(&call.name) && !options.user_activated {
                return Err(invalid(format!(
                    "Function '{}' requires user activation",
                    call.name
                )));
            }
            catalog.apply_function_defaults(call);
            catalog.validate_function(call).map_err(invalid_error)?;
        }
        let resolver = Resolver::new(surface, &self.processor.catalogs, functions);
        let args = resolver
            .rpc_arguments(call, catalog)
            .map_err(execution_error)?;
        if call.args.is_some() || !args.is_empty() {
            call.args = Some(args);
        }

        self.enqueue(request, deadline, outbound)
    }

    pub(crate) fn send_resolved(
        &mut self,
        request: CallAgentFunction,
        timeout: Option<Duration>,
        now: Instant,
        outbound: &mut Outbound<'_>,
    ) -> Result<PendingCall, RpcError> {
        if self.disposed {
            return Err(RpcError::Disposed);
        }
        if self.used_ids.contains(&request.function_call_id) {
            return Err(RpcError::Duplicate(request.function_call_id));
        }
        let deadline = now
            .checked_add(timeout.unwrap_or(DEFAULT_TIMEOUT))
            .ok_or_else(|| invalid("Timeout is out of range"))?;
        if deadline <= now {
            return Err(RpcError::Timeout);
        }
        self.enqueue(request, deadline, outbound)
    }

    fn enqueue(
        &mut self,
        request: CallAgentFunction,
        deadline: Instant,
        outbound: &mut Outbound<'_>,
    ) -> Result<PendingCall, RpcError> {
        let id = request.function_call_id.clone();
        let state = Arc::new(Mutex::new(Completion::default()));
        self.used_ids.insert(id.clone());
        lock(&self.pending).insert(
            id.clone(),
            Entry {
                surface: request.surface_id.clone(),
                deadline,
                state: Arc::clone(&state),
            },
        );
        let handle = PendingCall {
            id,
            state,
            pending: Arc::downgrade(&self.pending),
        };
        outbound(RendererToAgentMessage::V1(
            RendererToAgent::CallAgentFunction(request),
        ))
        .map_err(RpcError::Delivery)?;
        Ok(handle)
    }

    /// Returns the earliest deadline among pending calls.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        lock(&self.pending)
            .values()
            .map(|entry| entry.deadline)
            .min()
    }

    /// Completes pending calls whose deadlines have elapsed.
    pub fn expire(&mut self, now: Instant) {
        self.finish_where(|entry| entry.deadline <= now, || RpcError::Timeout);
    }

    /// Cancels one pending call. Returns whether the call was pending.
    pub fn cancel_call(&mut self, id: &str) -> bool {
        let entry = lock(&self.pending).remove(id);
        if let Some(entry) = entry {
            entry.finish(Err(RpcError::Cancelled));
            true
        } else {
            false
        }
    }

    /// Cancels all pending calls for this surface.
    pub fn cancel_surface_calls(&mut self, surface_id: &str) {
        self.finish_where(|entry| entry.surface == surface_id, || RpcError::Cancelled);
    }

    /// Cancels all pending calls and closes the handler.
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.used_ids.clear();
        self.finish_where(|_| true, || RpcError::Cancelled);
    }

    fn ensure_open(&self) -> Result<(), ReceiveError> {
        if self.disposed {
            Err(ReceiveError::Disposed)
        } else {
            Ok(())
        }
    }

    fn finish_where(&mut self, predicate: impl Fn(&Entry) -> bool, error: impl Fn() -> RpcError) {
        let entries = lock(&self.pending)
            .extract_if(.., |_, entry| predicate(entry))
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();
        for entry in entries {
            entry.finish(Err(error()));
        }
    }

    fn settle(&mut self, response: FunctionResponse) {
        let (id, outcome) = match response {
            FunctionResponse::Value {
                function_call_id,
                value,
            } => (function_call_id, Ok(value)),
            FunctionResponse::Error {
                function_call_id,
                error,
            } => (
                function_call_id,
                Err(RpcError::Agent {
                    code: error.code,
                    message: error.message,
                }),
            ),
        };
        let entry = lock(&self.pending).remove(&id);
        if let Some(entry) = entry {
            entry.finish(outcome);
        }
    }

    fn catalog(&self, id: &str) -> Result<&Catalog, RpcError> {
        self.processor
            .catalogs
            .get(id)
            .ok_or_else(|| invalid(format!("Catalog not found: {id}")))
    }

    fn execute_renderer_call(
        &self,
        request: &CallRendererFunction,
        functions: &LocalFunctions<'_>,
    ) -> Result<Value, RpcError> {
        let empty = DataModel::default();
        let resolver = if let Some(id) = &request.surface_id {
            let surface = self
                .processor
                .get(id)
                .ok_or_else(|| invalid(format!("Unknown surface: {id}")))?;
            Resolver::new(surface, &self.processor.catalogs, functions)
        } else {
            Resolver::with_context(&empty, None, &self.processor.catalogs, functions)
        };
        let catalog = self.catalog(&request.call_function.catalog_id)?;
        let mut call = FunctionCall {
            name: request.call_function.name.clone(),
            catalog_id: Some(request.call_function.catalog_id.clone()),
            args: request.call_function.args.clone(),
        };
        validate_call_shape(&call).map_err(invalid_error)?;
        let schema = catalog.function_schema(&call.name);
        if schema.is_null() {
            return Err(invalid(format!("Function not found: {}", call.name)));
        }
        if !matches!(
            schema["allowedCallers"].as_str(),
            Some("rendererOrAgent" | "agentOnly")
        ) {
            return Err(invalid(format!(
                "Function '{}' cannot be called by agent (allowedCallers is rendererOnly).",
                call.name,
            )));
        }
        catalog.apply_function_defaults(&mut call);
        catalog
            .validate_function(&call)
            .map_err(|error| invalid(format!("Invalid function arguments: {error}")))?;
        let execution = catch_unwind(AssertUnwindSafe(|| {
            let args = resolver.rpc_arguments(&call, catalog)?;
            functions.invoke(catalog.id(), &call.name, &args)
        }));
        match execution {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(ResolveError::MissingHandler { .. })) => {
                Err(invalid(format!("Function not found: {}", call.name)))
            }
            Ok(Err(error)) => Err(execution_error(error)),
            Err(_) => Err(RpcError::Execution(
                "An error occurred during function execution.".to_owned(),
            )),
        }
    }
}

impl Drop for RpcHandler {
    fn drop(&mut self) {
        self.dispose();
    }
}

type Pending = BTreeMap<String, Entry>;
type Outcome = Result<Value, RpcError>;

struct Entry {
    surface: String,
    deadline: Instant,
    state: Arc<Mutex<Completion>>,
}

impl Entry {
    fn finish(self, result: Outcome) {
        let waker = {
            let mut completion = lock(&self.state);
            completion.result = Some(result);
            completion.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

#[derive(Default)]
struct Completion {
    result: Option<Outcome>,
    waker: Option<Waker>,
}

/// A pending agent call. Dropping this handle removes the pending call.
#[must_use]
pub struct PendingCall {
    id: String,
    state: Arc<Mutex<Completion>>,
    pending: Weak<Mutex<Pending>>,
}

impl PendingCall {
    /// Returns the message correlation ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Takes the result if it is available. Consume the result only once.
    pub fn try_result(&mut self) -> Option<Result<Value, RpcError>> {
        lock(&self.state).result.take()
    }
}

impl Future for PendingCall {
    type Output = Result<Value, RpcError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = lock(&self.state);
        if let Some(result) = state.result.take() {
            Poll::Ready(result)
        } else {
            state.waker = Some(context.waker().clone());
            Poll::Pending
        }
    }
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.upgrade() {
            lock(&pending).remove(&self.id);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A function call did not complete successfully.
#[derive(Debug, Error)]
pub enum RpcError {
    /// The function, catalog, or arguments are invalid for this call.
    #[error("{0}")]
    InvalidCall(String),
    /// An argument or renderer function failed to execute.
    #[error("{0}")]
    Execution(String),
    /// A response did not arrive by the deadline.
    #[error("The agent call timed out")]
    Timeout,
    /// The caller or surface cancelled the call.
    #[error("The agent call was cancelled")]
    Cancelled,
    /// The session is closed.
    #[error("The RPC handler is disposed")]
    Disposed,
    /// The session already used this correlation ID.
    #[error("Duplicate function call ID '{0}'")]
    Duplicate(String),
    /// The caller did not provide a transport listener.
    #[error("No outbound listener is available")]
    NoListener,
    /// The outbound listener failed to deliver the call.
    #[error("Could not deliver the agent call: {0}")]
    Delivery(String),
    /// The agent returned a structured failure.
    #[error("{message}")]
    Agent {
        /// The code supplied by the agent.
        code: String,
        /// The message supplied by the agent.
        message: String,
    },
}

impl RpcError {
    /// Returns the error code.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidCall(_) => "INVALID_FUNCTION_CALL",
            Self::Execution(_) | Self::Delivery(_) => "EXECUTION_ERROR",
            Self::Timeout => "TIMEOUT",
            Self::Cancelled => "CANCELLED",
            Self::Disposed => "DISPOSED",
            Self::Duplicate(_) => "DUPLICATE",
            Self::NoListener => "NO_LISTENER",
            Self::Agent { code, .. } => code,
        }
    }
}

/// An inbound message could not be applied.
#[derive(Debug, Error)]
pub enum ReceiveError {
    /// The JSON does not match an A2UI v1.0 message.
    #[error("{message}")]
    Envelope {
        /// The reason the message is invalid.
        message: String,
        /// The call ID of the invalid renderer call, if available.
        function_call_id: Option<String>,
    },
    /// The surface operation failed.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The session is closed.
    #[error("The RPC handler is disposed")]
    Disposed,
}

impl ReceiveError {
    /// Returns a correlated renderer error for an invalid RPC envelope.
    #[must_use]
    pub fn renderer_error(&self) -> Option<RendererToAgentMessage> {
        let Self::Envelope {
            message,
            function_call_id: Some(id),
        } = self
        else {
            return None;
        };
        Some(RendererToAgentMessage::V1(RendererToAgent::Error(
            RendererError::Generic(GenericError {
                code: "INVALID_FUNCTION_CALL".to_owned(),
                message: message.clone(),
                target: ErrorTarget::FunctionCallId(id.clone()),
                extra: BTreeMap::new(),
            }),
        )))
    }
}

fn invalid_envelope(input: &Value, error: &serde_json::Error) -> ReceiveError {
    let request = &input["callRendererFunction"];
    let call = &request["callFunction"];
    let function_call_id = (input["version"] == "1.0"
        && input.as_object().is_some_and(|object| object.len() == 2))
    .then(|| request["functionCallId"].as_str())
    .flatten()
    .map(str::to_owned);
    let message = if function_call_id.is_some() && !call.is_object() {
        "Invalid or missing callFunction".to_owned()
    } else if function_call_id.is_some() && !call["catalogId"].is_string() {
        "Invalid or missing callFunction.catalogId".to_owned()
    } else if function_call_id.is_some() && call.get("args").is_some_and(|args| !args.is_object()) {
        "Invalid callFunction.args: expected an object".to_owned()
    } else {
        error.to_string()
    };
    ReceiveError::Envelope {
        message,
        function_call_id,
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::InvalidCall(message.into())
}

fn invalid_error(error: impl std::fmt::Display) -> RpcError {
    invalid(error.to_string())
}

fn execution_error(error: ResolveError) -> RpcError {
    match error {
        ResolveError::LocalFunction(message) => RpcError::Execution(message),
        _ => RpcError::Execution(error.to_string()),
    }
}
