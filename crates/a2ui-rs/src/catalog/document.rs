use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::{CATALOG, Catalog, CatalogError, common_ref, entries, invalid, schema_nodes};

const SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// Selects or defines an entry in a derived catalog.
pub enum CatalogEntry {
    /// Keeps a named entry from the source catalog.
    Keep(String),
    /// Adds a schema or replaces the schema with this name.
    Define {
        /// The entry name.
        name: String,
        /// The JSON schema for the entry.
        schema: Value,
    },
}

/// Entry lists and instructions for a derived catalog.
#[derive(Default)]
pub struct CatalogDerivation {
    /// Component entries. An absent list keeps all source components.
    pub components: Option<Vec<CatalogEntry>>,
    /// Function entries. An absent list keeps all source functions.
    pub functions: Option<Vec<CatalogEntry>>,
    /// Instructions for the new catalog.
    pub instructions: Option<String>,
}

impl Catalog {
    /// Builds a catalog with a new ID and the selected entries.
    ///
    /// The new catalog keeps the source protocol version and theme.
    ///
    /// # Errors
    /// Returns an error if a selected name is absent or a new entry is invalid.
    pub fn derive(
        &self,
        catalog_id: &str,
        options: CatalogDerivation,
    ) -> Result<Self, CatalogError> {
        let mut document = self.document.clone();
        document["catalogId"] = json!(catalog_id);
        let fields = document
            .as_object_mut()
            .ok_or_else(|| invalid("Expected a catalog object"))?;
        for key in ["$id", "title", "description", "instructions"] {
            fields.remove(key);
        }
        if let Some(instructions) = options.instructions {
            fields.insert("instructions".to_owned(), json!(instructions));
        }
        for (kind, union, selected) in [
            ("components", "anyComponent", options.components),
            ("functions", "anyFunction", options.functions),
        ] {
            if let Some(selected) = selected {
                let current = entries(&document, kind)?;
                let mut picked = Map::new();
                for entry in selected {
                    let (name, schema) = match entry {
                        CatalogEntry::Keep(name) => {
                            let schema = current
                                .get(&name)
                                .cloned()
                                .ok_or_else(|| CatalogError::UnknownEntry(name.clone()))?;
                            (name, schema)
                        }
                        CatalogEntry::Define { name, schema } => (name, schema),
                    };
                    picked.insert(name, schema);
                }
                let changed = current.keys().ne(picked.keys());
                if let Some(defs) = document.get_mut("$defs").and_then(Value::as_object_mut)
                    && defs.contains_key(union)
                    && changed
                {
                    defs.insert(union.to_owned(), entry_union(kind, &picked));
                }
                document[kind] = Value::Object(picked);
            }
        }
        prune_replaced_defs(&self.document, &mut document)?;
        Self::from_json(document, &self.common_types)
    }
}

pub(super) fn filter_union(
    document: &mut Value,
    kind: &str,
    union: &str,
) -> Result<(), CatalogError> {
    let definition = &document["$defs"][union];
    let names = if let Some(items) = definition["oneOf"].as_array() {
        let prefix = format!("#/{kind}/");
        items
            .iter()
            .map(|item| {
                let reference = item["$ref"]
                    .as_str()
                    .and_then(|reference| reference.strip_prefix(&prefix))
                    .ok_or_else(|| invalid(format!("Invalid {union} entry reference")))?;
                let decoded = percent_encoding::percent_decode_str(reference)
                    .decode_utf8()
                    .map_err(|error| invalid(error.to_string()))?;
                Ok(decoded.replace("~1", "/").replace("~0", "~"))
            })
            .collect::<Result<BTreeSet<_>, CatalogError>>()?
    } else if definition
        .get("not")
        .is_some_and(|schema| schema == &json!({}))
    {
        BTreeSet::new()
    } else {
        return Ok(());
    };
    let current = entries(document, kind)?;
    for name in &names {
        if !current.contains_key(name) {
            return Err(CatalogError::UnknownEntry(name.clone()));
        }
    }
    if let Some(entries) = document.get_mut(kind).and_then(Value::as_object_mut) {
        entries.retain(|name, _| names.contains(name));
    }
    Ok(())
}

fn entry_union(kind: &str, entries: &Map<String, Value>) -> Value {
    if entries.is_empty() {
        return json!({"not": {}});
    }
    let branches: Vec<_> = entries
        .keys()
        .map(|name| json!({"$ref": format!("#/{kind}/{name}")}))
        .collect();
    if kind == "components" {
        json!({"oneOf": branches, "discriminator": {"propertyName": "component"}})
    } else {
        json!({"oneOf": branches})
    }
}

fn schema_references(schema: &Value) -> impl Iterator<Item = &str> {
    schema_nodes(schema).flat_map(|node| {
        ["$ref", "$dynamicRef"]
            .into_iter()
            .filter_map(move |key| node[key].as_str())
    })
}

fn local_defs_refs(schema: &Value) -> impl Iterator<Item = String> + '_ {
    schema_references(schema).filter_map(|reference| {
        let fragment = reference.strip_prefix("#/$defs/")?;
        let decoded = percent_encoding::percent_decode_str(fragment)
            .decode_utf8()
            .ok()?;
        Some(
            decoded
                .split('/')
                .next()?
                .replace("~1", "/")
                .replace("~0", "~"),
        )
    })
}

fn reachable_defs<'a>(
    roots: impl IntoIterator<Item = &'a Value>,
    defs: &Map<String, Value>,
) -> BTreeSet<String> {
    let mut pending: Vec<_> = roots.into_iter().flat_map(local_defs_refs).collect();
    let mut seen = BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !seen.contains(&name)
            && let Some(definition) = defs.get(&name)
        {
            pending.extend(local_defs_refs(definition));
            seen.insert(name);
        }
    }
    seen
}

fn schema_roots(document: &Value) -> impl Iterator<Item = &Value> {
    ["components", "functions"]
        .into_iter()
        .filter_map(|kind| document[kind].as_object())
        .flat_map(Map::values)
        .chain(document.get("theme"))
        .chain(document["$defs"].get("theme"))
}

fn prune_replaced_defs(source: &Value, document: &mut Value) -> Result<(), CatalogError> {
    let defs = entries(source, "$defs")?;
    let original = reachable_defs(schema_roots(source), defs);
    let retained = reachable_defs(
        schema_roots(document).chain(
            defs.iter()
                .filter(|(name, _)| !original.contains(name.as_str()))
                .map(|(_, schema)| schema),
        ),
        defs,
    );
    let removed: Vec<_> = original
        .difference(&retained)
        .filter(|name| !matches!(name.as_str(), "anyComponent" | "anyFunction" | "theme"))
        .cloned()
        .collect();
    if let Some(defs) = document.get_mut("$defs").and_then(Value::as_object_mut) {
        for name in removed {
            defs.remove(&name);
        }
    }
    Ok(())
}

impl Catalog {
    /// Returns a validation schema with local JSON Schema references.
    ///
    /// The schema includes the referenced common types and keeps catalog leaf definitions.
    ///
    /// # Errors
    /// Returns an error if a schema reference cannot be resolved within the catalog.
    pub fn validation_schema(&self) -> Result<Value, CatalogError> {
        let common = entries(&self.common_types, "$defs")?;
        let own_defs = entries(&self.document, "$defs")?;
        let mut schema = json!({
            "$schema": SCHEMA_DIALECT,
            "catalogId": self.id(),
            "protocolVersion": "1.0",
        });
        if let Some(instructions) = self.document.get("instructions") {
            schema["instructions"] = instructions.clone();
        }

        let components = entries(&self.document, "components")?;
        let functions = entries(&self.document, "functions")?;
        if !components.is_empty() {
            let exported = components
                .iter()
                .map(|(name, definition)| {
                    Ok((
                        name.clone(),
                        validation_component(name, definition, own_defs, common)?,
                    ))
                })
                .collect::<Result<Map<_, _>, CatalogError>>()?;
            schema["components"] = Value::Object(exported);
        }
        if !functions.is_empty() {
            schema["functions"] = Value::Object(functions.clone());
        }

        let mut defs = own_defs.clone();
        defs.entry("anyComponent")
            .or_insert_with(|| entry_union("components", components));
        defs.entry("anyFunction")
            .or_insert_with(|| entry_union("functions", functions));
        if let Some(theme) = self.document.get("theme") {
            defs.entry("theme").or_insert_with(|| theme.clone());
        }
        for kind in ["components", "functions"] {
            if let Some(entries) = schema.get_mut(kind).and_then(Value::as_object_mut) {
                for definition in entries.values_mut() {
                    normalize_refs(definition, Some(common), own_defs);
                }
            }
        }
        for definition in defs.values_mut() {
            normalize_refs(definition, Some(common), own_defs);
        }

        let mut pending: Vec<_> = schema_roots(&schema)
            .chain(defs.values())
            .flat_map(local_defs_refs)
            .collect();
        while let Some(name) = pending.pop() {
            if !defs.contains_key(&name) {
                let mut definition = common
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| invalid(format!("Missing schema definition '{name}'")))?;
                normalize_refs(&mut definition, None, own_defs);
                pending.extend(local_defs_refs(&definition));
                defs.insert(name, definition);
            }
        }
        schema["$defs"] = Value::Object(defs);
        for reference in schema_roots(&schema)
            .chain(entries(&schema, "$defs")?.values())
            .flat_map(schema_references)
        {
            let target = reference.strip_prefix('#').and_then(|fragment| {
                let fragment = percent_encoding::percent_decode_str(fragment)
                    .decode_utf8()
                    .ok()?;
                schema.pointer(&fragment)
            });
            if target.is_none() {
                return Err(invalid(format!(
                    "Unresolved schema reference '{reference}'"
                )));
            }
        }
        Ok(schema)
    }
}

fn validation_component(
    name: &str,
    definition: &Value,
    local: &Map<String, Value>,
    common: &Map<String, Value>,
) -> Result<Value, CatalogError> {
    let mut schema = definition
        .as_object()
        .cloned()
        .ok_or_else(|| invalid(format!("Expected a component schema for '{name}'")))?;
    let mut properties = Map::new();
    let mut required = BTreeSet::new();
    let mut remaining = Vec::new();
    if let Some(mixins) = schema.remove("allOf") {
        let mixins = mixins
            .as_array()
            .ok_or_else(|| invalid("Expected component allOf schemas"))?;
        let mut active = BTreeSet::new();
        for mixin in mixins {
            merge_component_mixin(
                mixin,
                local,
                common,
                &mut properties,
                &mut required,
                &mut remaining,
                &mut active,
            )?;
        }
    }
    merge_component_fields(&mut schema, &mut properties, &mut required);
    let component_tag = properties
        .get("component")
        .and_then(|field| field["const"].as_str())
        .unwrap_or(name);
    properties.insert("component".to_owned(), json!({"const": component_tag}));
    properties.insert("id".to_owned(), json!({"$ref": "#/$defs/ComponentId"}));
    required.extend(["component".to_owned(), "id".to_owned()]);
    if let Some(closure) = schema.remove("additionalProperties") {
        schema.entry("unevaluatedProperties").or_insert(closure);
    }
    if !remaining.is_empty() {
        schema.insert("allOf".to_owned(), Value::Array(remaining));
    }
    schema.insert("type".to_owned(), json!("object"));
    schema.insert("properties".to_owned(), Value::Object(properties));
    schema.insert("required".to_owned(), json!(required));
    Ok(Value::Object(schema))
}

fn merge_component_fields(
    schema: &mut Map<String, Value>,
    properties: &mut Map<String, Value>,
    required: &mut BTreeSet<String>,
) {
    if let Some(fields) = schema
        .remove("properties")
        .and_then(|value| value.as_object().cloned())
    {
        properties.extend(fields);
    }
    if let Some(Value::Array(names)) = schema.remove("required") {
        required.extend(
            names
                .into_iter()
                .filter_map(|name| name.as_str().map(str::to_owned)),
        );
    }
}

fn merge_component_mixin<'a>(
    mixin: &'a Value,
    local: &'a Map<String, Value>,
    common: &'a Map<String, Value>,
    properties: &mut Map<String, Value>,
    required: &mut BTreeSet<String>,
    remaining: &mut Vec<Value>,
    active: &mut BTreeSet<(&'a str, bool)>,
) -> Result<(), CatalogError> {
    let Some(mut schema) = mixin.as_object().cloned() else {
        remaining.push(mixin.clone());
        return Ok(());
    };
    if let Some(reference) = mixin["$ref"].as_str() {
        let standard =
            common_ref(reference).and_then(|reference| reference.strip_prefix("#/$defs/"));
        let name = standard.or_else(|| reference.strip_prefix("#/$defs/"));
        if let Some(name) = name {
            let defs = if standard.is_some() { common } else { local };
            let key = (name, standard.is_some());
            if let Some(target) = defs.get(name)
                && active.insert(key)
            {
                let mut target = target.clone();
                if standard == Some("ComponentCommon")
                    && let Some(target) = target["properties"].as_object_mut()
                {
                    target.remove("@catalogId");
                }
                if standard.is_some() {
                    let target = target
                        .as_object_mut()
                        .ok_or_else(|| invalid("Expected a common component mixin"))?;
                    merge_component_fields(target, properties, required);
                } else {
                    merge_component_mixin(
                        defs.get(name)
                            .ok_or_else(|| invalid("Missing component mixin"))?,
                        local,
                        common,
                        properties,
                        required,
                        remaining,
                        active,
                    )?;
                }
                schema.remove("$ref");
                active.remove(&key);
            }
        }
    }
    if let Some(nested) = mixin["allOf"].as_array() {
        for item in nested {
            merge_component_mixin(item, local, common, properties, required, remaining, active)?;
        }
        schema.remove("allOf");
    }
    merge_component_fields(&mut schema, properties, required);
    if schema.get("type") == Some(&json!("object")) {
        schema.remove("type");
    }
    schema.remove("description");
    if !schema.is_empty() {
        remaining.push(Value::Object(schema));
    }
    Ok(())
}

fn normalize_refs(
    value: &mut Value,
    descriptions: Option<&Map<String, Value>>,
    own_defs: &Map<String, Value>,
) {
    let Some(schema) = value.as_object_mut() else {
        return;
    };
    for keyword in ["$ref", "$dynamicRef"] {
        if let Some(reference) = schema.get(keyword).and_then(Value::as_str) {
            let standard = common_ref(reference);
            let local = standard.or_else(|| {
                let (resource, fragment) = reference.split_once('#')?;
                if matches!(resource, "catalog.json" | CATALOG) && fragment.starts_with('/') {
                    reference.get(resource.len()..)
                } else {
                    None
                }
            });
            let name = standard
                .or(Some(reference))
                .and_then(|reference| reference.strip_prefix("#/$defs/"))
                .filter(|name| standard.is_some() || !own_defs.contains_key(*name));
            let description = name
                .and_then(|name| descriptions?.get(name)?.get("description"))
                .cloned();
            if let Some(local) = local {
                schema.insert(keyword.to_owned(), json!(local));
            }
            if let Some(description) = description {
                schema.entry("description").or_insert(description);
            }
        }
    }

    for key in [
        "$defs",
        "properties",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(children) = schema.get_mut(key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                normalize_refs(child, descriptions, own_defs);
            }
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = schema.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                normalize_refs(child, descriptions, own_defs);
            }
        }
    }
    for key in [
        "items",
        "contains",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "propertyNames",
        "if",
        "then",
        "else",
        "not",
        "contentSchema",
    ] {
        if let Some(child) = schema.get_mut(key) {
            normalize_refs(child, descriptions, own_defs);
        }
    }
}
