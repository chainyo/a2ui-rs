use std::collections::BTreeMap;
use std::path::Path;

use a2ui_basic_catalog::{FormattingOptions, register_formatting_functions};
use a2ui_rs::catalog::{Catalog, CatalogError};
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::validation::{MessageProcessor, ValidationError};
use serde_json::{Value, json};

use super::{Case, TestError, assert_components, assert_error, read_json};

pub(super) fn execute(upstream: &Path, case: &Case, common: &Value) -> Result<(), TestError> {
    if case.action == "validate" {
        return resolve_tree(upstream, case, common);
    }
    let args = case
        .args
        .as_ref()
        .ok_or("missing catalog selection arguments")?;
    let surface = &args["surface"];
    let surface_id = text(&surface["id"])?;
    let default = text(&surface["defaultCatalogId"])?;
    let supported = surface["supportedCatalogIds"]
        .as_array()
        .ok_or("missing supported catalogs")?;
    let mut documents = BTreeMap::new();
    for supported in supported {
        let id = text(supported)?;
        let version = args["catalogs"][id]["protocolVersion"]
            .as_str()
            .unwrap_or("1.0");
        documents.insert(
            id,
            json!({
                "catalogId": id,
                "protocolVersion": version,
                "components": {},
                "functions": {}
            }),
        );
    }

    let components = args.get("components").and_then(Value::as_object);
    if let Some(expected) = &case.expect_error {
        return if let Some(components) = components {
            let mut processor = processor(documents, common, surface_id, default)?;
            reject_component(
                &mut processor,
                surface_id,
                &component_payloads(components)?,
                expected,
            )
        } else {
            reject_version(&documents, common, surface_id, default, expected)
        };
    }

    let expected = case
        .expect_selected
        .as_deref()
        .ok_or("missing selected catalog")?;
    if let Some(components) = components {
        select_components(
            documents,
            common,
            surface_id,
            default,
            component_payloads(components)?,
            expected,
        )
    } else {
        select_function(
            documents,
            common,
            surface_id,
            default,
            &args["functionCall"],
            expected,
        )
    }
}

fn reject_component(
    processor: &mut MessageProcessor,
    surface_id: &str,
    components: &[Value],
    expected: &Value,
) -> Result<(), TestError> {
    let before = processor.get(surface_id).cloned();
    let result = processor.apply(update_components(surface_id, components)?);
    match result {
        Err(ValidationError::UnknownCatalog(id))
            if processor.get(surface_id) == before.as_ref() =>
        {
            assert_error(
                expected,
                "CatalogError",
                &format!("Catalog '{id}' is not supported by surface '{surface_id}'."),
            )
        }
        result => Err(format!("unexpected component update result {result:?}").into()),
    }
}

fn reject_version(
    documents: &BTreeMap<&str, Value>,
    common: &Value,
    surface_id: &str,
    default: &str,
    expected: &Value,
) -> Result<(), TestError> {
    let surface_version =
        text(&documents.get(default).ok_or("missing default catalog")?["protocolVersion"])?;
    let mut processor = MessageProcessor::default();
    let mut rejected = false;
    for (&id, document) in documents {
        match Catalog::from_json(document.clone(), common) {
            Err(CatalogError::InvalidCatalog(message))
                if document["protocolVersion"] != surface_version
                    && message.contains("must declare protocolVersion 1.0") =>
            {
                assert_error(
                    expected,
                    "CatalogError",
                    &format!(
                        "Protocol version mismatch: cannot mix catalog '{id}' ({}) with surface version {surface_version}.",
                        text(&document["protocolVersion"])?
                    ),
                )?;
                rejected = true;
            }
            Ok(catalog) => processor.add_catalog(catalog)?,
            Err(error) => return Err(error.into()),
        }
    }
    if !rejected {
        return Err("expected incompatible catalog registration to fail".into());
    }
    create_surface(&mut processor, surface_id, default)
}

fn select_components(
    mut documents: BTreeMap<&str, Value>,
    common: &Value,
    surface_id: &str,
    default: &str,
    components: Vec<Value>,
    expected: &str,
) -> Result<(), TestError> {
    let last = components.last().ok_or("missing selection components")?;
    for component in &components {
        // Define each component only in the catalog that must supply it.
        let owner = if component["id"] == last["id"] {
            expected
        } else {
            component["@catalogId"].as_str().unwrap_or(default)
        };
        let kind = text(&component["component"])?;
        let mut properties = component.as_object().ok_or("invalid component")?.clone();
        properties.remove("id");
        properties.remove("@catalogId");
        for value in properties.values_mut() {
            *value = json!({"const": value});
        }
        documents
            .get_mut(owner)
            .ok_or("missing component catalog")?["components"][kind] =
            json!({"type": "object", "properties": properties});
    }
    documents
        .get_mut(default)
        .ok_or("missing default catalog")?["components"]["Container"] = json!({
        "type": "object",
        "properties": {
            "component": {"const": "Container"},
            "children": {"$ref": "common_types.json#/$defs/ChildList"}
        }
    });
    let mut processor = processor(documents, common, surface_id, default)?;
    let children = components
        .iter()
        .map(|component| text(&component["id"]).map(str::to_owned))
        .collect::<Result<Vec<_>, _>>()?;
    let root = json!({"id": "root", "component": "Container", "children": children});
    let batch: Vec<_> = std::iter::once(root).chain(components).collect();
    processor.apply(update_components(surface_id, &batch)?)?;
    let root = processor
        .resolve_components(surface_id, &LocalFunctions::default())?
        .ok_or("missing resolved catalog tree")?;
    if !root
        .children
        .iter()
        .map(|child| child.id.as_str())
        .eq(children.iter().map(String::as_str))
    {
        return Err("missing or reordered catalog components".into());
    }
    Ok(())
}

fn select_function(
    mut documents: BTreeMap<&str, Value>,
    common: &Value,
    surface_id: &str,
    default: &str,
    call: &Value,
    expected: &str,
) -> Result<(), TestError> {
    let name = text(&call["call"])?;
    let mut functions = LocalFunctions::default();
    for (&id, document) in &mut documents {
        document["functions"][name] = json!({"type": "object", "properties": {
            "@call": {"const": name}, "args": {"type": "object"}
        }});
        functions.insert(id, name, move |_| Ok(json!(id)));
    }
    let processor = processor(documents, common, surface_id, default)?;
    let call = json!({"@call": name, "catalogId": call["catalogId"], "args": call["args"]});
    let actual = processor.resolve_value(surface_id, &DataScope::default(), &call, &functions)?;
    if actual != expected {
        return Err(format!("expected catalog {expected}, got {actual}").into());
    }
    Ok(())
}

fn processor(
    documents: BTreeMap<&str, Value>,
    common: &Value,
    surface_id: &str,
    default: &str,
) -> Result<MessageProcessor, TestError> {
    let mut processor = MessageProcessor::default();
    for document in documents.into_values() {
        processor.add_catalog(Catalog::from_json(document, common)?)?;
    }
    create_surface(&mut processor, surface_id, default)?;
    Ok(processor)
}

fn create_surface(
    processor: &mut MessageProcessor,
    id: &str,
    catalog: &str,
) -> Result<(), TestError> {
    processor.apply(serde_json::from_value(json!({
        "version": "1.0",
        "createSurface": {"surfaceId": id, "catalogId": catalog}
    }))?)?;
    Ok(())
}

fn update_components(
    surface_id: &str,
    components: &[Value],
) -> Result<a2ui_rs::protocol::AgentToRendererMessage, serde_json::Error> {
    serde_json::from_value(json!({"version": "1.0", "updateComponents": {
        "surfaceId": surface_id, "components": components
    }}))
}

fn component_payloads(
    components: &serde_json::Map<String, Value>,
) -> Result<Vec<Value>, TestError> {
    components
        .iter()
        .map(|(id, component)| {
            let mut component = component.clone();
            component["id"] = json!(id);
            use_component_override(&mut component)?;
            Ok(component)
        })
        .collect()
}

fn use_component_override(component: &mut Value) -> Result<(), TestError> {
    let fields = component
        .as_object_mut()
        .ok_or("expected component object")?;
    if let Some(id) = fields.remove("catalogId")
        && fields.insert("@catalogId".to_owned(), id).is_some()
    {
        return Err("fixture has two catalog overrides".into());
    }
    Ok(())
}

fn resolve_tree(upstream: &Path, case: &Case, common: &Value) -> Result<(), TestError> {
    let mut processor = MessageProcessor::default();
    let mut basic = read_json(upstream, "catalogs/basic/v1/catalog.json")?;
    basic["catalogId"] = json!("basic");
    processor.add_catalog(Catalog::from_json(basic, common)?)?;
    let catalogs = case
        .catalogs
        .as_ref()
        .and_then(Value::as_array)
        .ok_or("missing inline catalogs")?;
    for document in catalogs {
        let mut document = document.clone();
        // The fixture uses a local name for the standard DynamicString schema.
        document["$defs"]["DynamicString"] =
            json!({"$ref": "common_types.json#/$defs/DynamicString"});
        processor.add_catalog(Catalog::from_json(document, common)?)?;
    }
    if case.steps.is_empty() {
        return Err("missing catalog resolution steps".into());
    }
    for step in &case.steps {
        for message in step["payload"]
            .as_array()
            .ok_or("missing catalog payload")?
        {
            let mut message = message.clone();
            if let Some(components) = message
                .pointer_mut("/createSurface/components")
                .and_then(Value::as_array_mut)
            {
                for component in components {
                    use_component_override(component)?;
                }
            }
            processor.apply(serde_json::from_value(message)?)?;
        }
    }
    let mut functions = LocalFunctions::default();
    register_formatting_functions(
        &mut functions,
        "basic",
        FormattingOptions::new("en-US", "UTC")?,
    );
    assert_components(
        &processor,
        &functions,
        case.expect
            .as_ref()
            .ok_or("missing resolved catalog tree expectation")?,
    )
}

fn text(value: &Value) -> Result<&str, TestError> {
    value
        .as_str()
        .ok_or_else(|| format!("expected string, got {value}").into())
}
