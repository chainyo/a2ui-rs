use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::check::{CheckRule, ValidationResult};
use super::{
    ACCESSIBILITY_PROPERTY, Evaluation, EvaluationState, EventAction, EventProgress, Flight, Path,
    RouterState, RoutingError, Slot, SlotState, Task,
};
use crate::catalog::{Catalog, PropertyKind};
use crate::protocol::{
    Action, CallAgentFunction, FunctionCall, JsonObject, Metadata, RendererToAgent,
    RendererToAgentMessage,
};
use crate::resolution::{
    ActionContext, DataScope, LocalFunctions, ResolveError, Resolver, coerce, index_with_offset,
    validate_call_shape,
};
use crate::rpc::{Outbound, RpcError, RpcHandler};

pub(super) struct Tick<'a, 'f, 'o> {
    pub(super) rpc: &'a mut RpcHandler,
    pub(super) functions: &'a LocalFunctions<'f>,
    pub(super) state: &'a mut RouterState,
    pub(super) now: Instant,
    pub(super) timeout: Option<Duration>,
    pub(super) outbound: Option<&'a mut Outbound<'o>>,
}

enum Progress {
    Waiting,
    Ready(Option<Value>),
    Cancelled,
}

impl Progress {
    fn value(value: Value) -> Self {
        Self::Ready(Some(value))
    }
}

type EvalResult = Result<Progress, RoutingError>;
type PropertyKinds = BTreeMap<*const Value, PropertyKind>;

struct Context<'a> {
    surface: &'a str,
    scope: &'a DataScope,
    imperative: bool,
    activated: bool,
    slots: &'a mut BTreeMap<Path, Slot>,
    visited: BTreeSet<Path>,
    diagnostics: BTreeMap<Path, RoutingError>,
    saw_suppressed: bool,
}

#[derive(Clone, Default)]
struct Location {
    path: Path,
    call_depth: usize,
}

impl Location {
    fn child(&self, index: usize) -> Self {
        let mut location = self.clone();
        location.path.push(index);
        location
    }

    fn call_args(&self) -> Self {
        let mut location = self.child(0);
        location.call_depth += 1;
        location
    }
}

impl Tick<'_, '_, '_> {
    pub(super) fn evaluate(
        &mut self,
        evaluation: &mut Evaluation,
    ) -> (EvaluationState, BTreeMap<Path, RoutingError>) {
        let (outcome, diagnostics, retry) = self.evaluate_pass(evaluation);
        if retry {
            evaluation
                .slots
                .retain(|_, slot| !matches!(slot.state, SlotState::Suppressed));
            let (outcome, diagnostics, _) = self.evaluate_pass(evaluation);
            (outcome, diagnostics)
        } else {
            (outcome, diagnostics)
        }
    }

    fn evaluate_pass(
        &mut self,
        evaluation: &mut Evaluation,
    ) -> (EvaluationState, BTreeMap<Path, RoutingError>, bool) {
        if self.rpc.processor().get(&evaluation.surface).is_none() {
            evaluation.slots.clear();
            return (EvaluationState::Cancelled, BTreeMap::new(), false);
        }
        let mut context = Context {
            surface: &evaluation.surface,
            scope: &evaluation.scope,
            imperative: evaluation.task.imperative(),
            // A pending action can resume after the user gesture ends.
            activated: evaluation.task.activated() && !evaluation.reported,
            slots: &mut evaluation.slots,
            visited: BTreeSet::new(),
            diagnostics: BTreeMap::new(),
            saw_suppressed: false,
        };
        let location = Location::default();
        let mut mounted_rule = None;
        let result = match &evaluation.task {
            Task::Binding(value)
            | Task::Check(value)
            | Task::FunctionAction { call: value, .. } => {
                self.resolve(&mut context, value, &location, &BTreeMap::new())
            }
            Task::CheckRule {
                component, index, ..
            } => self
                .check_rule(context.surface, component, *index)
                .and_then(|rule| {
                    mounted_rule = rule;
                    match &mounted_rule {
                        Some(rule) => {
                            self.resolve(&mut context, &rule.condition, &location, &BTreeMap::new())
                        }
                        None => Ok(Progress::Ready(None)),
                    }
                }),
            Task::Event {
                event, metadata, ..
            } => self.event(
                &mut context,
                event,
                metadata.as_ref(),
                &mut evaluation.event,
            ),
            Task::Property {
                component,
                property,
                ..
            } => self.property(&mut context, component, property),
        };
        let outcome = match result {
            Ok(Progress::Waiting) => EvaluationState::Pending,
            Ok(Progress::Cancelled) => EvaluationState::Cancelled,
            Ok(Progress::Ready(value)) => EvaluationState::Ready(value.unwrap_or(Value::Null)),
            Err(error) => EvaluationState::Failed(error),
        };
        evaluation.validation_result =
            mounted_rule.map(|rule| ValidationResult::from_check(&outcome, rule.message));
        let retry = context.saw_suppressed && outcome == EvaluationState::Pending;
        context.slots.retain(|path, slot| {
            if context.visited.contains(path) {
                if matches!(outcome, EvaluationState::Failed(_)) && slot.waiting() {
                    slot.state = SlotState::Suppressed;
                }
                true
            } else {
                false
            }
        });
        if context.imperative && outcome != EvaluationState::Pending {
            context.slots.clear();
        }
        (outcome, context.diagnostics, retry)
    }

    fn component(&self, surface: &str, id: &str) -> Option<&crate::protocol::Component> {
        self.rpc.processor().get(surface)?.component(id)
    }

    fn check_rule(
        &self,
        surface: &str,
        id: &str,
        index: usize,
    ) -> Result<Option<CheckRule>, RoutingError> {
        let Some(component) = self.component(surface, id) else {
            return Ok(None);
        };
        let catalog = self.catalog_id(surface, component.catalog_id.clone())?;
        let catalog = self.lookup_catalog(surface, &catalog)?;
        catalog
            .component_checks(component)
            .map_err(RoutingError::expression)?
            .and_then(Value::as_array)
            .and_then(|checks| checks.get(index))
            .map(|rule| serde_json::from_value(rule.clone()).map_err(RoutingError::expression))
            .transpose()
    }

    fn property(&mut self, context: &mut Context<'_>, id: &str, name: &str) -> EvalResult {
        let Some(component) = self.component(context.surface, id) else {
            return Ok(Progress::Ready(None));
        };
        if name == ACCESSIBILITY_PROPERTY {
            let Some(accessibility) = &component.accessibility else {
                return Ok(Progress::Ready(None));
            };
            let payload = serde_json::to_value(accessibility).map_err(RoutingError::expression)?;
            let kinds = [
                ("label", PropertyKind::String),
                ("description", PropertyKind::String),
                ("hidden", PropertyKind::Boolean),
            ]
            .into_iter()
            .filter_map(|(name, kind)| {
                payload
                    .get(name)
                    .map(|value| (std::ptr::from_ref(value), kind))
            })
            .collect();
            return self.resolve(context, &payload, &Location::default(), &kinds);
        }
        let catalog = self.catalog_id(context.surface, component.catalog_id.clone())?;
        let catalog = self.lookup_catalog(context.surface, &catalog)?;
        let payload =
            serde_json::to_value(&component.properties).map_err(RoutingError::expression)?;
        let Some(value) = payload.get(name) else {
            return Ok(Progress::Ready(None));
        };
        let kinds = catalog
            .inspect_payload(catalog.component_schema(&component.component), &payload)
            .properties;
        self.resolve(context, value, &Location::default(), &kinds)
    }

    fn event(
        &mut self,
        context: &mut Context<'_>,
        event: &EventAction,
        metadata: Option<&Metadata>,
        progress: &mut EventProgress,
    ) -> EvalResult {
        let location = Location::default();
        let kinds = BTreeMap::new();
        for (index, (key, value)) in event.context.iter().enumerate().skip(progress.completed) {
            let result = self
                .resolve(context, value, &location.child(index), &kinds)
                .map_err(|error| {
                    event_error(&format!("Action event context entry '{key}'"), error)
                })?;
            match result {
                Progress::Ready(value) => {
                    progress
                        .context
                        .insert(key.clone(), value.unwrap_or(Value::Null));
                    progress.completed = index + 1;
                }
                result => return Ok(result),
            }
        }
        let user_message = if let Some(value) = &event.user_message {
            let result = self
                .resolve(context, value, &location.child(event.context.len()), &kinds)
                .map_err(|error| event_error("Action event userMessage", error))?;
            match result {
                Progress::Ready(None | Some(Value::Null)) => None,
                Progress::Ready(Some(Value::String(message))) => {
                    (!message.is_empty()).then_some(message)
                }
                Progress::Ready(_) => {
                    return Err(RoutingError::new(
                        "INVALID_ACTION",
                        "Action event userMessage must be a string",
                    ));
                }
                result => return Ok(result),
            }
        } else {
            None
        };
        let action = Action {
            name: event.name.clone(),
            surface_id: context.surface.to_owned(),
            source_component_id: event.source_component_id.clone(),
            timestamp: event.timestamp.clone(),
            context: std::mem::take(&mut progress.context),
            user_message,
            metadata: metadata.cloned(),
            extra: BTreeMap::new(),
        };
        self.outbound
            .as_deref_mut()
            .ok_or_else(|| RoutingError::expression("No outbound listener for action"))?(
            RendererToAgentMessage::V1(RendererToAgent::Action(action)),
        )
        .map_err(|error| RoutingError::new("EXECUTION_ERROR", error))?;
        Ok(Progress::value(Value::Null))
    }

    fn resolve(
        &mut self,
        context: &mut Context<'_>,
        value: &Value,
        location: &Location,
        kinds: &PropertyKinds,
    ) -> EvalResult {
        if location.path.len() >= 50 {
            return Err(RoutingError::expression("Global recursion limit exceeded"));
        }
        let kind = kinds.get(&std::ptr::from_ref(value)).copied();
        if matches!(kind, Some(PropertyKind::Action)) {
            return Ok(Progress::value(value.clone()));
        }
        let result = match value {
            Value::Object(object) if object.contains_key("@path") => {
                if object.len() != 1 {
                    return Err(RoutingError::expression("Invalid binding envelope"));
                }
                let path = object["@path"]
                    .as_str()
                    .ok_or_else(|| RoutingError::expression("Invalid data binding path"))?;
                let surface = self
                    .rpc
                    .processor()
                    .get(context.surface)
                    .ok_or_else(|| RoutingError::expression("Unknown surface"))?;
                let value = context
                    .scope
                    .read(surface.data_model(), path)
                    .map_err(RoutingError::expression)?
                    .cloned();
                Progress::Ready(value)
            }
            Value::Object(object) if object.contains_key("@call") => {
                let call =
                    serde_json::from_value(value.clone()).map_err(RoutingError::expression)?;
                self.call(context, call, location)?
            }
            Value::Object(object) => self.object(context, object, location, kinds)?,
            Value::Array(array) => self.array(context, array, location, kinds)?,
            literal => Progress::value(literal.clone()),
        };
        Ok(match result {
            Progress::Ready(value) if kind.is_some() => {
                Progress::value(coerce(value.unwrap_or(Value::Null), kind))
            }
            result => result,
        })
    }

    fn object(
        &mut self,
        context: &mut Context<'_>,
        input: &JsonObject,
        location: &Location,
        kinds: &PropertyKinds,
    ) -> EvalResult {
        let mut resolved = JsonObject::new();
        let mut waiting = false;
        for (index, (key, value)) in input.iter().enumerate() {
            let key = if key.starts_with("@@") {
                &key[1..]
            } else if key.starts_with('@') {
                return Err(RoutingError::expression(format!(
                    "Unescaped reserved key '{key}'"
                )));
            } else {
                key
            };
            match self.resolve(context, value, &location.child(index), kinds)? {
                Progress::Ready(Some(value)) => {
                    resolved.insert(key.to_owned(), value);
                }
                Progress::Ready(None) => {}
                Progress::Waiting if context.imperative => return Ok(Progress::Waiting),
                Progress::Waiting => waiting = true,
                Progress::Cancelled => return Ok(Progress::Cancelled),
            }
        }
        Ok(if waiting {
            Progress::Waiting
        } else {
            Progress::value(Value::Object(resolved))
        })
    }

    fn array(
        &mut self,
        context: &mut Context<'_>,
        input: &[Value],
        location: &Location,
        kinds: &PropertyKinds,
    ) -> EvalResult {
        let mut resolved = Vec::with_capacity(input.len());
        let mut waiting = false;
        for (index, value) in input.iter().enumerate() {
            match self.resolve(context, value, &location.child(index), kinds)? {
                Progress::Ready(Some(value)) => resolved.push(value),
                Progress::Ready(None) => {
                    return Err(RoutingError::expression(
                        "An array element has no JSON value",
                    ));
                }
                Progress::Waiting if context.imperative => return Ok(Progress::Waiting),
                Progress::Waiting => waiting = true,
                Progress::Cancelled => return Ok(Progress::Cancelled),
            }
        }
        Ok(if waiting {
            Progress::Waiting
        } else {
            Progress::value(Value::Array(resolved))
        })
    }

    fn call(
        &mut self,
        context: &mut Context<'_>,
        mut call: FunctionCall,
        location: &Location,
    ) -> EvalResult {
        if location.call_depth >= 5 {
            return Err(RoutingError::expression(
                "Maximum functionCall depth exceeded",
            ));
        }
        validate_call_shape(&call).map_err(RoutingError::expression)?;
        if context.imperative
            && let Some(key) = context
                .slots
                .get(&location.path)
                .map(|slot| slot.key.clone())
        {
            context.visited.insert(location.path.clone());
            if let Some(result) = self.reuse_slot(context, location, &key, 0) {
                return result;
            }
        }
        if call.name == "@index" {
            let index = context.scope.index().ok_or_else(|| {
                RoutingError::expression(crate::resolution::ResolveError::IndexScope)
            })?;
            return match self.arguments(context, call.args.as_ref(), location, &BTreeMap::new())? {
                Arguments::Ready(args) => index_with_offset(index, &args)
                    .map(Progress::value)
                    .map_err(RoutingError::expression),
                Arguments::Waiting => Ok(Progress::Waiting),
                Arguments::Cancelled => Ok(Progress::Cancelled),
            };
        }

        let catalog_id = self.catalog_id(context.surface, call.catalog_id.take())?;
        let (declared, local, raw, kinds) = self.prepare(context, &mut call, &catalog_id)?;
        let args = match self.arguments(
            context,
            raw.get("args").and_then(Value::as_object),
            location,
            &kinds,
        )? {
            Arguments::Ready(args) => args,
            Arguments::Waiting => return Ok(Progress::Waiting),
            Arguments::Cancelled => return Ok(Progress::Cancelled),
        };
        self.set_resolved_arguments(
            context.surface,
            &mut call,
            args,
            &catalog_id,
            declared,
            local,
        )?;

        let empty = JsonObject::new();
        let key = serde_json::to_string(&(
            &catalog_id,
            &call.name,
            call.args.as_ref().unwrap_or(&empty),
        ))
        .map_err(RoutingError::expression)?;
        context.visited.insert(location.path.clone());
        let generation = self
            .state
            .surfaces
            .entry(context.surface.to_owned())
            .or_default()
            .generation;
        if let Some(result) = self.reuse_slot(context, location, &key, generation) {
            return result;
        }

        let state = if local {
            SlotState::Local(self.local(context, &catalog_id, &call, location))
        } else {
            SlotState::Remote(self.route(context, call, &key)?)
        };
        let result = match &state {
            SlotState::Local(value) => value.clone().map(Progress::value),
            SlotState::Remote(flight) => {
                self.read_flight(context.surface, &key, flight, !context.imperative)
            }
            SlotState::Suppressed => Ok(Progress::Waiting),
        };
        context.slots.insert(
            location.path.clone(),
            Slot {
                key,
                generation,
                state,
                diagnostics: context
                    .diagnostics
                    .iter()
                    .filter(|(path, _)| path.starts_with(&location.child(1).path))
                    .map(|(path, error)| (path.clone(), error.clone()))
                    .collect(),
            },
        );
        result
    }

    fn set_resolved_arguments(
        &self,
        surface: &str,
        call: &mut FunctionCall,
        mut args: JsonObject,
        catalog_id: &str,
        declared: bool,
        local: bool,
    ) -> Result<(), RoutingError> {
        let catalog = declared
            .then(|| self.lookup_catalog(surface, catalog_id))
            .transpose()?;
        if let Some(catalog) = catalog.filter(|_| local) {
            catalog.preserve_required_local_arguments(call, &mut args);
        }
        if call.args.is_some() || !args.is_empty() {
            call.args = Some(args);
        }
        if let Some(catalog) = catalog {
            catalog
                .validate_function(call)
                .map_err(|error| validation_error(&call.name, error))?;
        }
        Ok(())
    }

    fn reuse_slot(
        &mut self,
        context: &mut Context<'_>,
        location: &Location,
        key: &str,
        generation: u64,
    ) -> Option<EvalResult> {
        let slot = context
            .slots
            .get(&location.path)
            .filter(|slot| slot.key == key)?;
        match &slot.state {
            SlotState::Local(value) if context.imperative || generation == slot.generation => {
                context.diagnostics.extend(slot.diagnostics.clone());
                Some(value.clone().map(Progress::value))
            }
            SlotState::Remote(flight) => {
                let flight = Rc::clone(flight);
                let status = self.read_flight(context.surface, key, &flight, !context.imperative);
                if matches!(status, Ok(Progress::Cancelled))
                    && generation != slot.generation
                    && !context.imperative
                {
                    None
                } else {
                    Some(status)
                }
            }
            SlotState::Suppressed => {
                context.saw_suppressed = true;
                Some(Ok(Progress::Waiting))
            }
            SlotState::Local(_) => None,
        }
    }

    fn arguments(
        &mut self,
        context: &mut Context<'_>,
        args: Option<&JsonObject>,
        location: &Location,
        kinds: &PropertyKinds,
    ) -> Result<Arguments, RoutingError> {
        let Some(args) = args else {
            return Ok(Arguments::Ready(JsonObject::new()));
        };
        match self.object(context, args, &location.call_args(), kinds)? {
            Progress::Ready(Some(Value::Object(args))) => Ok(Arguments::Ready(args)),
            Progress::Ready(_) => Err(RoutingError::expression("Invalid function arguments")),
            Progress::Waiting => Ok(Arguments::Waiting),
            Progress::Cancelled => Ok(Arguments::Cancelled),
        }
    }

    fn catalog_id(&self, surface: &str, catalog: Option<String>) -> Result<String, RoutingError> {
        catalog
            .or_else(|| {
                self.rpc
                    .processor()
                    .get(surface)
                    .and_then(|surface| surface.catalog_id())
                    .map(str::to_owned)
            })
            .ok_or_else(|| RoutingError::new("CATALOG_ERROR", "Surface has no default catalogId"))
    }

    fn lookup_catalog(&self, surface: &str, catalog: &str) -> Result<&Catalog, RoutingError> {
        let processor = self.rpc.processor();
        processor
            .get(surface)
            .and_then(|surface| processor.surface_catalogs(surface).get(catalog))
            .ok_or_else(|| {
                RoutingError::new("CATALOG_ERROR", format!("Catalog not found: {catalog}"))
            })
    }

    fn prepare(
        &self,
        context: &Context<'_>,
        call: &mut FunctionCall,
        catalog_id: &str,
    ) -> Result<(bool, bool, Value, PropertyKinds), RoutingError> {
        let catalog = self.lookup_catalog(context.surface, catalog_id)?;
        call.catalog_id = Some(catalog_id.to_owned());
        let schema = catalog.function_schema(&call.name);
        let declared = !schema.is_null();
        if declared {
            if catalog.is_agent_only(&call.name) {
                return Err(RoutingError::expression(format!(
                    "Function '{}' is agentOnly",
                    call.name
                )));
            }
            if catalog.requires_activation(&call.name) && !context.activated {
                return Err(RoutingError::expression(format!(
                    "Function '{}' requires user activation",
                    call.name
                )));
            }
            catalog.apply_function_defaults(call);
            catalog
                .validate_function(call)
                .map_err(|error| validation_error(&call.name, error))?;
        }
        let local = declared
            && (call.name == "formatString" || self.functions.contains(catalog_id, &call.name));
        let raw = serde_json::to_value(call).map_err(RoutingError::expression)?;
        let kinds = catalog.inspect_payload(schema, &raw).properties;
        Ok((declared, local, raw, kinds))
    }

    fn local(
        &mut self,
        context: &mut Context<'_>,
        catalog: &str,
        call: &FunctionCall,
        location: &Location,
    ) -> Result<Value, RoutingError> {
        let surface = self
            .rpc
            .processor()
            .get(context.surface)
            .ok_or_else(|| RoutingError::expression("Unknown surface"))?;
        let empty = JsonObject::new();
        let args = call.args.as_ref().unwrap_or(&empty);
        if let Some(action) = self.functions.action(catalog, &call.name) {
            if !context.imperative {
                return Err(RoutingError::expression(
                    ResolveError::ActionContextRequired(call.name.clone()),
                ));
            }
            let original = surface.data_model();
            let mut data = original.clone();
            let value = action(args, &mut ActionContext::new(context.scope, &mut data))
                .map_err(|error| RoutingError::new("EXECUTION_ERROR", error))?;
            if &data != original {
                self.rpc
                    .write_binding(
                        context.surface,
                        &DataScope::default(),
                        "/",
                        data.root().clone(),
                    )
                    .map_err(|error| RoutingError::new("EXECUTION_ERROR", error.to_string()))?;
                self.state.bump(context.surface);
            }
            return Ok(value);
        }
        let resolver = Resolver::new(
            surface,
            self.rpc.processor().surface_catalogs(surface),
            self.functions,
        );
        if call.name == "formatString" {
            let template = args
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| RoutingError::expression("formatString requires a string value"))?;
            let mut text = String::new();
            for (index, part) in crate::expression::parse_template(template)
                .map_err(RoutingError::expression)?
                .into_iter()
                .enumerate()
            {
                let path = location.child(1).child(index).path;
                match resolver.template_part(context.scope, catalog, part) {
                    Ok(part) => text.push_str(&part),
                    Err(error) => {
                        context
                            .diagnostics
                            .insert(path, RoutingError::expression(error));
                    }
                }
            }
            return Ok(Value::String(text));
        }
        resolver
            .invoke_resolved(context.scope, catalog, &call.name, args)
            .map_err(|error| {
                if context.imperative
                    && matches!(error, crate::resolution::ResolveError::LocalFunction(_))
                {
                    RoutingError::new("EXECUTION_ERROR", error.to_string())
                } else {
                    RoutingError::expression(error)
                }
            })
    }

    fn route(
        &mut self,
        context: &Context<'_>,
        call: FunctionCall,
        key: &str,
    ) -> Result<Rc<RefCell<Flight>>, RoutingError> {
        let surface = self
            .state
            .surfaces
            .entry(context.surface.to_owned())
            .or_default();
        if !context.imperative {
            if let Some((_, value)) = surface.cache.iter().find(|(entry, _)| entry == key) {
                return Ok(Rc::new(RefCell::new(Flight::Ready(value.clone()))));
            }
            if let Some(flight) = surface.shared.get(key).and_then(Weak::upgrade)
                && matches!(*flight.borrow(), Flight::Waiting(_))
            {
                return Ok(flight);
            }
        }
        let outbound = self.outbound.as_deref_mut().ok_or_else(|| {
            RoutingError::expression(format!("No local implementation for '{}'", call.name))
        })?;
        let id = loop {
            self.state.next_call += 1;
            let id = format!("a2ui-route-{}", self.state.next_call);
            if !self.rpc.used_call_id(&id) {
                break id;
            }
        };
        let request = CallAgentFunction {
            surface_id: context.surface.to_owned(),
            function_call_id: id.clone(),
            call_function: call,
        };
        let status = match self
            .rpc
            .send_resolved(request, self.timeout, self.now, outbound)
        {
            Ok(pending) => Flight::Waiting(pending),
            Err(error) => Flight::Failed(RoutingError {
                code: "EXPRESSION_ERROR",
                message: error.to_string(),
                function_call_id: Some(id),
            }),
        };
        let sharing = matches!(status, Flight::Waiting(_)) && !context.imperative;
        let flight = Rc::new(RefCell::new(status));
        if sharing {
            surface
                .shared
                .insert(key.to_owned(), Rc::downgrade(&flight));
        }
        Ok(flight)
    }

    fn read_flight(
        &mut self,
        surface: &str,
        key: &str,
        flight: &RefCell<Flight>,
        reactive: bool,
    ) -> EvalResult {
        let mut flight = flight.borrow_mut();
        if let Flight::Waiting(pending) = &mut *flight
            && let Some(result) = pending.try_result()
        {
            *flight = match result {
                Ok(value) => {
                    if reactive {
                        self.state
                            .surfaces
                            .entry(surface.to_owned())
                            .or_default()
                            .remember(key.to_owned(), value.clone());
                    }
                    Flight::Ready(value)
                }
                Err(RpcError::Cancelled) => Flight::Cancelled,
                Err(error) => Flight::Failed(RoutingError {
                    code: "EXPRESSION_ERROR",
                    message: error.to_string(),
                    function_call_id: Some(pending.id().to_owned()),
                }),
            };
        }
        match &*flight {
            Flight::Ready(value) => Ok(Progress::value(value.clone())),
            Flight::Waiting(_) => Ok(Progress::Waiting),
            Flight::Cancelled => Ok(Progress::Cancelled),
            Flight::Failed(error) => Err(error.clone()),
        }
    }
}

enum Arguments {
    Ready(JsonObject),
    Waiting,
    Cancelled,
}

fn validation_error(name: &str, error: impl std::fmt::Display) -> RoutingError {
    RoutingError::expression(format!("Validation failed for function '{name}': {error}"))
}

fn event_error(field: &str, mut error: RoutingError) -> RoutingError {
    if error.code == "EXPRESSION_ERROR" && error.function_call_id.is_none() {
        error.message = format!("{field} is invalid: {}", error.message);
    }
    error
}
