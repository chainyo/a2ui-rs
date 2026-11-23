//! Tests the surface lifecycle from JSON messages to stored state.

use std::error::Error;

use a2ui_rs::protocol::AgentToRendererMessage;
use a2ui_rs::surface::{SurfaceError, SurfaceStore};
use serde_json::json;

#[test]
fn applies_the_surface_lifecycle() -> Result<(), Box<dyn Error>> {
    let messages: [AgentToRendererMessage; 8] = serde_json::from_str(include_str!(
        "../../../fixtures/local/surface-lifecycle.json"
    ))?;
    let [
        create,
        sidebar,
        rename,
        clear_draft,
        replace_root,
        delete,
        late_update,
        recreate,
    ] = messages;

    let mut store = SurfaceStore::default();
    store.apply(create.clone())?;
    store.apply(sidebar)?;

    let contact = store.get("contact").ok_or("missing contact")?;
    assert_eq!(contact.catalog_id(), Some("example:contacts"));
    assert!(contact.send_data_model());
    assert_eq!(
        contact.components().first().ok_or("missing root")?.id,
        "root"
    );
    assert_eq!(
        contact
            .metadata()
            .and_then(|metadata| metadata.extensions.as_ref())
            .and_then(|extensions| extensions.as_object().get("mode")),
        Some(&json!("edit"))
    );

    store.apply(rename)?;
    store.apply(clear_draft)?;
    assert_eq!(
        store.apply(create),
        Err(SurfaceError::DuplicateSurface("contact".to_owned()))
    );
    let contact = store.get("contact").ok_or("missing contact")?;
    assert_eq!(
        contact.data_model().root(),
        &json!({"person":{"name":"Grace"}})
    );

    store.apply(replace_root)?;
    assert_eq!(
        store
            .get("contact")
            .ok_or("missing contact")?
            .data_model()
            .root(),
        &json!({"ready":true})
    );

    store.apply(delete.clone())?;
    store.apply(delete)?;
    assert!(store.get("contact").is_none());
    assert_eq!(
        store.apply(late_update),
        Err(SurfaceError::UnknownSurface("contact".to_owned()))
    );
    store.apply(recreate)?;
    let contact = store.get("contact").ok_or("missing recreated contact")?;
    assert_eq!(contact.data_model().root(), &json!({}));
    assert!(contact.catalog_id().is_none());
    assert!(contact.metadata().is_none());
    assert!(contact.components().is_empty());
    assert!(!contact.send_data_model());
    assert_eq!(
        store
            .get("sidebar")
            .ok_or("missing sidebar")?
            .data_model()
            .root(),
        &json!({"count":1})
    );
    Ok(())
}

#[test]
fn replaces_component_definitions_and_rejects_duplicate_batches() -> Result<(), Box<dyn Error>> {
    let [create, update, duplicate] = serde_json::from_str::<[AgentToRendererMessage; 3]>(
        include_str!("../../../fixtures/local/component-updates.json"),
    )?;
    let mut store = SurfaceStore::default();
    store.apply(create)?;
    store.apply(update)?;

    let surface = store.get("checkout").ok_or("missing checkout surface")?;
    assert_eq!(
        serde_json::to_value(surface.components())?,
        json!([
            {"id": "root", "component": "Column", "children": ["summary", "total"]},
            {"id": "summary", "component": "Row", "children": []},
            {"id": "total", "component": "Text", "text": "$24.00"}
        ])
    );

    let before = store.clone();
    assert_eq!(
        store.apply(duplicate),
        Err(SurfaceError::DuplicateComponent("total".to_owned()))
    );
    assert_eq!(store, before);
    Ok(())
}
