//! Exercises validation with the public message processor.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::catalog::{Catalog, CatalogError};
use a2ui_rs::protocol::AgentToRendererMessage;
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{NodeTree, RoutingSession};
use a2ui_rs::rpc::{ReceiveError, RpcHandler};
use a2ui_rs::validation::{MessageProcessor, ValidationError};
use serde::Deserialize;
use serde_json::{Value, json};

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

#[test]
fn catalog_rejects_an_unavailable_resource_in_common_types() -> Result<(), Box<dyn Error>> {
    let common_types = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/upstream/specification/v1_0/json/common_types.json");
    let mut common_types: Value = serde_json::from_slice(&fs::read(common_types)?)?;
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../fixtures/local/catalog-topology.json"
    ))?;
    let unavailable = "https://schemas.example/component-id.json";
    common_types["$defs"]["ComponentId"] = json!({"$ref": unavailable});
    assert!(matches!(
        Catalog::from_json(fixture.catalog, &common_types),
        Err(CatalogError::InvalidCatalog(message)) if message.contains(unavailable)
    ));
    Ok(())
}

#[test]
fn rejected_self_reference_reports_the_surface_and_keeps_the_mounted_tree()
-> Result<(), Box<dyn Error>> {
    let common_types = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/upstream/specification/v1_0/json/common_types.json");
    let common_types = serde_json::from_slice(&fs::read(common_types)?)?;
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../fixtures/local/catalog-topology.json"
    ))?;
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    session.add_catalog(Catalog::from_json(fixture.catalog, &common_types)?)?;
    let now = Instant::now();
    let surface = "checkout";
    session.mount_tree(surface);
    let [create, dangling, ..] = fixture.messages;
    session.receive(create, now)?;
    assert!(session.advance(now, None).errors.is_empty());
    let root = session.node_tree(surface).and_then(NodeTree::root);
    assert!(root.is_some());
    let before = session.processor().get(surface).cloned();

    let update = serde_json::from_value(json!({
        "version": "1.0",
        "updateComponents": {
            "surfaceId": surface,
            "components": [{"id": "root", "component": "Stack", "slots": ["root"]}]
        }
    }))?;
    let Err(error @ ReceiveError::Validation(ValidationError::SelfReference(_))) =
        session.receive(update, now)
    else {
        return Err("expected the self-reference to fail".into());
    };
    assert_eq!(
        serde_json::to_value(error.renderer_error_for_surface(surface))?,
        json!({"version": "1.0", "error": {
            "code": "RECURSION_LIMIT_EXCEEDED",
            "surfaceId": surface,
            "message": "Self-reference detected at component 'root'"
        }})
    );

    let Err(error) = session.receive(dangling, now) else {
        return Err("expected the dangling child to fail".into());
    };
    assert_eq!(
        serde_json::to_value(error.renderer_error_for_surface(surface))?,
        json!({"version": "1.0", "error": {
            "code": "INTEGRITY_ERROR",
            "surfaceId": surface,
            "message": "Dangling reference: component 'root' references non-existent component 'missing' in field 'slots[1]'"
        }})
    );
    let update = session.advance(now, None);
    assert!(update.nodes.is_empty());
    assert!(update.errors.is_empty());
    assert_eq!(session.processor().get(surface), before.as_ref());
    assert_eq!(session.node_tree(surface).and_then(NodeTree::root), root);
    Ok(())
}

#[test]
fn nested_catalog_lookup_reports_the_surface_and_keeps_the_previous_definition()
-> Result<(), Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let catalog =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let mut session = RoutingSession::new(RpcHandler::default(), LocalFunctions::default());
    session.add_catalog(Catalog::from_json(catalog, &common)?)?;
    let [create, missing, unknown]: [AgentToRendererMessage; 3] = serde_json::from_str(
        include_str!("../../../fixtures/local/catalog-lookup-errors.json"),
    )?;
    let now = Instant::now();
    let surface = "shipment";
    session.mount_tree(surface);
    session.receive(create, now)?;
    assert!(session.advance(now, None).errors.is_empty());
    let before = session.processor().get(surface).cloned();
    assert!(before.is_some());

    for (message, expected) in [
        (missing, "Surface has no default catalogId"),
        (unknown, "Catalog not found: example:shipping"),
    ] {
        let Err(error) = session.receive(message, now) else {
            return Err("expected the nested catalog lookup to fail".into());
        };
        assert_eq!(
            serde_json::to_value(error.renderer_error_for_surface(surface))?,
            json!({"version": "1.0", "error": {
                "code": "CATALOG_NOT_FOUND",
                "surfaceId": surface,
                "message": expected
            }})
        );
        let update = session.advance(now, None);
        assert!(update.nodes.is_empty());
        assert!(update.errors.is_empty());
        assert_eq!(session.processor().get(surface), before.as_ref());
    }
    Ok(())
}
