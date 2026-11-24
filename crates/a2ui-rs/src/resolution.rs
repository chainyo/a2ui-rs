//! Resolves v1.0 data bindings, collection scopes, and local expressions.

use std::collections::BTreeMap;

use serde_json::{Number, Value};
use thiserror::Error;

use crate::catalog::{Catalog, CatalogError, CatalogView, ChildReference, PropertyKind};
use crate::data_model::{DataError, DataModel};
use crate::expression::{Expression, ParseError, is_identifier, parse_template};
use crate::protocol::{Component, DataBinding, FunctionCall, JsonObject};
use crate::surface::{Surface, SurfaceError};

const MAX_DEPTH: usize = 50;
const MAX_CALL_DEPTH: usize = 5;

/// A location in the surface data model and its nearest collection index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataScope {
    path: String,
    index: Option<usize>,
}

impl Default for DataScope {
    fn default() -> Self {
        Self {
            path: "/".to_owned(),
            index: None,
        }
    }
}

impl DataScope {
    /// Returns the absolute path for this scope.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the index of the nearest collection item.
    #[must_use]
    pub fn index(&self) -> Option<usize> {
        self.index
    }

    /// Resolves a relative or absolute path.
    #[must_use]
    pub fn resolve_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_owned()
        } else if path.is_empty() || path == "." {
            self.path.clone()
        } else {
            format!("{}/{path}", self.path.trim_end_matches('/'))
        }
    }

    /// Creates a nested scope with the same collection index.
    #[must_use]
    pub fn nested(&self, path: &str) -> Self {
        let path = self.resolve_path(path);
        let path = path.trim_end_matches('/');
        Self {
            path: if path.is_empty() { "/" } else { path }.to_owned(),
            index: self.index,
        }
    }

    /// Reads a binding in this scope.
    ///
    /// # Errors
    /// Returns an error for an invalid data path.
    pub fn read<'a>(
        &self,
        data: &'a DataModel,
        path: &str,
    ) -> Result<Option<&'a Value>, DataError> {
        data.get(&self.resolve_path(path))
    }

    /// Writes a binding in this scope.
    ///
    /// # Errors
    /// Returns an error if the data model rejects the path or value.
    pub fn write(&self, data: &mut DataModel, path: &str, value: Value) -> Result<(), DataError> {
        data.set(&self.resolve_path(path), value)
    }

    /// Returns one scope per item in the array at this path.
    ///
    /// An absent or non-array value produces an empty list.
    ///
    /// # Errors
    /// Returns an error for an invalid data path.
    pub fn collection(&self, data: &DataModel, path: &str) -> Result<Vec<Self>, DataError> {
        let collection = self.resolve_path(path);
        let Some(array) = data.get(&collection)?.and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        let base = collection.trim_end_matches('/');
        Ok((0..array.len())
            .map(|index| Self {
                path: format!("{base}/{index}"),
                index: Some(index),
            })
            .collect())
    }
}

/// The data scope for a local action function.
pub struct ActionContext<'a> {
    scope: &'a DataScope,
    data: &'a mut DataModel,
}

impl<'a> ActionContext<'a> {
    pub(crate) fn new(scope: &'a DataScope, data: &'a mut DataModel) -> Self {
        Self { scope, data }
    }

    /// Reads data at a relative or absolute path on the action's surface.
    ///
    /// # Errors
    /// Returns an error if the data model rejects the path.
    pub fn get(&self, path: &str) -> Result<Option<&Value>, DataError> {
        self.scope.read(self.data, path)
    }

    /// Writes data at a relative or absolute path on the action's surface.
    ///
    /// The writes from this function take effect when it returns successfully.
    ///
    /// # Errors
    /// Returns an error if the data model rejects the path or value.
    pub fn set(&mut self, path: &str, value: Value) -> Result<(), DataError> {
        self.scope.write(self.data, path, value)
    }
}

type PureHandler<'a> = dyn Fn(&JsonObject) -> Result<Value, String> + 'a;
pub(crate) type ActionHandler<'a> =
    dyn Fn(&JsonObject, &mut ActionContext<'_>) -> Result<Value, String> + 'a;

enum Handler<'a> {
    Pure(Box<PureHandler<'a>>),
    Action(Box<ActionHandler<'a>>),
}

/// Caller-supplied local functions, indexed by catalog ID and function name.
#[derive(Default)]
pub struct LocalFunctions<'a> {
    handlers: BTreeMap<String, BTreeMap<String, Handler<'a>>>,
}

impl<'a> LocalFunctions<'a> {
    pub(crate) fn contains(&self, catalog: &str, name: &str) -> bool {
        self.handler(catalog, name).is_some()
    }
    /// Registers a handler. A later registration replaces the handler for the same function.
    pub fn insert(
        &mut self,
        catalog_id: impl Into<String>,
        name: impl Into<String>,
        handler: impl Fn(&JsonObject) -> Result<Value, String> + 'a,
    ) {
        self.handlers
            .entry(catalog_id.into())
            .or_default()
            .insert(name.into(), Handler::Pure(Box::new(handler)));
    }

    /// Registers a local function with access to the action's surface data.
    ///
    /// The function can run only in a renderer action.
    pub fn insert_action(
        &mut self,
        catalog_id: impl Into<String>,
        name: impl Into<String>,
        handler: impl Fn(&JsonObject, &mut ActionContext<'_>) -> Result<Value, String> + 'a,
    ) {
        self.handlers
            .entry(catalog_id.into())
            .or_default()
            .insert(name.into(), Handler::Action(Box::new(handler)));
    }

    pub(crate) fn action(&self, catalog: &str, name: &str) -> Option<&ActionHandler<'a>> {
        match self.handler(catalog, name) {
            Some(Handler::Action(handler)) => Some(handler.as_ref()),
            _ => None,
        }
    }

    fn handler(&self, catalog: &str, name: &str) -> Option<&Handler<'a>> {
        self.handlers.get(catalog)?.get(name)
    }

    pub(crate) fn invoke(
        &self,
        catalog_id: &str,
        name: &str,
        args: &JsonObject,
    ) -> Result<Value, ResolveError> {
        let handler =
            self.handler(catalog_id, name)
                .ok_or_else(|| ResolveError::MissingHandler {
                    catalog_id: catalog_id.to_owned(),
                    name: name.to_owned(),
                })?;
        match handler {
            Handler::Pure(handler) => handler(args).map_err(ResolveError::LocalFunction),
            Handler::Action(_) => Err(ResolveError::ActionContextRequired(name.to_owned())),
        }
    }
}

/// A snapshot of one component instance.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedComponent {
    /// The component definition ID.
    pub id: String,
    /// The component type.
    pub component: String,
    /// The data scope for the instance.
    pub scope: DataScope,
    /// The resolved properties of the component.
    pub properties: JsonObject,
    /// The component instances referenced by this component.
    pub children: Vec<Self>,
}

pub(crate) struct Resolver<'a, 'f> {
    data: &'a DataModel,
    default_catalog: Option<&'a str>,
    catalogs: CatalogView<'a>,
    functions: &'a LocalFunctions<'f>,
    properties: Option<&'a BTreeMap<*const Value, PropertyKind>>,
}

impl<'a, 'f> Resolver<'a, 'f> {
    pub(crate) fn new(
        surface: &'a Surface,
        catalogs: CatalogView<'a>,
        functions: &'a LocalFunctions<'f>,
    ) -> Self {
        Self::with_context(
            surface.data_model(),
            surface.catalog_id(),
            catalogs,
            functions,
        )
    }

    pub(crate) fn with_context(
        data: &'a DataModel,
        default_catalog: Option<&'a str>,
        catalogs: CatalogView<'a>,
        functions: &'a LocalFunctions<'f>,
    ) -> Self {
        Self {
            data,
            default_catalog,
            catalogs,
            functions,
            properties: None,
        }
    }

    pub(crate) fn value(&self, scope: &DataScope, value: &Value) -> Result<Value, ResolveError> {
        self.resolve(scope, value, None, Nesting::default())
    }

    pub(crate) fn component_tree(
        &self,
        surface: &Surface,
    ) -> Result<Option<ResolvedComponent>, ResolveError> {
        let components = surface
            .components()
            .iter()
            .map(|component| (component.id.as_str(), component))
            .collect::<BTreeMap<_, _>>();
        components
            .get("root")
            .map(|root| self.component(&components, root, &DataScope::default(), 0))
            .transpose()
    }

    fn component(
        &self,
        components: &BTreeMap<&str, &Component>,
        component: &Component,
        scope: &DataScope,
        depth: usize,
    ) -> Result<ResolvedComponent, ResolveError> {
        if depth > MAX_DEPTH {
            return Err(ResolveError::Depth);
        }
        let catalog = self.catalog(component.catalog_id.as_deref())?;
        let schema = catalog.component_schema(&component.component);
        let raw = Value::Object(component.properties.clone().into_iter().collect());
        let shape = catalog.inspect_payload(schema, &raw);
        let resolver = Resolver {
            properties: Some(&shape.properties),
            ..*self
        };
        let mut properties = JsonObject::new();
        for (name, value) in raw.as_object().into_iter().flatten() {
            properties.insert(name.clone(), resolver.value(scope, value)?);
        }
        let mut children = Vec::new();
        for ChildReference { id, collection, .. } in shape.children {
            let Some(child) = components.get(id) else {
                continue;
            };
            if let Some(path) = collection {
                for item in scope.collection(self.data, path)? {
                    children.push(self.component(components, child, &item, depth + 1)?);
                }
            } else {
                children.push(self.component(components, child, scope, depth + 1)?);
            }
        }
        Ok(ResolvedComponent {
            id: component.id.clone(),
            component: component.component.clone(),
            scope: scope.clone(),
            properties,
            children,
        })
    }

    fn resolve(
        &self,
        scope: &DataScope,
        value: &Value,
        template_catalog: Option<&str>,
        depth: Nesting,
    ) -> Result<Value, ResolveError> {
        let kind = self
            .properties
            .and_then(|properties| properties.get(&std::ptr::from_ref(value)));
        if matches!(kind, Some(PropertyKind::Action)) {
            return Ok(value.clone());
        }
        let nested = depth.descend()?;
        let resolved = match value {
            Value::Object(properties) if properties.contains_key("@path") => {
                let binding: DataBinding = serde_json::from_value(value.clone())?;
                self.binding(scope, &binding.path)
            }
            Value::Object(properties) if properties.contains_key("@call") => {
                let call: FunctionCall = serde_json::from_value(value.clone())?;
                self.call(scope, call, template_catalog, depth)
            }
            Value::Object(properties) => Ok(Value::Object(self.object(
                scope,
                properties,
                template_catalog,
                nested,
            )?)),
            Value::Array(items) => Ok(Value::Array(
                items
                    .iter()
                    .map(|item| self.resolve(scope, item, template_catalog, nested))
                    .collect::<Result<_, _>>()?,
            )),
            value => Ok(value.clone()),
        }?;
        Ok(coerce(resolved, kind.copied()))
    }

    fn object(
        &self,
        scope: &DataScope,
        object: &JsonObject,
        inherited_catalog: Option<&str>,
        depth: Nesting,
    ) -> Result<JsonObject, ResolveError> {
        object
            .iter()
            .map(|(key, value)| {
                let key = if key.starts_with("@@") {
                    &key[1..]
                } else if key.starts_with('@') {
                    return Err(ResolveError::ReservedKey(key.clone()));
                } else {
                    key
                };
                Ok((
                    key.to_owned(),
                    self.resolve(scope, value, inherited_catalog, depth)?,
                ))
            })
            .collect()
    }

    fn binding(&self, scope: &DataScope, path: &str) -> Result<Value, ResolveError> {
        Ok(scope.read(self.data, path)?.cloned().unwrap_or(Value::Null))
    }

    fn catalog(&self, id: Option<&str>) -> Result<&Catalog, ResolveError> {
        let id = id
            .or(self.default_catalog)
            .ok_or(ResolveError::MissingCatalog)?;
        self.catalogs
            .get(id)
            .ok_or_else(|| ResolveError::UnknownCatalog(id.to_owned()))
    }

    fn call(
        &self,
        scope: &DataScope,
        call: FunctionCall,
        template_catalog: Option<&str>,
        depth: Nesting,
    ) -> Result<Value, ResolveError> {
        let nested = depth.call()?;
        validate_call_shape(&call)?;
        if call.name == "@index" {
            let index = scope.index.ok_or(ResolveError::IndexScope)?;
            let args = self.arguments(scope, &call, template_catalog, nested)?;
            return index_with_offset(index, &args);
        }

        let catalog = self.catalog(call.catalog_id.as_deref().or(template_catalog))?;
        if catalog.is_agent_only(&call.name) {
            return Err(ResolveError::AgentOnly(call.name));
        }
        if catalog.requires_activation(&call.name) {
            return Err(ResolveError::UserActivation(call.name));
        }
        if self.functions.action(catalog.id(), &call.name).is_some() {
            return Err(ResolveError::ActionContextRequired(call.name));
        }
        catalog.validate_function(&call)?;
        let args = self.catalog_arguments(scope, &call, catalog, template_catalog, nested)?;
        self.invoke_local(scope, catalog.id(), &call.name, &args, nested)
    }

    pub(crate) fn invoke_resolved(
        &self,
        scope: &DataScope,
        catalog_id: &str,
        name: &str,
        args: &JsonObject,
    ) -> Result<Value, ResolveError> {
        self.invoke_local(scope, catalog_id, name, args, Nesting::default().call()?)
    }

    pub(crate) fn template_part(
        &self,
        scope: &DataScope,
        catalog: &str,
        part: Expression,
    ) -> Result<String, ResolveError> {
        let value = match part {
            Expression::Literal(value) => value,
            Expression::Binding(path) => self.binding(scope, &path)?,
            Expression::Call(call) => {
                self.call(scope, call, Some(catalog), Nesting::default().call()?)?
            }
        };
        Ok(to_text(&value))
    }

    fn invoke_local(
        &self,
        scope: &DataScope,
        catalog_id: &str,
        name: &str,
        args: &JsonObject,
        nested: Nesting,
    ) -> Result<Value, ResolveError> {
        if name == "formatString" {
            let template = args
                .get("value")
                .and_then(Value::as_str)
                .ok_or(ResolveError::InvalidTemplate)?;
            let mut output = String::new();
            for part in parse_template(template)? {
                let result = match part {
                    Expression::Literal(value) => value,
                    Expression::Binding(path) => self.binding(scope, &path)?,
                    Expression::Call(call) => self.call(scope, call, Some(catalog_id), nested)?,
                };
                output.push_str(&to_text(&result));
            }
            Ok(Value::String(output))
        } else {
            self.functions.invoke(catalog_id, name, args)
        }
    }

    pub(crate) fn rpc_arguments(
        &self,
        call: &FunctionCall,
        catalog: &Catalog,
    ) -> Result<JsonObject, ResolveError> {
        self.catalog_arguments(
            &DataScope::default(),
            call,
            catalog,
            None,
            Nesting::default(),
        )
    }

    fn catalog_arguments(
        &self,
        scope: &DataScope,
        call: &FunctionCall,
        catalog: &Catalog,
        template_catalog: Option<&str>,
        depth: Nesting,
    ) -> Result<JsonObject, ResolveError> {
        let payload = serde_json::to_value(call)?;
        let shape = catalog.inspect_payload(catalog.function_schema(&call.name), &payload);
        let resolver = Resolver {
            properties: Some(&shape.properties),
            ..*self
        };
        payload.get("args").and_then(Value::as_object).map_or_else(
            || Ok(JsonObject::new()),
            |args| resolver.object(scope, args, template_catalog, depth),
        )
    }

    fn arguments(
        &self,
        scope: &DataScope,
        call: &FunctionCall,
        template_catalog: Option<&str>,
        depth: Nesting,
    ) -> Result<JsonObject, ResolveError> {
        call.args.as_ref().map_or_else(
            || Ok(JsonObject::new()),
            |args| self.object(scope, args, template_catalog, depth),
        )
    }
}

pub(crate) fn validate_call_shape(call: &FunctionCall) -> Result<(), ResolveError> {
    if call.name == "@index" {
        if call.catalog_id.is_some()
            || call
                .args
                .iter()
                .flat_map(serde_json::Map::keys)
                .any(|key| key != "offset")
        {
            return Err(ResolveError::InvalidIndex);
        }
    } else if !is_identifier(&call.name)
        || call
            .args
            .iter()
            .flat_map(serde_json::Map::keys)
            .any(|key| !is_identifier(key))
    {
        return Err(ResolveError::InvalidFunction(call.name.clone()));
    }
    Ok(())
}

pub(crate) fn index_with_offset(index: usize, args: &JsonObject) -> Result<Value, ResolveError> {
    let Some(offset) = args.get("offset") else {
        return Ok(Value::from(index));
    };
    if let Some(value) = offset.as_i64()
        && let Ok(index) = i64::try_from(index)
        && let Some(total) = index.checked_add(value)
    {
        return Ok(Value::from(total));
    }
    let integer = Number::from(index)
        .as_f64()
        .ok_or(ResolveError::InvalidIndex)?;
    let offset = offset.as_f64().ok_or(ResolveError::InvalidIndex)?;
    Number::from_f64(integer + offset)
        .map(Value::Number)
        .ok_or(ResolveError::InvalidIndex)
}

fn to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Number(value) if value.is_f64() => value.as_f64().map_or_else(
            || value.to_string(),
            |value| {
                if value == 0.0 {
                    "0".to_owned()
                } else {
                    value.to_string()
                }
            },
        ),
        value => value.to_string(),
    }
}

pub(crate) fn to_boolean(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::String(value) => value.eq_ignore_ascii_case("true"),
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::Object(value) => value.get("valid").and_then(Value::as_bool).unwrap_or(false),
        _ => false,
    }
}

pub(crate) fn coerce(value: Value, kind: Option<PropertyKind>) -> Value {
    match kind {
        Some(PropertyKind::String) => Value::String(to_text(&value)),
        Some(PropertyKind::Boolean) => Value::Bool(to_boolean(&value)),
        Some(PropertyKind::Number) => to_number(value),
        _ => value,
    }
}

fn to_number(value: Value) -> Value {
    if value.is_number() {
        return value;
    }
    value
        .as_str()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .and_then(Number::from_f64)
        .map_or_else(|| Value::from(0), Value::Number)
}

#[derive(Clone, Copy, Default)]
struct Nesting {
    value: usize,
    calls: usize,
}

impl Nesting {
    fn descend(self) -> Result<Self, ResolveError> {
        if self.value >= MAX_DEPTH {
            Err(ResolveError::Depth)
        } else {
            Ok(Self {
                value: self.value + 1,
                ..self
            })
        }
    }

    fn call(self) -> Result<Self, ResolveError> {
        if self.calls >= MAX_CALL_DEPTH {
            Err(ResolveError::FunctionDepth)
        } else {
            Ok(Self {
                calls: self.calls + 1,
                ..self.descend()?
            })
        }
    }
}

/// The reason a binding or expression could not be resolved.
#[derive(Debug, Error)]
pub enum ResolveError {
    /// The data model rejected a path.
    #[error(transparent)]
    Data(#[from] DataError),
    /// The surface operation failed.
    #[error(transparent)]
    Surface(#[from] SurfaceError),
    /// The catalog rejected the call.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// The expression syntax is invalid.
    #[error(transparent)]
    Parse(#[from] ParseError),
    /// A binding or function envelope is invalid.
    #[error("Invalid expression: {0}")]
    Envelope(#[from] serde_json::Error),
    /// The surface has no default catalog and the call names none.
    #[error("Surface has no default catalogId and the call names no catalog")]
    MissingCatalog,
    /// The catalog ID is unknown.
    #[error("Catalog not found: {0}")]
    UnknownCatalog(String),
    /// The call has an invalid function or argument name.
    #[error("Function and argument names must be valid UAX #31 identifiers: '{0}'")]
    InvalidFunction(String),
    /// The function can only run when the agent calls it.
    #[error("Function '{0}' is agentOnly")]
    AgentOnly(String),
    /// The function needs a user action.
    #[error("Function '{0}' requires user activation")]
    UserActivation(String),
    /// The function needs a renderer action context.
    #[error("Function '{0}' requires an action context")]
    ActionContextRequired(String),
    /// The function has no registered implementation.
    #[error("No local function '{name}' for catalog '{catalog_id}'")]
    MissingHandler {
        /// The resolved catalog ID.
        catalog_id: String,
        /// The function name.
        name: String,
    },
    /// The function implementation returned an error.
    #[error("Local function failed: {0}")]
    LocalFunction(String),
    /// The index function was used outside a collection.
    #[error("@index function can only be evaluated inside a collection template iteration scope.")]
    IndexScope,
    /// The index function has an invalid argument or catalog ID.
    #[error("Invalid @index arguments")]
    InvalidIndex,
    /// The template did not resolve to a string.
    #[error("formatString requires a string value")]
    InvalidTemplate,
    /// A plain object uses an unescaped reserved key.
    #[error("Unescaped reserved key '{0}'")]
    ReservedKey(String),
    /// The value or component exceeds the nesting limit.
    #[error("Global recursion limit exceeded during resolution")]
    Depth,
    /// The call exceeds the function nesting limit.
    #[error("Maximum functionCall depth exceeded")]
    FunctionDepth,
}
