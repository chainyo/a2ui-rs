//! Exercises validation with the public message processor.

use std::error::Error;
use std::fs;
use std::path::Path;

use a2ui_rs::catalog::{Catalog, CatalogError};
use a2ui_rs::protocol::AgentToRendererMessage;
use a2ui_rs::validation::{MessageProcessor, ValidationError};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    catalog: Value,
    partner_catalog: Value,
    messages: [AgentToRendererMessage; 6],
}

#[test]
fn rejects_invalid_candidates_without_changing_surface_state() -> Result<(), Box<dyn Error>> {
    let common_types = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/upstream/specification/v1_0/json/common_types.json");
    let common_types = serde_json::from_slice(&fs::read(common_types)?)?;
    let Fixture {
        catalog,
        partner_catalog,
        messages:
            [
                create,
                dangling,
                unallowed,
                invalid,
                override_catalog,
                without_override,
            ],
    } = serde_json::from_str(include_str!(
        "../../../fixtures/local/catalog-topology.json"
    ))?;
    let mut processor = MessageProcessor::default();
    processor.add_catalog(Catalog::from_json(catalog, &common_types)?)?;
    processor.add_catalog(Catalog::from_json(partner_catalog, &common_types)?)?;

    processor.apply(create)?;
    let before = processor
        .get("checkout")
        .ok_or("missing checkout surface")?
        .clone();
    assert!(
        matches!(processor.apply(dangling), Err(ValidationError::DanglingReference { child, .. }) if child == "missing")
    );
    assert!(
        matches!(processor.apply(unallowed), Err(ValidationError::UnallowedChild { child, .. }) if child == "payment")
    );
    assert!(
        matches!(processor.apply(invalid), Err(ValidationError::Catalog(CatalogError::InvalidPayload { name, .. })) if name == "Summary")
    );
    assert_eq!(processor.get("checkout"), Some(&before));

    processor.apply(override_catalog)?;
    let after_override = processor
        .get("checkout")
        .ok_or("missing checkout surface")?
        .clone();
    let summary = after_override
        .components()
        .iter()
        .find(|component| component.id == "summary")
        .ok_or("missing summary")?;
    assert_eq!(summary.catalog_id.as_deref(), Some("example:payments"));
    assert!(
        matches!(processor.apply(without_override), Err(ValidationError::Catalog(CatalogError::InvalidPayload { name, .. })) if name == "Summary")
    );
    assert_eq!(processor.get("checkout"), Some(&after_override));
    Ok(())
}
