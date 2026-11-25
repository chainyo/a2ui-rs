//! Tests Basic Catalog formatting through mounted nodes.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use a2ui_rs::basic_catalog::{
    FormattingOptions, FormattingOptionsError, register_formatting_functions,
};
use a2ui_rs::catalog::Catalog;
use a2ui_rs::resolution::LocalFunctions;
use a2ui_rs::routing::{NodeChangeKind, NodeId, RoutingSession};
use a2ui_rs::rpc::RpcHandler;
use serde_json::json;

#[test]
fn mounted_formatters_use_the_renderer_locale_and_time_zone() -> Result<(), Box<dyn Error>> {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    let catalog =
        serde_json::from_slice(&fs::read(upstream.join("catalogs/basic/v1/catalog.json"))?)?;
    let common = serde_json::from_slice(&fs::read(
        upstream.join("specification/v1_0/json/common_types.json"),
    )?)?;
    let catalog = Catalog::from_json(catalog, &common)?;
    let mut functions = LocalFunctions::default();
    register_formatting_functions(
        &mut functions,
        catalog.id(),
        FormattingOptions::new("de-DE", "Europe/Berlin")?,
    );
    let mut session = RoutingSession::new(RpcHandler::default(), functions);
    session.add_catalog(catalog)?;

    let surface = "invoice";
    let now = Instant::now();
    session.mount_tree(surface);
    session.receive(
        serde_json::from_str(include_str!(
            "../../../fixtures/local/basic-formatting.json"
        ))?,
        now,
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty(), "{:?}", update.errors);

    let tree = session.node_tree(surface).ok_or("missing tree")?;
    let root = tree
        .root()
        .and_then(|id| tree.node(id))
        .ok_or("missing root")?;
    let [total, currency, sent, sent_local, count] = root.children.as_slice() else {
        return Err("expected five invoice fields".into());
    };
    let (total, currency, sent, sent_local, count) =
        (total.id, currency.id, sent.id, sent_local.id, count.id);
    assert_text(&session, total, "1.234,50")?;
    assert_text(&session, currency, "1234,500\u{a0}€")?;
    assert_text(&session, sent, "2026-07-01T10:00:00.000Z")?;
    assert_text(&session, sent_local, "Mittwoch, 1. Juli 2026 (Mi.)")?;
    assert_text(&session, count, "Eine Position")?;

    session.receive(
        serde_json::from_value(json!({
            "version": "1.0",
            "updateDataModel": {"surfaceId": surface, "path": "/total", "value": -9876.5435}
        }))?,
        now,
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty(), "{:?}", update.errors);
    assert_eq!(update.nodes.len(), 2);
    assert!(update.nodes.iter().all(|change| {
        [total, currency].contains(&change.id) && change.kind == NodeChangeKind::Updated
    }));
    assert_text(&session, total, "-9.876,54")?;
    assert_text(&session, currency, "-9876,544\u{a0}€")?;

    session.receive(
        serde_json::from_value(json!({
            "version": "1.0",
            "updateDataModel": {"surfaceId": surface, "value": {
                "total": -9876.5435,
                "count": 2,
                "sentAt": "2026-01-01"
            }}
        }))?,
        now,
    )?;
    let update = session.advance(now, None);
    assert!(update.errors.is_empty(), "{:?}", update.errors);
    assert_eq!(update.nodes.len(), 3);
    assert!(update.nodes.iter().all(|change| {
        [sent, sent_local, count].contains(&change.id) && change.kind == NodeChangeKind::Updated
    }));
    assert_text(&session, sent, "2025-12-31T23:00:00.000Z")?;
    assert_text(&session, sent_local, "Donnerstag, 1. Januar 2026 (Do.)")?;
    assert_text(&session, count, "Mehrere Positionen")?;
    Ok(())
}

#[test]
fn rejects_invalid_locale_and_time_zone() {
    assert!(matches!(
        FormattingOptions::new("en--US", "UTC"),
        Err(FormattingOptionsError::Locale(_))
    ));
    assert!(matches!(
        FormattingOptions::new("en-US", "Missing/Zone"),
        Err(FormattingOptionsError::TimeZone(_))
    ));
}

fn assert_text(
    session: &RoutingSession<'_>,
    id: NodeId,
    expected: &str,
) -> Result<(), Box<dyn Error>> {
    let node = session
        .node_tree("invoice")
        .and_then(|tree| tree.node(id))
        .ok_or("missing invoice field")?;
    assert_eq!(node.properties["text"].value, expected);
    Ok(())
}
