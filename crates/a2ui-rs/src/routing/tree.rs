use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::Value;
use thiserror::Error;

use super::{
    Evaluation, EvaluationId, EvaluationState, RoutingError, RoutingSession, RoutingUpdate, Task,
};
use crate::catalog::{Catalog, PropertyKind};
use crate::data_model::DataModel;
use crate::protocol::Metadata;
use crate::resolution::DataScope;
use crate::rpc::ReceiveError;
use crate::surface::Surface;
use crate::validation::MessageProcessor;

mod action;

pub use action::{ActionDispatchOptions, NodeActionError};

const VALIDATION_PROPERTIES: [&str; 3] = ["isValid", "validationErrors", "validationResults"];

/// Identifies one mounted node in a routing session.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NodeId(u64);

/// The current value and write path of a node property.
#[derive(Clone, Debug, PartialEq)]
pub struct MountedProperty {
    /// The resolved value. A pending or failed expression can have a null value.
    pub value: Value,
    /// The absolute data path for a writable binding.
    pub binding_path: Option<String>,
    /// Whether the catalog declares this property as an action.
    pub is_action: bool,
}

/// A child instance and the field that refers to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountedChild {
    /// The field path. For a template, the path ends with the item index.
    pub field: String,
    /// The mounted child ID.
    pub id: NodeId,
}

/// The current state of one mounted component instance.
#[derive(Clone, Debug, PartialEq)]
pub struct MountedNode {
    /// The mounted instance ID.
    pub id: NodeId,
    /// The component definition ID.
    pub component_id: String,
    /// The component type from the catalog.
    pub component: String,
    /// The data scope for this instance.
    pub scope: DataScope,
    /// The component definition's `@metadata` envelope.
    pub metadata: Option<Metadata>,
    /// The resolved properties, including child declarations and check results.
    pub properties: BTreeMap<String, MountedProperty>,
    /// The mounted children in order.
    pub children: Vec<MountedChild>,
}

/// The change to a mounted node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeChangeKind {
    /// The node is available with its initial properties and metadata.
    Mounted,
    /// The properties or child list of a retained node changed.
    Updated,
    /// The component definition's `@metadata` changed or cleared.
    MetadataUpdated,
    /// The node and its property evaluations were removed.
    Disposed,
}

/// A node change reported by the routing session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeChange {
    /// The surface that contains or previously contained the node.
    pub surface_id: String,
    /// The mounted instance ID.
    pub id: NodeId,
    /// The type of change.
    pub kind: NodeChangeKind,
}

/// The reason a write to a node property failed.
#[derive(Debug, Error)]
pub enum NodeWriteError {
    /// The surface does not have a mounted node with this ID.
    #[error("Node {0:?} is not mounted on this surface")]
    UnknownNode(NodeId),
    /// The node property has no writable binding.
    #[error("Node property '{0}' has no writable binding")]
    ReadOnly(String),
    /// The processor rejected the data write.
    #[error(transparent)]
    Receive(#[from] ReceiveError),
}

/// The mounted root and descendants for one surface.
#[derive(Default)]
pub struct NodeTree {
    root: Option<NodeId>,
    entries: BTreeMap<NodeId, Entry>,
    definitions: BTreeMap<String, Definition>,
    has_collections: bool,
    generation: u64,
    template_errors: BTreeMap<(NodeId, String), RoutingError>,
    reported_errors: BTreeMap<(NodeId, String), RoutingError>,
}

impl NodeTree {
    /// Returns the root node ID, if the surface has a root.
    #[must_use]
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    /// Returns a current node by its mounted ID.
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&MountedNode> {
        self.entries.get(&id).map(|entry| &entry.node)
    }
}

#[derive(Default)]
pub(super) struct MountedTrees {
    active: BTreeMap<String, NodeTree>,
    dirty: BTreeSet<String>,
    changes: Vec<NodeChange>,
    next_id: u64,
}

impl MountedTrees {
    pub(super) fn mark_dirty(&mut self, surface: String) {
        if self.active.contains_key(&surface) {
            self.dirty.insert(surface);
        }
    }

    pub(super) fn clear_surface(&mut self, surface: &str) {
        self.dirty.remove(surface);
        if let Some(tree) = self.active.get_mut(surface) {
            let previous = std::mem::take(tree);
            record_disposals(&mut self.changes, surface, previous.entries);
        }
    }

    pub(super) fn clear(&mut self) {
        self.dirty.clear();
        for (surface, tree) in std::mem::take(&mut self.active) {
            record_disposals(&mut self.changes, &surface, tree.entries);
        }
    }

    pub(super) fn publish(
        &mut self,
        processor: &MessageProcessor,
        evaluations: &BTreeMap<EvaluationId, Evaluation>,
        update: &mut RoutingUpdate,
    ) {
        for (surface, tree) in &mut self.active {
            if tree.reported_errors != tree.template_errors {
                update.errors.extend(
                    tree.template_errors
                        .iter()
                        .filter(|(key, error)| tree.reported_errors.get(*key) != Some(error))
                        .map(|(_, error)| error.clone().wire(surface)),
                );
                tree.reported_errors.clone_from(&tree.template_errors);
            }
            let model = processor.get(surface);
            for (&id, entry) in &mut tree.entries {
                let validation = entry
                    .validation_properties(evaluations)
                    .unwrap_or_else(|error| {
                        update
                            .errors
                            .push(RoutingError::expression(error).wire(surface));
                        Vec::new()
                    });
                let mounted = !entry.published;
                if mounted
                    || entry.next_children.is_some()
                    || entry.properties_changed(evaluations, &validation)
                {
                    let kind = if mounted {
                        NodeChangeKind::Mounted
                    } else {
                        NodeChangeKind::Updated
                    };
                    self.changes.push(NodeChange {
                        surface_id: surface.clone(),
                        id,
                        kind,
                    });
                    entry.publish_properties(evaluations, validation);
                }

                let metadata = model
                    .and_then(|model| model.component(&entry.node.component_id))
                    .and_then(|component| component.metadata.as_ref());
                if entry.node.metadata.as_ref() != metadata {
                    entry.node.metadata = metadata.cloned();
                    if !mounted {
                        self.changes.push(NodeChange {
                            surface_id: surface.clone(),
                            id,
                            kind: NodeChangeKind::MetadataUpdated,
                        });
                    }
                }
                entry.published = true;
            }
        }
        update.nodes = std::mem::take(&mut self.changes);
    }
}

impl RoutingSession<'_> {
    /// Tracks the root and descendants of a surface.
    ///
    /// Call `advance` to mount the tree. A surface can arrive after this call.
    pub fn mount_tree(&mut self, surface: &str) {
        if !self.trees.active.contains_key(surface) {
            self.trees
                .active
                .insert(surface.to_owned(), NodeTree::default());
            self.trees.dirty.insert(surface.to_owned());
        }
    }

    /// Returns the tree for a tracked surface.
    #[must_use]
    pub fn node_tree(&self, surface: &str) -> Option<&NodeTree> {
        self.trees.active.get(surface)
    }

    /// Writes a value through a mounted node's data binding.
    ///
    /// Call `advance` to publish the result.
    ///
    /// # Errors
    /// Returns an error if the node is absent, the property has no write path, or the write fails.
    pub fn write_node_property(
        &mut self,
        surface: &str,
        id: NodeId,
        property: &str,
        value: Value,
    ) -> Result<(), NodeWriteError> {
        self.reconcile_tree(surface);
        let entry = self
            .trees
            .active
            .get(surface)
            .and_then(|tree| tree.entries.get(&id))
            .ok_or(NodeWriteError::UnknownNode(id))?;
        let path = entry
            .evaluations
            .get(property)
            .and_then(|binding| binding.binding_path.clone())
            .ok_or_else(|| NodeWriteError::ReadOnly(property.to_owned()))?;
        self.write_binding(surface, &DataScope::default(), &path, value)?;
        Ok(())
    }

    /// Stops tracking a surface and removes its mounted nodes.
    ///
    /// The next `advance` reports their disposal.
    pub fn unmount_tree(&mut self, surface: &str) {
        self.trees.dirty.remove(surface);
        if let Some(tree) = self.trees.active.remove(surface) {
            self.retire_entries(surface, tree.entries);
        }
    }

    fn retire_entries(&mut self, surface: &str, entries: BTreeMap<NodeId, Entry>) {
        for entry in entries.values() {
            for property in entry.evaluations.values() {
                self.unmount(property.id);
            }
            for &check in entry.checks.iter().flatten() {
                self.unmount(check);
            }
        }
        record_disposals(&mut self.trees.changes, surface, entries);
    }

    pub(super) fn reconcile_trees(&mut self) {
        let mut pending = self.trees.dirty.clone();
        pending.extend(
            self.trees
                .active
                .iter()
                .filter(|(surface, tree)| {
                    tree.has_collections && tree.generation != self.state.generation(surface)
                })
                .map(|(surface, _)| surface.clone()),
        );
        for surface in pending {
            self.reconcile_tree(&surface);
        }
    }

    fn reconcile_tree(&mut self, surface: &str) {
        let layout_changed = self.trees.dirty.remove(surface);
        let generation = self.state.generation(surface);
        let Some(tree) = self.trees.active.get_mut(surface) else {
            return;
        };
        if !layout_changed && (!tree.has_collections || tree.generation == generation) {
            return;
        }
        tree.generation = generation;
        let processor = self.rpc.processor();
        let Some(model) = processor.get(surface) else {
            return;
        };
        if layout_changed {
            tree.definitions = definitions(model, &processor.catalogs);
            tree.has_collections = tree.definitions.values().any(|definition| {
                definition
                    .children
                    .iter()
                    .any(|child| child.collection.is_some())
            });
        }
        let mut reconcile = Reconcile {
            evaluations: &mut self.evaluations,
            next_evaluation: &mut self.next_id,
            next_node: &mut self.trees.next_id,
            surface,
            data: model.data_model(),
            definitions: &tree.definitions,
            previous: std::mem::take(&mut tree.entries),
            entries: BTreeMap::new(),
            errors: BTreeMap::new(),
        };
        tree.root = reconcile.node("root", &DataScope::default(), None, tree.root);
        let Reconcile {
            entries,
            previous,
            errors,
            ..
        } = reconcile;
        tree.entries = entries;
        tree.template_errors = errors;
        if !tree.has_collections {
            tree.definitions.clear();
        }
        self.retire_entries(surface, previous);
    }
}

fn record_disposals(
    changes: &mut Vec<NodeChange>,
    surface: &str,
    entries: BTreeMap<NodeId, Entry>,
) {
    changes.extend(entries.into_iter().rev().filter_map(|(id, entry)| {
        entry.published.then(|| NodeChange {
            surface_id: surface.to_owned(),
            id,
            kind: NodeChangeKind::Disposed,
        })
    }));
}

struct Entry {
    node: MountedNode,
    catalog_id: Option<String>,
    template_field: Option<String>,
    evaluations: BTreeMap<String, PropertyEvaluation>,
    checks: Option<Vec<EvaluationId>>,
    published: bool,
    next_children: Option<Vec<MountedChild>>,
}

impl Entry {
    fn publish_properties(
        &mut self,
        evaluations: &BTreeMap<EvaluationId, Evaluation>,
        validation: Vec<(&str, Value)>,
    ) {
        self.node.properties = self
            .evaluations
            .iter()
            .map(|(name, property)| {
                (
                    name.clone(),
                    MountedProperty {
                        value: property.value(evaluations).clone(),
                        binding_path: property.binding_path.clone(),
                        is_action: property.is_action,
                    },
                )
            })
            .collect();
        self.node
            .properties
            .extend(validation.into_iter().map(|(name, value)| {
                (
                    name.to_owned(),
                    MountedProperty {
                        value,
                        binding_path: None,
                        is_action: false,
                    },
                )
            }));
        if let Some(children) = self.next_children.take() {
            self.node.children = children;
        }
    }

    fn properties_changed(
        &self,
        evaluations: &BTreeMap<EvaluationId, Evaluation>,
        validation: &[(&str, Value)],
    ) -> bool {
        self.node.properties.len() != self.evaluations.len() + validation.len()
            || validation.iter().any(|(name, value)| {
                self.node.properties.get(*name).is_none_or(|property| {
                    property.binding_path.is_some()
                        || property.is_action
                        || &property.value != value
                })
            })
            || self.evaluations.iter().any(|(name, property)| {
                self.node.properties.get(name).is_none_or(|previous| {
                    previous.is_action != property.is_action
                        || previous.binding_path != property.binding_path
                        || &previous.value != property.value(evaluations)
                })
            })
    }

    fn validation_properties(
        &self,
        evaluations: &BTreeMap<EvaluationId, Evaluation>,
    ) -> Result<Vec<(&'static str, Value)>, serde_json::Error> {
        let Some(checks) = &self.checks else {
            return Ok(Vec::new());
        };
        let failed: Vec<_> = checks
            .iter()
            .filter_map(|id| evaluations.get(id)?.validation_result.as_ref())
            .filter(|result| !result.valid)
            .collect();
        let mut valid = true;
        let mut messages = Vec::new();
        for result in failed.iter().copied().filter(|result| result.is_blocking()) {
            valid = false;
            if let Some(message) = &result.message {
                messages.push(Value::String(message.clone()));
            }
        }
        let [is_valid, errors, results] = VALIDATION_PROPERTIES;
        Ok(vec![
            (is_valid, Value::Bool(valid)),
            (errors, Value::Array(messages)),
            (results, serde_json::to_value(failed)?),
        ])
    }
}

struct PropertyEvaluation {
    id: EvaluationId,
    binding_path: Option<String>,
    is_action: bool,
}

impl PropertyEvaluation {
    fn value<'a>(&self, evaluations: &'a BTreeMap<EvaluationId, Evaluation>) -> &'a Value {
        match evaluations.get(&self.id).map(|entry| &entry.outcome) {
            Some(EvaluationState::Ready(value)) => value,
            _ => &Value::Null,
        }
    }
}

struct Definition {
    component: String,
    catalog_id: Option<String>,
    properties: BTreeMap<String, PropertyDefinition>,
    check_count: Option<usize>,
    children: Vec<ChildDefinition>,
}

struct PropertyDefinition {
    binding_path: Option<String>,
    is_action: bool,
}

struct ChildDefinition {
    field: String,
    id: String,
    collection: Option<String>,
}

fn definitions(
    surface: &Surface,
    catalogs: &BTreeMap<String, Catalog>,
) -> BTreeMap<String, Definition> {
    surface
        .components()
        .iter()
        .map(|component| {
            let catalog_id = component.catalog_id.as_deref().or(surface.catalog_id());
            let payload = Value::Object(component.properties.clone().into_iter().collect());
            let catalog = catalog_id.and_then(|id| catalogs.get(id));
            let shape = catalog.map_or_else(Default::default, |catalog| {
                catalog.inspect_payload(catalog.component_schema(&component.component), &payload)
            });
            let checks =
                catalog.and_then(|catalog| catalog.component_checks(component).ok().flatten());
            let check_count = checks.and_then(Value::as_array).map(Vec::len);
            let properties = component
                .properties
                .iter()
                .filter(|(name, value)| {
                    let check_list = checks.is_some_and(|checks| std::ptr::eq(checks, *value));
                    let derived =
                        check_count.is_some() && VALIDATION_PROPERTIES.contains(&name.as_str());
                    !(check_list || derived)
                })
                .map(|(name, value)| {
                    let kind = payload
                        .get(name)
                        .and_then(|value| shape.properties.get(&std::ptr::from_ref(value)));
                    let is_action = matches!(kind, Some(PropertyKind::Action));
                    let binding_path = value
                        .as_object()
                        .filter(|object| object.len() == 1 && !is_action)
                        .and_then(|object| object.get("@path"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    (
                        name.clone(),
                        PropertyDefinition {
                            binding_path,
                            is_action,
                        },
                    )
                })
                .collect();
            let children = shape
                .children
                .into_iter()
                .map(|child| {
                    let field = if child.collection.is_some() {
                        child
                            .field
                            .strip_suffix(".componentId")
                            .unwrap_or(&child.field)
                            .to_owned()
                    } else {
                        child.field
                    };
                    ChildDefinition {
                        field,
                        id: child.id.to_owned(),
                        collection: child.collection.map(str::to_owned),
                    }
                })
                .collect();
            (
                component.id.clone(),
                Definition {
                    component: component.component.clone(),
                    catalog_id: catalog_id.map(str::to_owned),
                    properties,
                    check_count,
                    children,
                },
            )
        })
        .collect()
}

struct Reconcile<'a> {
    evaluations: &'a mut BTreeMap<EvaluationId, Evaluation>,
    next_evaluation: &'a mut u64,
    next_node: &'a mut u64,
    surface: &'a str,
    data: &'a DataModel,
    definitions: &'a BTreeMap<String, Definition>,
    previous: BTreeMap<NodeId, Entry>,
    entries: BTreeMap<NodeId, Entry>,
    errors: BTreeMap<(NodeId, String), RoutingError>,
}

impl Reconcile<'_> {
    fn node(
        &mut self,
        component_id: &str,
        scope: &DataScope,
        template_field: Option<&str>,
        old_id: Option<NodeId>,
    ) -> Option<NodeId> {
        let definition = self.definitions.get(component_id)?;
        let mut entry = old_id
            .filter(|id| {
                self.previous.get(id).is_some_and(|entry| {
                    entry.node.component_id == component_id
                        && entry.node.component == definition.component
                        && entry.node.scope == *scope
                        && entry.catalog_id == definition.catalog_id
                })
            })
            .and_then(|id| self.previous.remove(&id))
            .unwrap_or_else(|| self.new_entry(component_id, definition, scope, template_field));
        self.mount_properties(&mut entry, definition);
        self.mount_checks(&mut entry, definition.check_count);

        let children = self.children(&entry, definition);
        entry.next_children = (entry.node.children != children).then_some(children);
        let id = entry.node.id;
        self.entries.insert(id, entry);
        Some(id)
    }

    fn children(&mut self, parent: &Entry, definition: &Definition) -> Vec<MountedChild> {
        let mut previous = BTreeMap::<_, VecDeque<_>>::new();
        for child in parent
            .next_children
            .as_ref()
            .unwrap_or(&parent.node.children)
        {
            if let Some(entry) = self.previous.get(&child.id) {
                previous
                    .entry((
                        entry.node.component_id.clone(),
                        entry.template_field.clone(),
                    ))
                    .or_default()
                    .push_back(child.id);
            }
        }

        let mut children = Vec::new();
        for child in &definition.children {
            if let Some(path) = &child.collection {
                match parent.node.scope.collection(self.data, path) {
                    Ok(scopes) => {
                        let mut old = previous
                            .remove(&(child.id.clone(), Some(child.field.clone())))
                            .unwrap_or_default();
                        for (index, scope) in scopes.iter().enumerate() {
                            if let Some(id) =
                                self.node(&child.id, scope, Some(&child.field), old.pop_front())
                            {
                                children.push(MountedChild {
                                    field: format!("{}[{index}]", child.field),
                                    id,
                                });
                            }
                        }
                    }
                    Err(error) => {
                        self.errors.insert(
                            (parent.node.id, child.field.clone()),
                            RoutingError::expression(error),
                        );
                    }
                }
            } else {
                let old = previous
                    .get_mut(&(child.id.clone(), None))
                    .and_then(VecDeque::pop_front);
                if let Some(id) = self.node(&child.id, &parent.node.scope, None, old) {
                    children.push(MountedChild {
                        field: child.field.clone(),
                        id,
                    });
                }
            }
        }
        children
    }

    fn new_entry(
        &mut self,
        component_id: &str,
        definition: &Definition,
        scope: &DataScope,
        template_field: Option<&str>,
    ) -> Entry {
        *self.next_node += 1;
        Entry {
            node: MountedNode {
                id: NodeId(*self.next_node),
                component_id: component_id.to_owned(),
                component: definition.component.clone(),
                scope: scope.clone(),
                metadata: None,
                properties: BTreeMap::new(),
                children: Vec::new(),
            },
            catalog_id: definition.catalog_id.clone(),
            template_field: template_field.map(str::to_owned),
            evaluations: BTreeMap::new(),
            checks: None,
            published: false,
            next_children: None,
        }
    }

    fn mount_checks(&mut self, entry: &mut Entry, check_count: Option<usize>) {
        let mut checks = entry.checks.take().unwrap_or_default();
        let count = check_count.unwrap_or_default();
        for id in checks.drain(count.min(checks.len())..) {
            self.evaluations.remove(&id);
        }
        for index in checks.len()..count {
            checks.push(Evaluation::mount(
                self.evaluations,
                self.next_evaluation,
                self.surface,
                entry.node.scope.clone(),
                Task::CheckRule {
                    component: entry.node.component_id.clone(),
                    index,
                    internal: true,
                },
            ));
        }
        entry.checks = check_count.map(|_| checks);
    }

    fn mount_properties(&mut self, entry: &mut Entry, definition: &Definition) {
        entry.evaluations.retain(|name, property| {
            if definition.properties.contains_key(name) {
                true
            } else {
                self.evaluations.remove(&property.id);
                false
            }
        });
        for (name, definition) in &definition.properties {
            let property = entry.evaluations.entry(name.clone()).or_insert_with(|| {
                let id = Evaluation::mount(
                    self.evaluations,
                    self.next_evaluation,
                    self.surface,
                    entry.node.scope.clone(),
                    Task::Property {
                        component: entry.node.component_id.clone(),
                        property: name.clone(),
                        internal: true,
                    },
                );
                PropertyEvaluation {
                    id,
                    binding_path: None,
                    is_action: false,
                }
            });
            property.is_action = definition.is_action;
            property.binding_path = definition
                .binding_path
                .as_ref()
                .map(|path| entry.node.scope.resolve_path(path));
        }
    }
}
