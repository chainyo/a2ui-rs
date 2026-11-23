//! Validates catalog payloads and component relationships before applying a message.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::catalog::{Catalog, CatalogError};
use crate::protocol::{
    AgentToRenderer, AgentToRendererMessage, Component, Components, FunctionCall,
};
use crate::surface::{Surface, SurfaceError, SurfaceStore};

const MAX_COMPONENT_DEPTH: usize = 50;
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
    catalogs: BTreeMap<String, Catalog>,
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

    /// Registers a catalog.
    ///
    /// # Errors
    /// Returns an error if a catalog with the same ID is already registered.
    pub fn add_catalog(&mut self, catalog: Catalog) -> Result<(), ValidationError> {
        let id = catalog.id().to_owned();
        match self.catalogs.entry(id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(catalog);
                Ok(())
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                Err(ValidationError::DuplicateCatalog(entry.key().clone()))
            }
        }
    }

    /// Returns the active surface with this ID, if it exists.
    #[must_use]
    pub fn get(&self, surface_id: &str) -> Option<&Surface> {
        self.store.get(surface_id)
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
        message: AgentToRendererMessage,
        config: ValidationConfig,
    ) -> Result<(), ValidationError> {
        let AgentToRendererMessage::V1(operation) = &message;
        match operation {
            AgentToRenderer::CreateSurface(create) => {
                if self.get(&create.surface_id).is_some() {
                    return Err(SurfaceError::DuplicateSurface(create.surface_id.clone()).into());
                }
                if let Some(id) = create.catalog_id.as_deref() {
                    self.catalog(Some(id))?;
                }
                if let Some(components) = &create.components {
                    self.validate_batch(create.catalog_id.as_deref(), &[], components, config)?;
                }
            }
            AgentToRenderer::UpdateComponents(update) => {
                let surface = self
                    .get(&update.surface_id)
                    .ok_or_else(|| SurfaceError::UnknownSurface(update.surface_id.clone()))?;
                self.validate_batch(
                    surface.catalog_id(),
                    surface.components(),
                    &update.components,
                    config,
                )?;
            }
            _ => {}
        }
        self.store.apply(message)?;
        Ok(())
    }

    fn catalog(&self, id: Option<&str>) -> Result<&Catalog, ValidationError> {
        let id = id.ok_or(ValidationError::MissingCatalog)?;
        self.catalogs
            .get(id)
            .ok_or_else(|| ValidationError::UnknownCatalog(id.to_owned()))
    }

    fn validate_batch(
        &self,
        default_catalog: Option<&str>,
        current: &[Component],
        batch: &Components,
        config: ValidationConfig,
    ) -> Result<(), ValidationError> {
        let mut pending = BTreeMap::new();
        for component in batch.as_slice() {
            if pending.insert(component.id.as_str(), component).is_some() {
                return Err(SurfaceError::DuplicateComponent(component.id.clone()).into());
            }
        }

        for component in batch.as_slice() {
            let catalog = self.catalog(component.catalog_id.as_deref().or(default_catalog))?;
            check_entry(catalog.validate_component(component), config)?;
            for property in component.properties.values() {
                self.validate_calls(property, default_catalog, config, 0)?;
            }
        }

        let mut graph = BTreeMap::new();
        for component in current
            .iter()
            .filter(|item| !pending.contains_key(item.id.as_str()))
            .chain(pending.values().copied())
        {
            let catalog = self.catalog(component.catalog_id.as_deref().or(default_catalog))?;
            let schema = catalog.component_schema(&component.component);
            let payload = Value::Object(component.properties.clone().into_iter().collect());
            let children = catalog
                .child_references(schema, &payload)
                .into_iter()
                .map(|(id, field)| (id.to_owned(), field))
                .collect();
            graph.insert(
                component.id.as_str(),
                Node {
                    kind: &component.component,
                    schema,
                    children,
                },
            );
        }
        validate_graph(&graph, config)
    }

    fn validate_calls(
        &self,
        value: &Value,
        default_catalog: Option<&str>,
        config: ValidationConfig,
        depth: usize,
    ) -> Result<(), ValidationError> {
        let mut nodes = vec![(value, depth)];
        while let Some((value, depth)) = nodes.pop() {
            match value {
                Value::Object(object) if object.contains_key("@call") => {
                    if depth == MAX_FUNCTION_DEPTH {
                        return Err(ValidationError::FunctionDepth);
                    }
                    let call: FunctionCall = serde_json::from_value(value.clone())
                        .map_err(|error| ValidationError::InvalidCall(error.to_string()))?;
                    if call.name != "@index" {
                        let catalog =
                            self.catalog(call.catalog_id.as_deref().or(default_catalog))?;
                        if catalog.is_agent_only(&call.name) {
                            return Err(ValidationError::AgentOnlyFunction(call.name));
                        }
                        check_entry(catalog.validate_function(&call), config)?;
                    }
                    if let Some(args) = object.get("args").and_then(Value::as_object) {
                        nodes.extend(args.values().map(|value| (value, depth + 1)));
                    }
                }
                Value::Object(object) => nodes.extend(object.values().map(|value| (value, depth))),
                Value::Array(items) => nodes.extend(items.iter().map(|value| (value, depth))),
                _ => {}
            }
        }
        Ok(())
    }
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

struct Node<'a> {
    kind: &'a str,
    schema: &'a Value,
    children: BTreeMap<String, String>,
}

fn validate_graph(
    graph: &BTreeMap<&str, Node<'_>>,
    config: ValidationConfig,
) -> Result<(), ValidationError> {
    if let Some(root) = graph.get("root") {
        validate_parent("root", root, "Surface", "Surface")?;
    } else if !config.allow_missing_root {
        return Err(ValidationError::MissingRoot);
    }

    for (&id, node) in graph {
        for (child, field) in &node.children {
            if id == child {
                return Err(ValidationError::SelfReference(id.to_owned()));
            }
            if let Some(child_node) = graph.get(child.as_str()) {
                validate_parent(child, child_node, id, node.kind)?;
                if !allows(&node.schema["allowedChildren"], child_node.kind) {
                    let allowed = names(&node.schema["allowedChildren"]);
                    return Err(ValidationError::UnallowedChild {
                        parent: id.to_owned(),
                        child: child.clone(),
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
        Ok(())
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
) -> Result<(), ValidationError> {
    if allows(&node.schema["allowedParents"], kind) {
        Ok(())
    } else {
        let allowed = names(&node.schema["allowedParents"]);
        Err(ValidationError::UnallowedParent {
            child: id.to_owned(),
            parent: parent.to_owned(),
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
    #[error("No catalog ID or surface default")]
    MissingCatalog,
    /// The catalog ID is unknown.
    #[error("Unknown catalog '{0}'")]
    UnknownCatalog(String),
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
        /// The child type and the permitted types.
        message: String,
    },
    /// The graph exceeds the component depth limit.
    #[error("Global recursion limit exceeded in component graph")]
    ComponentDepth,
    /// A nested function call exceeds the function depth limit.
    #[error("Maximum functionCall depth exceeded")]
    FunctionDepth,
    /// The function call envelope is invalid.
    #[error("Invalid function call: {0}")]
    InvalidCall(String),
    /// A renderer expression calls a function reserved for the agent.
    #[error("Function '{0}' is agentOnly")]
    AgentOnlyFunction(String),
}
