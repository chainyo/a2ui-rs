//! Surface storage for the A2UI lifecycle.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::data_model::{DataError, DataModel};
use crate::protocol::{AgentToRenderer, AgentToRendererMessage, Component, Components, Metadata};

/// The data and configuration for an active surface.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    catalog_id: Option<String>,
    catalog_count: usize,
    send_data_model: bool,
    components: Vec<Component>,
    component_indices: BTreeMap<String, usize>,
    data_model: DataModel,
    metadata: Option<Metadata>,
}

impl Surface {
    /// Returns the default catalog ID.
    #[must_use]
    pub fn catalog_id(&self) -> Option<&str> {
        self.catalog_id.as_deref()
    }

    pub(crate) fn catalog_count(&self) -> usize {
        self.catalog_count
    }

    /// Returns the data model flag for outgoing transport metadata.
    #[must_use]
    pub fn send_data_model(&self) -> bool {
        self.send_data_model
    }

    /// Returns current component definitions in order of insertion.
    #[must_use]
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    pub(crate) fn component(&self, id: &str) -> Option<&Component> {
        self.component_indices
            .get(id)
            .map(|&index| &self.components[index])
    }

    /// Returns the surface data model.
    #[must_use]
    pub fn data_model(&self) -> &DataModel {
        &self.data_model
    }

    /// Returns the surface metadata.
    #[must_use]
    pub fn metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref()
    }

    fn apply_components(&mut self, components: Components) -> Result<(), SurfaceError> {
        let mut ids = BTreeSet::new();
        for component in components.as_slice() {
            if !ids.insert(component.id.as_str()) {
                return Err(SurfaceError::DuplicateComponent(component.id.clone()));
            }
        }

        for component in components.into_vec() {
            if let Some(&index) = self.component_indices.get(&component.id) {
                self.components[index] = component;
            } else {
                self.component_indices
                    .insert(component.id.clone(), self.components.len());
                self.components.push(component);
            }
        }
        Ok(())
    }
}

/// Stores active surfaces by ID for one renderer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceStore {
    surfaces: BTreeMap<String, Surface>,
}

impl SurfaceStore {
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Surface)> {
        self.surfaces
            .iter()
            .map(|(id, surface)| (id.as_str(), surface))
    }

    /// Returns the active surface with this ID, if it exists.
    #[must_use]
    pub fn get(&self, surface_id: &str) -> Option<&Surface> {
        self.surfaces.get(surface_id)
    }

    /// Applies one create, component update, data update, or delete message.
    ///
    /// A delete of an absent surface leaves the store unchanged.
    /// After deletion, a create message can use the same ID.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate surface or component IDs, an update to an absent
    /// surface, an invalid data update, or an unsupported operation.
    /// An error leaves the store unchanged.
    pub fn apply(&mut self, message: AgentToRendererMessage) -> Result<(), SurfaceError> {
        self.apply_with_catalog_count(message, 0)
    }

    pub(crate) fn apply_with_catalog_count(
        &mut self,
        message: AgentToRendererMessage,
        catalog_count: usize,
    ) -> Result<(), SurfaceError> {
        let AgentToRendererMessage::V1(operation) = message;
        match operation {
            AgentToRenderer::CreateSurface(create) => {
                match self.surfaces.entry(create.surface_id) {
                    Entry::Occupied(entry) => {
                        return Err(SurfaceError::DuplicateSurface(entry.key().clone()));
                    }
                    Entry::Vacant(entry) => {
                        let mut surface = Surface {
                            catalog_id: create.catalog_id,
                            catalog_count,
                            send_data_model: create.send_data_model.unwrap_or_default(),
                            components: Vec::new(),
                            component_indices: BTreeMap::new(),
                            data_model: DataModel::new(create.data_model.unwrap_or_default()),
                            metadata: create.metadata,
                        };
                        if let Some(components) = create.components {
                            surface.apply_components(components)?;
                        }
                        entry.insert(surface);
                    }
                }
            }
            AgentToRenderer::UpdateComponents(update) => {
                let Some(surface) = self.surfaces.get_mut(&update.surface_id) else {
                    return Err(SurfaceError::UnknownSurface(update.surface_id));
                };
                surface.apply_components(update.components)?;
            }
            AgentToRenderer::UpdateDataModel(update) => {
                let Some(surface) = self.surfaces.get_mut(&update.surface_id) else {
                    return Err(SurfaceError::UnknownSurface(update.surface_id));
                };
                surface
                    .data_model
                    .set(update.path.as_deref().unwrap_or("/"), update.value)
                    .map_err(|source| SurfaceError::Data {
                        surface_id: update.surface_id,
                        source,
                    })?;
            }
            AgentToRenderer::DeleteSurface(delete) => {
                self.surfaces.remove(&delete.surface_id);
            }
            _ => return Err(SurfaceError::UnsupportedOperation),
        }
        Ok(())
    }
}

/// An error from a surface lifecycle operation.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SurfaceError {
    /// The create message used an active surface ID.
    #[error("Surface '{0}' already exists")]
    DuplicateSurface(String),
    /// A message included the same component ID more than once.
    #[error("Duplicate component ID '{0}'")]
    DuplicateComponent(String),
    /// The update message used an absent surface ID.
    #[error("Surface '{0}' does not exist")]
    UnknownSurface(String),
    /// An update failed in the data model.
    #[error("Cannot update data for surface '{surface_id}': {source}")]
    Data {
        /// The destination surface ID.
        surface_id: String,
        /// The data model error.
        #[source]
        source: DataError,
    },
    /// The store cannot apply this operation.
    #[error("Operation is outside the surface lifecycle")]
    UnsupportedOperation,
}
