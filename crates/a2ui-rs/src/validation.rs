//! Validates catalog payloads and component relationships before applying a message.

use std::collections::{BTreeMap, BTreeSet};

use jsonschema::paths::Location;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::catalog::{Catalog, CatalogError, CatalogRegistry, CatalogView};
use crate::data_model::{self, DataError};
use crate::expression::{Expression, is_identifier, parse_template};
use crate::protocol::{
    AgentToRenderer, AgentToRendererMessage, Component, Components, ErrorTarget, FunctionCall,
    GenericError, RendererCapabilities, RendererCapabilitiesV1, RendererDataModel, RendererError,
    RendererToAgent, RendererToAgentMessage, UpdateDataModel, ValidationCode,
    ValidationError as ProtocolValidationError,
};
use crate::resolution::{
    DataScope, LocalFunctions, ResolveError, ResolvedComponent, Resolver, validate_call_shape,
};
use crate::surface::{Surface, SurfaceError, SurfaceStore};

const MAX_COMPONENT_DEPTH: usize = 50;
const MAX_PAYLOAD_DEPTH: usize = 50;
const MAX_FUNCTION_DEPTH: usize = 5;

/// Controls which incomplete component graphs a renderer accepts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "The upstream rules are independent flags"
)]
pub struct ValidationConfig {
    /// Accept a graph before its root arrives.
    pub allow_missing_root: bool,
    /// Accept references to components that have yet to arrive.
    pub allow_dangling_references: bool,
    /// Accept components that are unreachable from the root.
    pub allow_orphan_components: bool,
    /// Accept component and function types that their catalog does not declare.
    pub allow_unknown_elements: bool,
}

impl ValidationConfig {
    /// Allows incomplete component graphs and unknown element types.
    pub const RELAXED: Self = Self {
        allow_missing_root: true,
        allow_dangling_references: true,
        allow_orphan_components: true,
        allow_unknown_elements: true,
    };
}

/// Validates incoming messages and stores the resulting surfaces.
#[derive(Default)]
pub struct MessageProcessor {
    store: SurfaceStore,
    catalogs: CatalogRegistry,
    topologies: BTreeMap<String, Topology>,
    config: ValidationConfig,
}

impl MessageProcessor {
    /// Creates a processor with the specified validation rules.
    #[must_use]
    pub fn new(config: ValidationConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// Registers a catalog. New surfaces can use the catalog.
    /// Active surfaces keep the catalogs available when they were created.
    ///
    /// # Errors
    /// Returns an error if a catalog with the same ID is already registered.
    pub fn add_catalog(&mut self, catalog: Catalog) -> Result<(), ValidationError> {
        self.catalogs
            .insert(catalog)
            .map_err(ValidationError::DuplicateCatalog)
    }

    pub(crate) fn current_catalogs(&self) -> CatalogView<'_> {
        self.catalogs.current()
    }

    pub(crate) fn surface_catalogs(&self, surface: &Surface) -> CatalogView<'_> {
        self.catalogs.view(surface.catalog_count())
    }

    /// Returns the catalogs registered for v1.0.
    ///
    /// Set `include_inline_catalogs` when the agent accepts inline definitions.
    #[must_use]
    pub fn renderer_capabilities(&self, include_inline_catalogs: bool) -> RendererCapabilities {
        let catalogs = self.catalogs.in_registration_order();
        RendererCapabilities {
            v1_0: RendererCapabilitiesV1 {
                supported_catalog_ids: catalogs
                    .iter()
                    .map(|catalog| catalog.id().to_owned())
                    .collect(),
                inline_catalogs: include_inline_catalogs.then(|| {
                    catalogs
                        .iter()
                        .map(|catalog| catalog.as_json().clone())
                        .collect()
                }),
            },
        }
    }

    /// Returns current data for surfaces with `sendDataModel` enabled.
    ///
    /// Returns `None` if no active surface requests data model reporting.
    #[must_use]
    pub fn renderer_data_model(&self) -> Option<RendererDataModel> {
        let surfaces: BTreeMap<_, _> = self
            .store
            .iter()
            .filter(|(_, surface)| surface.send_data_model())
            .map(|(id, surface)| (id.to_owned(), surface.data_model().root().clone()))
            .collect();
        (!surfaces.is_empty()).then_some(RendererDataModel {
            version: crate::ProtocolVersion::V1_0,
            surfaces,
        })
    }

    /// Returns the active surface with this ID, if it exists.
    #[must_use]
    pub fn get(&self, surface_id: &str) -> Option<&Surface> {
        self.store.get(surface_id)
    }

    /// Resolves a value against an active surface and data scope.
    ///
    /// # Errors
    /// Returns an error if the surface is absent or the expression is invalid.
    pub fn resolve_value(
        &self,
        surface_id: &str,
        scope: &DataScope,
        value: &Value,
        functions: &LocalFunctions<'_>,
    ) -> Result<Value, ResolveError> {
        let surface = self
            .get(surface_id)
            .ok_or_else(|| SurfaceError::UnknownSurface(surface_id.to_owned()))?;
        Resolver::new(surface, self.surface_catalogs(surface), functions).value(scope, value)
    }

    /// Resolves a snapshot of the component instances below the root.
    ///
    /// # Errors
    /// Returns an error if the surface is absent or a property cannot resolve.
    pub fn resolve_components(
        &self,
        surface_id: &str,
        functions: &LocalFunctions<'_>,
    ) -> Result<Option<ResolvedComponent>, ResolveError> {
        let surface = self
            .get(surface_id)
            .ok_or_else(|| SurfaceError::UnknownSurface(surface_id.to_owned()))?;
        Resolver::new(surface, self.surface_catalogs(surface), functions).component_tree(surface)
    }

    /// Writes a bound value to the surface data model.
    ///
    /// # Errors
    /// Returns an error if the surface is absent or the data model rejects the write.
    pub fn write_binding(
        &mut self,
        surface_id: &str,
        scope: &DataScope,
        path: &str,
        value: Value,
    ) -> Result<(), SurfaceError> {
        self.store.apply(AgentToRendererMessage::V1(
            AgentToRenderer::UpdateDataModel(UpdateDataModel {
                surface_id: surface_id.to_owned(),
                path: Some(scope.resolve_path(path)),
                value,
            }),
        ))
    }

    /// Applies one JSON message, a message list, or a `messages` wrapper.
    ///
    /// Each message applies in order. Processing stops at the first state error.
    /// Earlier successful messages stay applied.
    ///
    /// # Errors
    /// Returns an error if the input fails to decode or a message fails validation.
    /// A decode error leaves the entire batch unapplied.
    pub fn apply_json(&mut self, input: &Value) -> Result<(), ProcessError> {
        let messages = if let Some(messages) = input.as_array() {
            messages.as_slice()
        } else if let Some(object) = input.as_object()
            && object.contains_key("messages")
        {
            object
                .get("messages")
                .filter(|_| object.len() == 1)
                .and_then(Value::as_array)
                .ok_or(ProcessError::InvalidBatch)?
        } else {
            std::slice::from_ref(input)
        };
        let decoded: Vec<AgentToRendererMessage> = messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                serde_json::from_value(message.clone())
                    .map_err(|source| ProcessError::Envelope { index, source })
            })
            .collect::<Result<_, _>>()?;
        for (index, message) in decoded.into_iter().enumerate() {
            self.apply(message)
                .map_err(|source| ProcessError::Validation { index, source })?;
        }
        Ok(())
    }

    /// Validates and applies one protocol message.
    ///
    /// # Errors
    /// Returns an error if the message violates a surface, catalog, or topology rule.
    /// An error leaves surface state unchanged.
    pub fn apply(&mut self, message: AgentToRendererMessage) -> Result<(), ValidationError> {
        self.apply_with_config(message, self.config)
    }

    /// Validates and applies one protocol message with rules for this call.
    ///
    /// # Errors
    /// Returns an error if the message violates a surface, catalog, or topology rule.
    /// An error leaves surface state unchanged.
    pub fn apply_with_config(
        &mut self,
        mut message: AgentToRendererMessage,
        config: ValidationConfig,
    ) -> Result<(), ValidationError> {
        let AgentToRendererMessage::V1(operation) = &mut message;
        let mut topology = None;
        let mut deleted = None;
        match operation {
            AgentToRenderer::CreateSurface(create) => {
                if self.get(&create.surface_id).is_some() {
                    return Err(SurfaceError::DuplicateSurface(create.surface_id.clone()).into());
                }
                if let Some(id) = create.catalog_id.as_deref() {
                    self.current_catalogs().require(Some(id))?;
                }
                if let Some(components) = &mut create.components {
                    let catalogs = self.current_catalogs();
                    catalogs.normalize_components(create.catalog_id.as_deref(), components)?;
                    if let Some(index) = catalogs.validate_batch(
                        create.catalog_id.as_deref(),
                        None,
                        components,
                        config,
                    )? {
                        topology = Some((create.surface_id.clone(), index));
                    }
                }
            }
            AgentToRenderer::UpdateComponents(update) => {
                let surface = self
                    .get(&update.surface_id)
                    .ok_or_else(|| SurfaceError::UnknownSurface(update.surface_id.clone()))?;
                let catalogs = self.surface_catalogs(surface);
                catalogs.normalize_components(surface.catalog_id(), &mut update.components)?;
                let current = self.topologies.get(&update.surface_id);
                if let Some(index) = catalogs.validate_batch(
                    surface.catalog_id(),
                    Some((surface, current)),
                    &update.components,
                    config,
                )? {
                    topology = Some((update.surface_id.clone(), index));
                }
            }
            AgentToRenderer::UpdateDataModel(update) => {
                validate_nesting(&update.value, false)?;
                if self.get(&update.surface_id).is_none()
                    && let Err(source @ DataError::InvalidPointer(_)) =
                        data_model::parse_pointer(update.path.as_deref().unwrap_or("/"))
                {
                    return Err(SurfaceError::Data {
                        surface_id: update.surface_id.clone(),
                        source,
                    }
                    .into());
                }
            }
            AgentToRenderer::DeleteSurface(delete) => deleted = Some(delete.surface_id.clone()),
            _ => {}
        }
        self.store
            .apply_with_catalog_count(message, self.catalogs.len())?;
        if let Some((id, index)) = topology {
            self.topologies.insert(id, index);
        }
        if let Some(id) = deleted {
            self.topologies.remove(&id);
        }
        Ok(())
    }
}

impl CatalogView<'_> {
    fn require(&self, id: Option<&str>) -> Result<&Catalog, ValidationError> {
        let id = id.ok_or(ValidationError::MissingCatalog)?;
        self.get(id)
            .ok_or_else(|| ValidationError::UnknownCatalog(id.to_owned()))
    }

    fn normalize_components(
        &self,
        default: Option<&str>,
        components: &mut Components,
    ) -> Result<(), ValidationError> {
        for (index, component) in components.as_mut_slice().iter_mut().enumerate() {
            let catalog = component
                .catalog_id
                .as_deref()
                .or(default)
                .and_then(|id| self.get(id));
            if component.properties.contains_key("catalogId")
                && !catalog.is_some_and(|catalog| {
                    catalog.has_custom_component_property(&component.component, "catalogId")
                })
                && let Some(id) = legacy_field(
                    component,
                    index,
                    "catalogId",
                    component.catalog_id.is_some(),
                )?
            {
                component.catalog_id = Some(id);
            }
            let catalog = component
                .catalog_id
                .as_deref()
                .or(default)
                .and_then(|id| self.get(id));
            if component.properties.contains_key("metadata")
                && !catalog.is_some_and(|catalog| {
                    catalog.has_custom_component_property(&component.component, "metadata")
                })
                && let Some(metadata) =
                    legacy_field(component, index, "metadata", component.metadata.is_some())?
            {
                component.metadata = Some(metadata);
            }
            if component.properties.contains_key("accessibility")
                && !catalog.is_some_and(|catalog| {
                    catalog.has_custom_component_property(&component.component, "accessibility")
                })
                && let Some(accessibility) = legacy_field(
                    component,
                    index,
                    "accessibility",
                    component.accessibility.is_some(),
                )?
            {
                component.accessibility = Some(accessibility);
            }
        }
        Ok(())
    }

    fn validate_batch(
        &self,
        default_catalog: Option<&str>,
        current: Option<(&Surface, Option<&Topology>)>,
        batch: &Components,
        config: ValidationConfig,
    ) -> Result<Option<Topology>, ValidationError> {
        let mut ids = BTreeSet::new();
        for component in batch.as_slice() {
            if !ids.insert(component.id.as_str()) {
                return Err(SurfaceError::DuplicateComponent(component.id.clone()).into());
            }
        }

        self.validate_payloads(default_catalog, batch, config)?;
        let mut graph = BTreeMap::new();
        for (index, component) in batch.as_slice().iter().enumerate() {
            graph.insert(
                component.id.as_str(),
                self.graph_node(default_catalog, component, Some(index))?,
            );
        }
        if let Some((surface, index)) = current {
            if index.is_some_and(|index| index.accepts_leaves(self, surface, &graph, config)) {
                return Ok(None);
            }
            for component in surface.components() {
                if !ids.contains(component.id.as_str()) {
                    graph.insert(
                        component.id.as_str(),
                        self.graph_node(default_catalog, component, None)?,
                    );
                }
            }
        }
        let can_attach_leaf = validate_graph(&graph, config)?;
        Ok(Some(Topology::new(&graph, config, can_attach_leaf)))
    }

    fn graph_node<'a>(
        &'a self,
        default_catalog: Option<&str>,
        component: &'a Component,
        index: Option<usize>,
    ) -> Result<Node<'a>, ValidationError> {
        let catalog = self.require(component.catalog_id.as_deref().or(default_catalog))?;
        let schema = catalog.component_schema(&component.component);
        let payload = Value::Object(component.properties.clone().into_iter().collect());
        let children = catalog
            .child_references(schema, &payload)
            .into_iter()
            .map(|(id, field)| (id.to_owned(), field))
            .collect();
        Ok(Node {
            kind: &component.component,
            schema,
            children,
            index,
        })
    }

    fn validate_payloads(
        &self,
        default_catalog: Option<&str>,
        batch: &Components,
        config: ValidationConfig,
    ) -> Result<(), ValidationError> {
        for (index, component) in batch.as_slice().iter().enumerate() {
            let base = component_path(index);
            if !is_identifier(&component.id) {
                return Err(ValidationError::InvalidComponentId {
                    id: component.id.clone(),
                    path: format!("{base}/id"),
                });
            }
            if !is_identifier(&component.component) {
                return Err(ValidationError::InvalidComponentType {
                    name: component.component.clone(),
                    path: format!("{base}/component"),
                });
            }
            for property in component.properties.values() {
                validate_nesting(property, true)?;
            }
            let catalog = self.require(component.catalog_id.as_deref().or(default_catalog))?;
            let result = catalog.validate_component_at(component, Some(&base));
            match result {
                Err(CatalogError::UnknownEntry(name)) if !config.allow_unknown_elements => {
                    return Err(ValidationError::UnknownComponent {
                        name,
                        path: format!("{base}/component"),
                    });
                }
                result => check_entry(result, config)?,
            }
            let base = Location::new().join("components").join(index);
            for (name, property) in &component.properties {
                self.validate_calls(
                    property,
                    default_catalog,
                    None,
                    config,
                    0,
                    &base.join(name.as_str()),
                )?;
            }
            if let Some(accessibility) = &component.accessibility {
                let value = serde_json::to_value(accessibility)
                    .map_err(|error| ValidationError::InvalidCall(error.to_string()))?;
                self.validate_calls(
                    &value,
                    default_catalog,
                    None,
                    config,
                    0,
                    &base.join("@accessibility"),
                )?;
            }
        }
        Ok(())
    }

    fn validate_calls(
        &self,
        value: &Value,
        default_catalog: Option<&str>,
        template_catalog: Option<&str>,
        config: ValidationConfig,
        depth: usize,
        path: &Location,
    ) -> Result<(), ValidationError> {
        let mut nodes = vec![(value, path.clone())];
        while let Some((value, path)) = nodes.pop() {
            match value {
                Value::Object(object) if object.contains_key("@call") => {
                    let call: FunctionCall = serde_json::from_value(value.clone())
                        .map_err(|error| ValidationError::InvalidCall(error.to_string()))?;
                    self.validate_call(
                        &call,
                        default_catalog,
                        template_catalog,
                        config,
                        depth,
                        &path,
                    )?;
                }
                Value::Object(object) => {
                    if let Some(binding) = object.get("@path").and_then(Value::as_str) {
                        let absolute = DataScope::default().resolve_path(binding);
                        data_model::parse_pointer(&absolute).map_err(|source| {
                            ValidationError::BindingPath {
                                path: path.join("@path").to_string(),
                                source,
                            }
                        })?;
                    }
                    nodes.extend(
                        object
                            .iter()
                            .map(|(key, value)| (value, path.join(key.as_str()))),
                    );
                }
                Value::Array(items) => nodes.extend(
                    items
                        .iter()
                        .enumerate()
                        .map(|(index, value)| (value, path.join(index))),
                ),
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_call(
        &self,
        call: &FunctionCall,
        default_catalog: Option<&str>,
        template_catalog: Option<&str>,
        config: ValidationConfig,
        depth: usize,
        path: &Location,
    ) -> Result<(), ValidationError> {
        if depth >= MAX_FUNCTION_DEPTH {
            return Err(ValidationError::FunctionDepth);
        }
        validate_call_shape(call)
            .map_err(|error| ValidationError::InvalidCall(error.to_string()))?;
        if call.name != "@index" {
            let catalog = self.require(
                call.catalog_id
                    .as_deref()
                    .or(template_catalog)
                    .or(default_catalog),
            )?;
            if catalog.is_agent_only(&call.name) {
                return Err(ValidationError::AgentOnlyFunction {
                    name: call.name.clone(),
                    path: path.to_string(),
                });
            }
            match catalog.validate_function_at(call, Some(path.as_str())) {
                Err(CatalogError::UnknownEntry(name)) if !config.allow_unknown_elements => {
                    return Err(ValidationError::UnknownFunction {
                        name,
                        path: path.to_string(),
                    });
                }
                result => check_entry(result, config)?,
            }
            if call.name == "formatString"
                && let Some(template) = call
                    .args
                    .as_ref()
                    .and_then(|args| args.get("value"))
                    .and_then(Value::as_str)
            {
                for part in parse_template(template)? {
                    if let Expression::Call(call) = part {
                        self.validate_call(
                            &call,
                            default_catalog,
                            Some(catalog.id()),
                            config,
                            depth + 1,
                            &path.join("args").join("value"),
                        )?;
                    }
                }
            }
        }
        for (name, value) in call.args.iter().flatten() {
            self.validate_calls(
                value,
                default_catalog,
                template_catalog,
                config,
                depth + 1,
                &path.join("args").join(name.as_str()),
            )?;
        }
        Ok(())
    }
}

fn validate_nesting(value: &Value, expressions: bool) -> Result<(), ValidationError> {
    let mut pending = vec![(value, 0, 0)];
    while let Some((value, depth, calls)) = pending.pop() {
        if depth > MAX_PAYLOAD_DEPTH {
            return Err(if expressions {
                ValidationError::PayloadDepth
            } else {
                ValidationError::DataModelDepth
            });
        }
        match value {
            Value::Object(object) => {
                let calls = calls + usize::from(expressions && object.contains_key("@call"));
                if calls > MAX_FUNCTION_DEPTH {
                    return Err(ValidationError::FunctionDepth);
                }
                pending.extend(object.values().map(|value| (value, depth + 1, calls)));
            }
            Value::Array(array) => {
                pending.extend(array.iter().map(|value| (value, depth + 1, calls)));
            }
            _ => {}
        }
    }
    Ok(())
}

fn legacy_field<T: serde::de::DeserializeOwned>(
    component: &mut Component,
    index: usize,
    name: &str,
    has_canonical: bool,
) -> Result<Option<T>, CatalogError> {
    let Some(value) = component.properties.remove(name) else {
        return Ok(None);
    };
    let invalid = |message: String| CatalogError::InvalidPayload {
        name: component.component.clone(),
        message,
        path: Some(format!("/components/{index}/{name}")),
    };
    if has_canonical {
        return Err(invalid(format!(
            "Component specifies both '{name}' and '@{name}'"
        )));
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| invalid(format!("Invalid {name}: {error}")))
}

fn check_entry(
    result: Result<(), CatalogError>,
    config: ValidationConfig,
) -> Result<(), ValidationError> {
    match result {
        Err(CatalogError::UnknownEntry(_)) if config.allow_unknown_elements => Ok(()),
        result => result.map_err(Into::into),
    }
}

struct Topology {
    parents: BTreeMap<String, Vec<String>>,
    containers: BTreeSet<String>,
    config: ValidationConfig,
    can_attach_leaf: bool,
}

impl Topology {
    fn new(
        graph: &BTreeMap<&str, Node<'_>>,
        config: ValidationConfig,
        can_attach_leaf: bool,
    ) -> Self {
        let mut parents = BTreeMap::<String, Vec<String>>::new();
        let mut containers = BTreeSet::new();
        for (&id, node) in graph {
            if !node.children.is_empty() {
                containers.insert(id.to_owned());
            }
            for child in node.children.keys() {
                parents
                    .entry(child.clone())
                    .or_default()
                    .push(id.to_owned());
            }
        }
        Self {
            parents,
            containers,
            config,
            can_attach_leaf,
        }
    }

    fn accepts_leaves(
        &self,
        catalogs: &CatalogView<'_>,
        surface: &Surface,
        batch: &BTreeMap<&str, Node<'_>>,
        config: ValidationConfig,
    ) -> bool {
        if config != self.config {
            return false;
        }
        let require_parent = !config.allow_orphan_components && surface.component("root").is_some();
        // A leaf cannot make a cycle or change an existing path.
        // A new leaf can extend a path by one edge.
        // Later leaf updates cannot extend the same path again.
        batch.iter().all(|(&id, node)| {
            if id == "root" || !node.children.is_empty() || self.containers.contains(id) {
                return false;
            }
            let parents = self.parents.get(id);
            if surface.component(id).is_none()
                && (!self.can_attach_leaf || (require_parent && parents.is_none()))
            {
                return false;
            }
            parents.into_iter().flatten().all(|parent| {
                surface.component(parent).is_some_and(|component| {
                    component
                        .catalog_id
                        .as_deref()
                        .or(surface.catalog_id())
                        .and_then(|id| catalogs.get(id))
                        .is_some_and(|catalog| {
                            let schema = catalog.component_schema(&component.component);
                            allows(&node.schema["allowedParents"], &component.component)
                                && allows(&schema["allowedChildren"], node.kind)
                        })
                })
            })
        })
    }
}

struct Node<'a> {
    kind: &'a str,
    schema: &'a Value,
    children: BTreeMap<String, (String, Location)>,
    index: Option<usize>,
}

fn component_path(index: usize) -> String {
    format!("/components/{index}")
}

impl Node<'_> {
    fn path(&self, field: &str) -> Option<String> {
        self.index
            .map(|index| format!("/components/{index}{field}"))
    }
}

fn validate_graph(
    graph: &BTreeMap<&str, Node<'_>>,
    config: ValidationConfig,
) -> Result<bool, ValidationError> {
    if let Some(root) = graph.get("root") {
        validate_parent("root", root, "Surface", "Surface", || root.path(""))?;
    } else if !config.allow_missing_root {
        return Err(ValidationError::MissingRoot);
    }

    for (&id, node) in graph {
        for (child, (field, pointer)) in &node.children {
            if id == child {
                return Err(ValidationError::SelfReference(id.to_owned()));
            }
            if let Some(child_node) = graph.get(child.as_str()) {
                validate_parent(child, child_node, id, node.kind, || {
                    child_node.path("").or_else(|| node.path(pointer.as_str()))
                })?;
                if !allows(&node.schema["allowedChildren"], child_node.kind) {
                    let allowed = names(&node.schema["allowedChildren"]);
                    return Err(ValidationError::UnallowedChild {
                        parent: id.to_owned(),
                        child: child.clone(),
                        path: node
                            .path(pointer.as_str())
                            .or_else(|| child_node.path(""))
                            .unwrap_or_else(|| "/components".to_owned()),
                        message: format!(
                            "Container '{id}' ({}) cannot contain child '{child}' ({}). Allowed children: {allowed}.",
                            node.kind, child_node.kind
                        ),
                    });
                }
            } else if !config.allow_dangling_references {
                return Err(ValidationError::DanglingReference {
                    parent: id.to_owned(),
                    child: child.clone(),
                    field: field.clone(),
                });
            }
        }
    }

    let mut heights = BTreeMap::new();
    let mut active = BTreeSet::new();
    let mut orphan = None;
    if graph.contains_key("root") {
        walk(graph, "root", 0, &mut heights, &mut active)?;
        if !config.allow_orphan_components {
            orphan = graph.keys().find(|id| !heights.contains_key(*id)).copied();
        }
    }
    for &id in graph.keys() {
        walk(graph, id, 0, &mut heights, &mut active)?;
    }
    if let Some(id) = orphan {
        Err(ValidationError::Orphan(id.to_owned()))
    } else {
        Ok(heights
            .values()
            .all(|&height| height <= MAX_COMPONENT_DEPTH))
    }
}

fn allows(schema: &Value, kind: &str) -> bool {
    schema
        .as_array()
        .is_none_or(|allowed| allowed.iter().any(|item| item.as_str() == Some(kind)))
}

fn names(schema: &Value) -> String {
    let names = schema
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{names}]")
}

fn validate_parent(
    id: &str,
    node: &Node<'_>,
    parent: &str,
    kind: &str,
    path: impl FnOnce() -> Option<String>,
) -> Result<(), ValidationError> {
    if allows(&node.schema["allowedParents"], kind) {
        Ok(())
    } else {
        let allowed = names(&node.schema["allowedParents"]);
        Err(ValidationError::UnallowedParent {
            child: id.to_owned(),
            parent: parent.to_owned(),
            path: path().unwrap_or_else(|| "/components".to_owned()),
            message: format!(
                "Component '{id}' ({}) cannot be placed under parent '{parent}' ({kind}). Allowed parents: {allowed}.",
                node.kind
            ),
        })
    }
}

fn walk<'a>(
    graph: &'a BTreeMap<&str, Node<'_>>,
    id: &'a str,
    depth: usize,
    heights: &mut BTreeMap<&'a str, usize>,
    active: &mut BTreeSet<&'a str>,
) -> Result<usize, ValidationError> {
    if depth > MAX_COMPONENT_DEPTH {
        return Err(ValidationError::ComponentDepth);
    }
    if let Some(&height) = heights.get(id) {
        return if depth + height - 1 > MAX_COMPONENT_DEPTH {
            Err(ValidationError::ComponentDepth)
        } else {
            Ok(height)
        };
    }
    if !active.insert(id) {
        return Err(ValidationError::CircularReference(id.to_owned()));
    }
    let mut height = 1;
    if let Some(node) = graph.get(id) {
        for child in node
            .children
            .keys()
            .filter(|child| graph.contains_key(child.as_str()))
        {
            height = height.max(1 + walk(graph, child, depth + 1, heights, active)?);
        }
    }
    active.remove(id);
    heights.insert(id, height);
    Ok(height)
}

/// An error from an incoming JSON message or batch.
#[derive(Debug, Error)]
pub enum ProcessError {
    /// The batch wrapper must contain only a `messages` array.
    #[error("Invalid messages batch")]
    InvalidBatch,
    /// The message does not have a valid v1.0 envelope.
    #[error("Invalid message at index {index}: {source}")]
    Envelope {
        /// The zero-based message index.
        index: usize,
        /// The envelope decode error.
        #[source]
        source: serde_json::Error,
    },
    /// The processor rejected a decoded message.
    #[error("Rejected message at index {index}: {source}")]
    Validation {
        /// The zero-based message index.
        index: usize,
        /// The state validation error.
        #[source]
        source: ValidationError,
    },
}

/// The reason a message failed validation.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// A surface state operation failed.
    #[error(transparent)]
    Surface(#[from] SurfaceError),
    /// A catalog or payload check failed.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// The catalog ID is already registered.
    #[error("Catalog '{0}' is already registered")]
    DuplicateCatalog(String),
    /// The component or function names no catalog and the surface has no default.
    #[error("Surface has no default catalogId")]
    MissingCatalog,
    /// The catalog ID is unknown.
    #[error("Catalog not found: {0}")]
    UnknownCatalog(String),
    /// A component ID violates the protocol identifier rules.
    #[error("Component id '{id}' must be a valid UAX #31 identifier")]
    InvalidComponentId {
        /// The invalid component ID.
        id: String,
        /// The field in the incoming component batch.
        path: String,
    },
    /// A component type violates the protocol identifier rules.
    #[error("Component type '{name}' must be a valid UAX #31 identifier")]
    InvalidComponentType {
        /// The component type.
        name: String,
        /// The field in the incoming component batch.
        path: String,
    },
    /// The catalog does not define the component type.
    #[error("Catalog does not define component '{name}'")]
    UnknownComponent {
        /// The component type.
        name: String,
        /// The field in the incoming component batch.
        path: String,
    },
    /// The graph has no root.
    #[error("Missing root component")]
    MissingRoot,
    /// A child reference has no matching component.
    #[error(
        "Dangling reference: component '{parent}' references non-existent component '{child}' in field '{field}'"
    )]
    DanglingReference {
        /// The parent ID.
        parent: String,
        /// The missing child ID.
        child: String,
        /// The field that contains the reference.
        field: String,
    },
    /// A component references itself.
    #[error("Self-reference detected at component '{0}'")]
    SelfReference(String),
    /// The graph contains a cycle.
    #[error("Circular component reference: Circular reference detected at '{0}'")]
    CircularReference(String),
    /// A component is unreachable from the root.
    #[error("Component '{0}' is not reachable from 'root'")]
    Orphan(String),
    /// The catalog disallows this parent.
    #[error("{message}")]
    UnallowedParent {
        /// The parent ID.
        parent: String,
        /// The field in the incoming component batch.
        path: String,
        /// The child ID.
        child: String,
        /// The parent type and the permitted types.
        message: String,
    },
    /// The catalog disallows this child.
    #[error("{message}")]
    UnallowedChild {
        /// The parent ID.
        parent: String,
        /// The child ID.
        child: String,
        /// The field in the incoming component batch.
        path: String,
        /// The child type and the permitted types.
        message: String,
    },
    /// The graph exceeds the component depth limit.
    #[error("Global recursion limit exceeded in component graph")]
    ComponentDepth,
    /// A component property exceeds the nesting limit.
    #[error("Global recursion limit exceeded in component properties")]
    PayloadDepth,
    /// An update exceeds the data model nesting limit.
    #[error("Global recursion limit exceeded in data model update")]
    DataModelDepth,
    /// A data binding has an invalid path.
    #[error("{source}")]
    BindingPath {
        /// The binding location in the incoming component batch.
        path: String,
        /// The data model path error.
        #[source]
        source: DataError,
    },
    /// The catalog does not declare the function.
    #[error("Unrecognized function '{name}'")]
    UnknownFunction {
        /// The function name.
        name: String,
        /// The path of the call in the message.
        path: String,
    },
    /// A nested function call exceeds the function depth limit.
    #[error("Maximum functionCall depth exceeded")]
    FunctionDepth,
    /// The function call envelope is invalid.
    #[error("Invalid function call: {0}")]
    InvalidCall(String),
    /// A renderer expression calls a function reserved for the agent.
    #[error("Function '{name}' is agentOnly")]
    AgentOnlyFunction {
        /// The function name.
        name: String,
        /// The call location in the incoming component batch.
        path: String,
    },
    /// The local expression template is invalid.
    #[error(transparent)]
    Expression(#[from] crate::expression::ParseError),
}

impl ValidationError {
    /// Builds a renderer error for supported surface, graph, catalog, and data errors.
    ///
    /// Supply the surface ID from the rejected message.
    /// Returns `None` for other validation failures.
    #[must_use]
    pub fn renderer_error(&self, surface_id: &str) -> Option<RendererToAgentMessage> {
        let field = match self {
            Self::InvalidComponentId { path, .. }
            | Self::InvalidComponentType { path, .. }
            | Self::UnknownComponent { path, .. }
            | Self::UnknownFunction { path, .. }
            | Self::AgentOnlyFunction { path, .. }
            | Self::BindingPath { path, .. }
            | Self::Catalog(CatalogError::InvalidPayload {
                path: Some(path), ..
            }) => Some((
                ValidationCode::ValidationFailed,
                path.as_str(),
                self.to_string(),
            )),
            Self::UnallowedParent { path, .. } => Some((
                ValidationCode::UnallowedParent,
                path.as_str(),
                self.to_string(),
            )),
            Self::UnallowedChild { path, .. } => Some((
                ValidationCode::UnallowedChild,
                path.as_str(),
                self.to_string(),
            )),
            Self::Surface(SurfaceError::Data {
                source: source @ DataError::InvalidPointer(_),
                ..
            }) => Some((
                ValidationCode::ValidationFailed,
                "/path",
                format!("Invalid updateDataModel path. {source}"),
            )),
            _ => None,
        };
        let error = if let Some((code, path, message)) = field {
            RendererError::Validation(ProtocolValidationError {
                code,
                message,
                surface_id: surface_id.to_owned(),
                path: path.to_owned(),
            })
        } else {
            let code = match self {
                Self::Surface(
                    SurfaceError::DuplicateSurface(_)
                    | SurfaceError::DuplicateComponent(_)
                    | SurfaceError::UnknownSurface(_),
                )
                | Self::DanglingReference { .. }
                | Self::Orphan(_) => "INTEGRITY_ERROR",
                Self::SelfReference(_)
                | Self::CircularReference(_)
                | Self::PayloadDepth
                | Self::DataModelDepth => "RECURSION_LIMIT_EXCEEDED",
                Self::MissingCatalog | Self::UnknownCatalog(_) => "CATALOG_NOT_FOUND",
                Self::Surface(SurfaceError::Data { .. }) => "DATA_ERROR",
                _ => return None,
            };
            RendererError::Generic(GenericError {
                code: code.to_owned(),
                message: self.to_string(),
                target: ErrorTarget::SurfaceId(surface_id.to_owned()),
                extra: BTreeMap::new(),
            })
        };
        Some(RendererToAgentMessage::V1(RendererToAgent::Error(error)))
    }
}
