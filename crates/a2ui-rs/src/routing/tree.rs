use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::Value;

use super::{Evaluation, EvaluationId, EvaluationState, RoutingSession, Task};
use crate::catalog::{Catalog, PropertyKind};
use crate::resolution::DataScope;
use crate::surface::Surface;

/// Identifies one mounted node in a routing session.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NodeId(u64);

/// The current value and write path of a node property.
#[derive(Clone, Debug, PartialEq)]
pub struct MountedProperty {
    /// The resolved value. A pending or failed expression can have a null value.
    pub value: Value,
    /// The data path for a writable binding.
    pub binding_path: Option<String>,
}

/// An explicit child and the field that refers to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountedChild {
    /// The field path in the component definition.
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
    /// The resolved properties, including explicit child references.
    pub properties: BTreeMap<String, MountedProperty>,
    /// The mounted explicit children in order.
    pub children: Vec<MountedChild>,
}

/// The change to a mounted node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeChangeKind {
    /// The node is available with its initial properties.
    Mounted,
    /// The properties or child list of a retained node changed.
    Updated,
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

/// The mounted root and explicit descendants for one surface.
#[derive(Default)]
pub struct NodeTree {
    root: Option<NodeId>,
    entries: BTreeMap<NodeId, Entry>,
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
            tree.root = None;
            record_disposals(
                &mut self.changes,
                surface,
                std::mem::take(&mut tree.entries),
            );
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
        evaluations: &BTreeMap<EvaluationId, Evaluation>,
    ) -> Vec<NodeChange> {
        for (surface, tree) in &mut self.active {
            for (&id, entry) in &mut tree.entries {
                let kind = if !entry.published {
                    NodeChangeKind::Mounted
                } else if entry.children_changed || entry.properties_changed(evaluations) {
                    NodeChangeKind::Updated
                } else {
                    continue;
                };
                self.changes.push(NodeChange {
                    surface_id: surface.clone(),
                    id,
                    kind,
                });
                entry.node.properties = entry
                    .evaluations
                    .iter()
                    .map(|(name, property)| {
                        (
                            name.clone(),
                            MountedProperty {
                                value: property.value(evaluations).clone(),
                                binding_path: property.binding_path.clone(),
                            },
                        )
                    })
                    .collect();
                entry.published = true;
                entry.children_changed = false;
            }
        }
        std::mem::take(&mut self.changes)
    }
}

impl RoutingSession<'_> {
    /// Tracks the root and explicit descendants of a surface.
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
        }
        record_disposals(&mut self.trees.changes, surface, entries);
    }

    pub(super) fn reconcile_trees(&mut self) {
        for surface in std::mem::take(&mut self.trees.dirty) {
            let Some(tree) = self.trees.active.remove(&surface) else {
                continue;
            };
            let processor = self.processor();
            let definitions = processor
                .get(&surface)
                .map_or_else(BTreeMap::new, |surface| {
                    definitions(surface, &processor.catalogs)
                });
            let mut reconcile = Reconcile {
                session: self,
                surface: &surface,
                definitions: &definitions,
                previous: tree.entries,
                entries: BTreeMap::new(),
            };
            let root = reconcile.node("root", tree.root);
            let Reconcile {
                entries, previous, ..
            } = reconcile;
            self.retire_entries(&surface, previous);
            self.trees
                .active
                .insert(surface, NodeTree { root, entries });
        }
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
    evaluations: BTreeMap<String, PropertyEvaluation>,
    published: bool,
    children_changed: bool,
}

impl Entry {
    fn properties_changed(&self, evaluations: &BTreeMap<EvaluationId, Evaluation>) -> bool {
        self.node.properties.len() != self.evaluations.len()
            || self.evaluations.iter().any(|(name, property)| {
                self.node.properties.get(name).is_none_or(|previous| {
                    previous.binding_path != property.binding_path
                        || &previous.value != property.value(evaluations)
                })
            })
    }
}

struct PropertyEvaluation {
    id: EvaluationId,
    binding_path: Option<String>,
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
    properties: BTreeMap<String, Option<String>>,
    children: Vec<(String, String)>,
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
            let properties = component
                .properties
                .iter()
                .filter(|(_, value)| !checks.is_some_and(|checks| std::ptr::eq(checks, *value)))
                .map(|(name, value)| {
                    let kind = payload
                        .get(name)
                        .and_then(|value| shape.properties.get(&std::ptr::from_ref(value)));
                    let binding = value
                        .as_object()
                        .filter(|object| {
                            object.len() == 1 && !matches!(kind, Some(PropertyKind::Action))
                        })
                        .and_then(|object| object.get("@path"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    (name.clone(), binding)
                })
                .collect();
            let children = shape
                .children
                .into_iter()
                .filter(|child| child.collection.is_none())
                .map(|child| (child.field, child.id.to_owned()))
                .collect();
            (
                component.id.clone(),
                Definition {
                    component: component.component.clone(),
                    catalog_id: catalog_id.map(str::to_owned),
                    properties,
                    children,
                },
            )
        })
        .collect()
}

struct Reconcile<'a, 'f> {
    session: &'a mut RoutingSession<'f>,
    surface: &'a str,
    definitions: &'a BTreeMap<String, Definition>,
    previous: BTreeMap<NodeId, Entry>,
    entries: BTreeMap<NodeId, Entry>,
}

impl Reconcile<'_, '_> {
    fn node(&mut self, component_id: &str, old_id: Option<NodeId>) -> Option<NodeId> {
        let definition = self.definitions.get(component_id)?;
        let mut entry = old_id
            .filter(|id| {
                self.previous.get(id).is_some_and(|entry| {
                    entry.node.component_id == component_id
                        && entry.node.component == definition.component
                        && entry.catalog_id == definition.catalog_id
                })
            })
            .and_then(|id| self.previous.remove(&id))
            .unwrap_or_else(|| self.new_entry(component_id, definition));
        self.mount_properties(&mut entry, definition);

        let mut previous_children = BTreeMap::<_, VecDeque<_>>::new();
        for child in &entry.node.children {
            if let Some(previous) = self.previous.get(&child.id) {
                previous_children
                    .entry(previous.node.component_id.clone())
                    .or_default()
                    .push_back(child.id);
            }
        }
        let children = definition
            .children
            .iter()
            .filter_map(|(field, child)| {
                let old = previous_children
                    .get_mut(child)
                    .and_then(VecDeque::pop_front);
                self.node(child, old).map(|id| MountedChild {
                    field: field.clone(),
                    id,
                })
            })
            .collect();
        entry.children_changed = entry.node.children != children;
        entry.node.children = children;
        let id = entry.node.id;
        self.entries.insert(id, entry);
        Some(id)
    }

    fn new_entry(&mut self, component_id: &str, definition: &Definition) -> Entry {
        self.session.trees.next_id += 1;
        Entry {
            node: MountedNode {
                id: NodeId(self.session.trees.next_id),
                component_id: component_id.to_owned(),
                component: definition.component.clone(),
                properties: BTreeMap::new(),
                children: Vec::new(),
            },
            catalog_id: definition.catalog_id.clone(),
            evaluations: BTreeMap::new(),
            published: false,
            children_changed: false,
        }
    }

    fn mount_properties(&mut self, entry: &mut Entry, definition: &Definition) {
        entry.evaluations.retain(|name, property| {
            if definition.properties.contains_key(name) {
                true
            } else {
                self.session.unmount(property.id);
                false
            }
        });
        for (name, binding_path) in &definition.properties {
            let property = entry.evaluations.entry(name.clone()).or_insert_with(|| {
                let id = self.session.mount(
                    self.surface,
                    DataScope::default(),
                    Task::Property {
                        component: entry.node.component_id.clone(),
                        property: name.clone(),
                        internal: true,
                    },
                );
                PropertyEvaluation {
                    id,
                    binding_path: None,
                }
            });
            property.binding_path.clone_from(binding_path);
        }
    }
}
