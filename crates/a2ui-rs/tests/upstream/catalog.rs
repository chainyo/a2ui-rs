use std::path::Path;

use a2ui_rs::basic_catalog::register_validation_functions;
use a2ui_rs::catalog::{Catalog, CatalogDerivation, CatalogEntry, CatalogError};
use a2ui_rs::resolution::{DataScope, LocalFunctions};
use a2ui_rs::validation::MessageProcessor;
use serde_json::{Value, json};

use super::{Case, TestError, assert_rejection, catalog_document, read_json};

const BASIC_PATH: &str = "catalogs/basic/v1/catalog.json";
const BASIC_ID: &str = "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";

pub(super) fn execute(upstream: &Path, case: &Case, common: &Value) -> Result<(), TestError> {
    let document = match (
        &case.catalog,
        &case.catalog_path,
        case.builtin_catalog.as_deref(),
    ) {
        (Some(document), None, None) => catalog_document(case, document)?,
        (None, Some(path), None) => read_json(upstream, path)?,
        (None, None, Some("basic")) => read_json(upstream, BASIC_PATH)?,
        _ => return Err("expected one catalog input".into()),
    };
    let result = Catalog::from_json(document.clone(), common);
    if case.action == "from_json" {
        return assert_loaded(result, case);
    }
    let catalog = result?;
    match case.action.as_str() {
        "round_trip" => {
            assert_schema(catalog.as_json(), &document)?;
            let reloaded = Catalog::from_json(
                serde_json::from_slice(&serde_json::to_vec(catalog.as_json())?)?,
                common,
            )?;
            assert_schema(reloaded.as_json(), &document)
        }
        "to_json" | "validation_schema" => {
            let path = case
                .expect_file
                .as_deref()
                .ok_or("missing expected catalog file")?;
            let actual = if case.action == "validation_schema" {
                let schema = catalog.validation_schema()?;
                assert_self_contained(&schema, &schema)?;
                schema
            } else {
                serde_json::to_value(catalog.as_json())?
            };
            assert_schema(&actual, &read_json(upstream, path)?)
        }
        "derive" => assert_derivation(&catalog, case),
        _ => Err("unsupported catalog action".into()),
    }
}

fn assert_loaded(result: Result<Catalog, CatalogError>, case: &Case) -> Result<(), TestError> {
    let catalog = match (result, &case.expect_error) {
        (Ok(catalog), None) => catalog,
        (Err(error), Some(expected)) => {
            let category = if matches!(error, CatalogError::InvalidComponentMetadata { .. }) {
                "ValidationError"
            } else {
                "CatalogError"
            };
            return assert_rejection(expected, category, None, &error.to_string());
        }
        (Ok(_), Some(_)) => return Err("expected catalog error".into()),
        (Err(error), None) => return Err(error.into()),
    };
    if let Some(expected) = &case.expect {
        let mut fields = expected
            .as_object()
            .ok_or("expected catalog fields")?
            .clone();
        for (key, should_pass) in [("validComponents", true), ("invalidComponents", false)] {
            if let Some(samples) = fields.remove(key) {
                for sample in samples.as_array().ok_or("missing catalog samples")? {
                    let result =
                        catalog.validate_component(&serde_json::from_value(sample.clone())?);
                    if result.is_ok() != should_pass {
                        return Err(format!(
                            "unexpected catalog validation result {result:?} for {sample}"
                        )
                        .into());
                    }
                }
            }
        }
        if fields
            .remove("selfContained")
            .and_then(|field| field.as_bool())
            == Some(true)
        {
            let schema = catalog.validation_schema()?;
            assert_self_contained(&schema, &schema)?;
        }
        let matches_builtin = fields
            .remove("matchesBuiltinCatalog")
            .and_then(|field| field.as_bool())
            == Some(true);
        assert_catalog(catalog.as_json(), &Value::Object(fields))?;
        if matches_builtin {
            assert_basic_catalog(catalog)?;
        }
    }
    Ok(())
}

fn assert_basic_catalog(catalog: Catalog) -> Result<(), TestError> {
    if catalog.id() != BASIC_ID {
        return Err("expected the SDK Basic Catalog ID".into());
    }
    let mut functions = LocalFunctions::default();
    register_validation_functions(&mut functions, BASIC_ID);
    let mut processor = MessageProcessor::default();
    processor.add_catalog(catalog)?;
    processor.apply_json(&json!({"version": "1.0", "createSurface": {
        "surfaceId": "basic", "catalogId": BASIC_ID, "components": [
            {"id": "root", "component": "TextField", "label": "Status", "value": "Ready"}
        ]
    }}))?;
    let result = processor.resolve_value(
        "basic",
        &DataScope::default(),
        &json!({"@call": "required", "args": {"value": "Ready"}}),
        &functions,
    )?;
    if result["valid"] != true {
        return Err(format!("unexpected built-in required result {result}").into());
    }
    Ok(())
}

fn assert_derivation(catalog: &Catalog, case: &Case) -> Result<(), TestError> {
    let args = case.args.as_ref().ok_or("missing derive arguments")?;
    let id = args["catalogId"]
        .as_str()
        .ok_or("missing derived catalog ID")?;
    let options = CatalogDerivation {
        components: selected_entries(args.get("components"))?,
        functions: selected_entries(args.get("functions"))?,
        instructions: args["instructions"].as_str().map(str::to_owned),
    };
    match (catalog.derive(id, options), &case.expect_error) {
        (Ok(derived), None) => assert_catalog(
            derived.as_json(),
            case.expect
                .as_ref()
                .ok_or("missing derived catalog expectation")?,
        ),
        (Err(error), Some(expected)) => {
            assert_rejection(expected, "CatalogError", None, &error.to_string())
        }
        (Ok(_), Some(_)) => Err("expected catalog derivation to fail".into()),
        (Err(error), None) => Err(error.into()),
    }
}

fn selected_entries(entries: Option<&Value>) -> Result<Option<Vec<CatalogEntry>>, TestError> {
    entries
        .map(|entries| {
            entries
                .as_array()
                .ok_or("expected catalog entry list")?
                .iter()
                .map(|entry| {
                    if let Some(name) = entry.as_str() {
                        return Ok(CatalogEntry::Keep(name.to_owned()));
                    }
                    let fields = entry
                        .as_object()
                        .filter(|fields| fields.len() == 1)
                        .ok_or("expected a named schema")?;
                    let (name, schema) = fields.iter().next().ok_or("missing named schema")?;
                    Ok(CatalogEntry::Define {
                        name: name.clone(),
                        schema: schema.clone(),
                    })
                })
                .collect()
        })
        .transpose()
}

fn assert_self_contained(root: &Value, value: &Value) -> Result<(), TestError> {
    if let Some(object) = value.as_object() {
        for keyword in ["$ref", "$dynamicRef"] {
            if let Some(reference) = value[keyword].as_str()
                && reference
                    .strip_prefix('#')
                    .and_then(|pointer| root.pointer(pointer))
                    .is_none()
            {
                return Err(
                    format!("validation schema has an unresolved reference {reference}").into(),
                );
            }
        }
        for child in object.values() {
            assert_self_contained(root, child)?;
        }
    } else if let Some(items) = value.as_array() {
        for child in items {
            assert_self_contained(root, child)?;
        }
    }
    Ok(())
}

fn assert_schema(actual: &Value, expected: &Value) -> Result<(), TestError> {
    let mut actual = actual.clone();
    let mut expected = expected.clone();
    normalize_sets(&mut actual);
    normalize_sets(&mut expected);
    if let Some((path, actual, expected)) = first_diff(&actual, &expected, String::new()) {
        return Err(format!(
            "catalog schema differs at {path}: expected {expected:?}, got {actual:?}"
        )
        .into());
    }
    Ok(())
}

fn normalize_sets(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        for (key, child) in object {
            normalize_sets(child);
            if matches!(key.as_str(), "required" | "enum")
                && let Some(items) = child.as_array_mut()
            {
                items.sort_by_cached_key(Value::to_string);
            }
        }
    } else if let Some(items) = value.as_array_mut() {
        for child in items {
            normalize_sets(child);
        }
    }
}

fn first_diff<'a>(
    actual: &'a Value,
    expected: &'a Value,
    path: String,
) -> Option<(String, Option<&'a Value>, Option<&'a Value>)> {
    if actual == expected {
        return None;
    }
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            for key in actual.keys().chain(expected.keys()) {
                match (actual.get(key), expected.get(key)) {
                    (Some(a), Some(b)) => {
                        if let Some(diff) = first_diff(a, b, format!("{path}/{key}")) {
                            return Some(diff);
                        }
                    }
                    (a, b) => return Some((format!("{path}/{key}"), a, b)),
                }
            }
            None
        }
        (Value::Array(actual), Value::Array(expected)) if actual.len() == expected.len() => actual
            .iter()
            .zip(expected)
            .enumerate()
            .find_map(|(index, (a, b))| first_diff(a, b, format!("{path}/{index}"))),
        _ => Some((path, Some(actual), Some(expected))),
    }
}
fn assert_catalog(document: &Value, expected: &Value) -> Result<(), TestError> {
    for (key, value) in expected.as_object().ok_or("expected catalog fields")? {
        let actual = match key.as_str() {
            "absentKeys" => {
                for key in value.as_array().ok_or("expected absent catalog fields")? {
                    let key = key.as_str().ok_or("expected a catalog field name")?;
                    if document.get(key).is_some() {
                        return Err(format!("catalog retains {key}").into());
                    }
                }
                continue;
            }
            "componentMetadata" => {
                let fields = value
                    .as_object()
                    .ok_or("expected component metadata fields")?;
                Value::Object(
                    fields
                        .keys()
                        .map(|name| {
                            (
                                name.clone(),
                                document["components"][name]["metadata"].clone(),
                            )
                        })
                        .collect(),
                )
            }
            "components" | "functions" if value.is_array() => {
                let actual = document[key].as_object().ok_or("missing catalog entries")?;
                let names = actual
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>();
                let expected_names = value
                    .as_array()
                    .ok_or("expected names")?
                    .iter()
                    .map(|name| name.as_str().ok_or("expected catalog entry name"))
                    .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
                if names != expected_names {
                    return Err(format!("expected {key} {expected_names:?}, got {names:?}").into());
                }
                continue;
            }
            _ => document[key].clone(),
        };
        if actual != *value {
            return Err(format!("expected {key} {value}, got {actual}").into());
        }
    }
    Ok(())
}
