//! Exercises scoped bindings and local expressions through the message processor.

use std::cell::Cell;
use std::error::Error;
use std::fs;
use std::path::Path;

use a2ui_rs::catalog::{Catalog, CatalogError};
use a2ui_rs::protocol::AgentToRendererMessage;
use a2ui_rs::resolution::{DataScope, LocalFunctions, ResolveError, ResolvedComponent};
use a2ui_rs::validation::{MessageProcessor, ValidationError};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Fixture {
    catalogs: Vec<Value>,
    create: AgentToRendererMessage,
    invalid: AgentToRendererMessage,
}

#[test]
fn resolves_nested_collections_and_validates_local_calls() -> Result<(), Box<dyn Error>> {
    let common = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/upstream/specification/v1_0/json/common_types.json");
    let common = serde_json::from_slice(&fs::read(common)?)?;
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../fixtures/local/binding-expressions.json"
    ))?;
    let mut processor = MessageProcessor::default();
    for catalog in fixture.catalogs {
        processor.add_catalog(Catalog::from_json(catalog, &common)?)?;
    }
    processor.apply(fixture.create)?;

    let confirms = Cell::new(0);
    let mut functions = LocalFunctions::default();
    functions.insert("example:display", "label", |args| {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or("missing name")?;
        let city = args
            .get("city")
            .and_then(Value::as_str)
            .ok_or("missing city")?;
        Ok(Value::from(format!("{name} in {city}")))
    });
    functions.insert("example:display", "decorate", |args| {
        let value = args
            .get("value")
            .and_then(Value::as_str)
            .ok_or("missing value")?;
        Ok(Value::from(format!("[{value}]")))
    });
    functions.insert("example:display", "confirm", |_| {
        confirms.set(confirms.get() + 1);
        Ok(Value::Null)
    });

    let tree = snapshot(&processor, &functions)?;
    let first = order(&tree, 0, 0)?;
    assert_eq!(first.scope.path(), "/groups/0/items/0");
    assert_eq!(first.scope.nested("details").index(), Some(0));
    assert_eq!(first.properties["text"], "#1 [Tea in London]");
    assert_eq!(
        order(&tree, 0, 1)?.properties["text"],
        "#2 [Bread in London]"
    );
    assert_eq!(
        order(&tree, 1, 0)?.properties["text"],
        "#1 [Rice in London]"
    );
    assert_eq!(
        first.properties["controls"]["confirm"]["functionCall"]["@call"],
        "confirm"
    );

    let nested = processor.resolve_value(
        "orders",
        &first.scope,
        &json!({
            "name": {"@path": "name"},
            "items": [{"@path": "/city"}, {"@path": "."}],
            "@@description": {"@path": "missing"}
        }),
        &functions,
    )?;
    assert_eq!(
        nested,
        json!({"name": "Tea", "items": ["London", {"name": "Tea"}], "@description": null})
    );

    processor.write_binding("orders", &first.scope, "name", json!("Coffee"))?;
    assert!(matches!(
        processor.apply(fixture.invalid),
        Err(ValidationError::Catalog(CatalogError::InvalidPayload { name, .. })) if name == "label"
    ));
    let updated = snapshot(&processor, &functions)?;
    assert_eq!(
        order(&updated, 0, 0)?.properties["text"],
        "#1 [Coffee in London]"
    );
    assert_eq!(
        order(&updated, 0, 1)?.properties["text"],
        "#2 [Bread in London]"
    );

    let root = DataScope::default();
    assert_eq!(processor.resolve_value("orders", &root, &json!({
        "@call": "decorate", "catalogId": "example:display", "args": {"value": {"@path": "/count"}}
    }), &functions)?, "[5]");
    assert!(matches!(
        processor.resolve_value("orders", &root, &json!({"@call": "confirm", "catalogId": "example:display"}), &functions),
        Err(ResolveError::UserActivation(name)) if name == "confirm"
    ));
    assert!(matches!(
        processor.resolve_value("orders", &root, &json!({"@call": "label", "args": {"name": "Tea", "city": "London"}}), &functions),
        Err(ResolveError::Catalog(CatalogError::UnknownEntry(name))) if name == "label"
    ));
    assert_eq!(confirms.get(), 0);
    Ok(())
}

fn snapshot(
    processor: &MessageProcessor,
    functions: &LocalFunctions<'_>,
) -> Result<ResolvedComponent, Box<dyn Error>> {
    Ok(processor
        .resolve_components("orders", functions)?
        .ok_or("missing order tree")?)
}

fn order(
    tree: &ResolvedComponent,
    group: usize,
    item: usize,
) -> Result<&ResolvedComponent, Box<dyn Error>> {
    Ok(tree
        .children
        .get(group)
        .and_then(|group| group.children.get(item))
        .ok_or("missing order")?)
}
