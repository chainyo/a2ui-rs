//! Checks graph validation across incremental component updates.

use std::error::Error;
use std::fs;
use std::path::Path;

use a2ui_rs::catalog::Catalog;
use a2ui_rs::protocol::AgentToRendererMessage;
use a2ui_rs::validation::{MessageProcessor, ValidationConfig, ValidationError};
use serde_json::{Value, json};

fn processor() -> Result<MessageProcessor, Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let mut catalog: Value = serde_json::from_slice(&fs::read(
        upstream.join("conformance/test_data/catalogs/composition_catalog_v1_0.json"),
    )?)?;
    let mut panel = catalog["components"]["Column"].clone();
    panel["properties"]["component"]["const"] = json!("Panel");
    panel["allowedChildren"] = json!(["Column"]);
    catalog["components"]["Panel"] = panel;
    catalog["$defs"]["anyComponent"]["oneOf"]
        .as_array_mut()
        .ok_or("missing component union")?
        .push(json!({"$ref": "#/components/Panel"}));
    catalog["components"]["Column"]["allowedChildren"] = json!(["Column", "Panel", "Text"]);
    catalog["components"]["Column"]["allowedParents"] = json!(["Surface", "Column", "Panel"]);
    let mut processor = MessageProcessor::new(ValidationConfig::RELAXED);
    processor.add_catalog(Catalog::from_json(catalog, &common)?)?;
    Ok(processor)
}

fn update(components: &Value) -> Result<AgentToRendererMessage, serde_json::Error> {
    serde_json::from_value(json!({
        "version": "1.0",
        "updateComponents": {"surfaceId": "main", "components": components}
    }))
}

#[test]
fn checks_all_waiting_parents_and_keeps_rejected_batches_atomic() -> Result<(), Box<dyn Error>> {
    let mut processor = processor()?;
    processor.apply_json(&json!({
        "version": "1.0", "createSurface": {
            "surfaceId": "main", "catalogId": "conformance/composition",
            "components": [
                {"id": "root", "component": "Column", "children": ["item", "panel"]},
                {"id": "panel", "component": "Panel", "children": ["item"]}
            ]
        }
    }))?;
    let before = processor.get("main").cloned();
    let error = processor.apply(update(&json!([
        {"id": "item", "component": "Text", "text": "Arrival"},
        {"id": "extra", "component": "Text", "text": "Other"}
    ]))?);
    assert!(matches!(
        error,
        Err(ValidationError::UnallowedParent { parent, child, path, .. })
            if parent == "panel" && child == "item" && path == "/components/0"
    ));
    assert_eq!(processor.get("main"), before.as_ref());

    processor.apply(update(&json!([
        {"id": "item", "component": "Column", "children": []}
    ]))?)?;
    processor.apply(update(&json!([
        {"id": "panel", "component": "Panel", "children": []}
    ]))?)?;
    processor.apply(update(&json!([
        {"id": "item", "component": "Text", "text": "Replacement"}
    ]))?)?;
    Ok(())
}

#[test]
fn checks_depth_when_a_leaf_completes_a_dangling_path() -> Result<(), Box<dyn Error>> {
    let mut processor = processor()?;
    let mut components = vec![json!({"id": "root", "component": "Column", "children": ["n1"]})];
    components.extend((1..=50).map(|index| {
        json!({
            "id": format!("n{index}"), "component": "Column",
            "children": [format!("n{}", index + 1)]
        })
    }));
    processor.apply_json(&json!({
        "version": "1.0", "createSurface": {
            "surfaceId": "main", "catalogId": "conformance/composition", "components": components
        }
    }))?;
    let before = processor.get("main").cloned();
    assert!(matches!(
        processor.apply(update(
            &json!([{"id": "n51", "component": "Text", "text": "Deep"}])
        )?),
        Err(ValidationError::ComponentDepth)
    ));
    assert_eq!(processor.get("main"), before.as_ref());
    Ok(())
}

#[test]
fn rechecks_unchanged_references_when_validation_rules_change() -> Result<(), Box<dyn Error>> {
    let mut processor = processor()?;
    processor.apply_json(&json!({
        "version": "1.0", "createSurface": {
            "surfaceId": "main", "catalogId": "conformance/composition",
            "components": [
                {"id": "root", "component": "Column", "children": ["item", "later"]},
                {"id": "item", "component": "Text", "text": "Before"}
            ]
        }
    }))?;
    let before = processor.get("main").cloned();
    let message = update(&json!([{"id": "item", "component": "Text", "text": "After"}]))?;
    assert!(matches!(
        processor.apply_with_config(message.clone(), ValidationConfig::default()),
        Err(ValidationError::DanglingReference { child, .. }) if child == "later"
    ));
    assert_eq!(processor.get("main"), before.as_ref());
    processor.apply(message)?;
    Ok(())
}
