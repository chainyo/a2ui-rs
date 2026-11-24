//! Offline A2UI v1.0 catalog validation.

use jsonschema::error::ValidationErrorKind;
use jsonschema::paths::Location;
use jsonschema::{Draft, Registry, Validator};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::protocol::{Component, FunctionCall, JsonObject, Metadata};

mod document;

pub use document::{CatalogDerivation, CatalogEntry};

const CATALOG: &str = "https://a2ui.org/specification/v1_0/catalog.json";
const COMMON: &str = "https://a2ui.org/specification/v1_0/common_types.json";

pub(crate) struct ChildReference<'a> {
    pub(crate) id: &'a str,
    pub(crate) field: String,
    pointer: Location,
    pub(crate) collection: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub(crate) enum PropertyKind {
    Action,
    String,
    Number,
    Boolean,
}

#[derive(Default)]
pub(crate) struct PayloadShape<'a> {
    pub(crate) children: Vec<ChildReference<'a>>,
    pub(crate) properties: BTreeMap<*const Value, PropertyKind>,
}

type PayloadCandidate<'a> = (&'a Value, &'a Value, bool, String, Location);

// Each entry keeps its index. A surface stores the number of entries at creation.
#[derive(Default)]
pub(crate) struct CatalogRegistry {
    entries: BTreeMap<String, (usize, Catalog)>,
}

impl CatalogRegistry {
    pub(crate) fn insert(&mut self, catalog: Catalog) -> Result<(), String> {
        let index = self.len();
        match self.entries.entry(catalog.id().to_owned()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((index, catalog));
                Ok(())
            }
            std::collections::btree_map::Entry::Occupied(entry) => Err(entry.key().clone()),
        }
    }

    pub(crate) fn in_registration_order(&self) -> Vec<&Catalog> {
        let mut entries: Vec<_> = self.entries.values().collect();
        entries.sort_unstable_by_key(|(index, _)| *index);
        entries.into_iter().map(|(_, catalog)| catalog).collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn view(&self, count: usize) -> CatalogView<'_> {
        CatalogView {
            registry: self,
            count,
        }
    }

    pub(crate) fn current(&self) -> CatalogView<'_> {
        self.view(self.len())
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CatalogView<'a> {
    registry: &'a CatalogRegistry,
    count: usize,
}

impl<'a> CatalogView<'a> {
    pub(crate) fn get(self, id: &str) -> Option<&'a Catalog> {
        let (index, catalog) = self.registry.entries.get(id)?;
        (*index < self.count).then_some(catalog)
    }
}

/// An immutable catalog with compiled component and function schemas.
pub struct Catalog {
    id: String,
    document: Value,
    common_types: Value,
    components: BTreeMap<String, Validator>,
    functions: BTreeMap<String, Validator>,
}

impl Catalog {
    /// Loads a v1.0 catalog and the v1.0 common types schema supplied by the caller.
    ///
    /// Schema resolution uses only the supplied documents and built-in JSON Schema resources.
    ///
    /// # Errors
    /// Returns an error if the document has an invalid ID, version, name, or schema.
    pub fn from_json(mut document: Value, common_types: &Value) -> Result<Self, CatalogError> {
        let id = document["catalogId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| invalid("catalog_id must be provided or exist in catalog schema"))?
            .to_owned();
        if document["protocolVersion"].as_str() != Some("1.0") {
            return Err(invalid("Catalog must declare protocolVersion 1.0"));
        }
        if !common_types["$defs"].is_object() {
            return Err(invalid("Common types must contain a $defs object"));
        }

        validate_entries(entries(&document, "components")?, "component")?;
        validate_entries(entries(&document, "functions")?, "function")?;
        for kind in ["$defs", "components", "functions"] {
            check_schema_refs(entries(&document, kind)?)?;
        }
        document::filter_union(&mut document, "components", "anyComponent")?;
        document::filter_union(&mut document, "functions", "anyFunction")?;
        let components = entries(&document, "components")?.keys().cloned().collect();
        let functions = entries(&document, "functions")?.keys().cloned().collect();
        let mut local = document.clone();
        if let Some(object) = local.as_object_mut() {
            object.remove("$id");
        }
        let published_common_types = common_types.clone();
        let mut common_types = published_common_types.clone();
        let common_defs = common_types["$defs"]
            .as_object_mut()
            .ok_or_else(|| invalid("Common types must contain a $defs object"))?;
        // The processor resolves each call against its own catalog.
        common_defs.insert("FunctionCall".to_owned(), json!({"oneOf": [
            {
                "type": "object",
                "properties": {"@call": {"type": "string", "not": {"const": "@index"}}, "args": {"type": "object"}, "catalogId": {"type": "string"}},
                "required": ["@call"],
                "additionalProperties": false
            },
            {"$ref": "#/$defs/IndexSystemFunction"}
        ]}));

        if let Some(entries) = local.get_mut("components").and_then(Value::as_object_mut) {
            for schema in entries.values_mut() {
                if let Some(schema) = schema.as_object_mut()
                    && schema.get("additionalProperties") == Some(&Value::Bool(false))
                    && let Some(properties) = schema
                        .entry("properties")
                        .or_insert_with(|| json!({}))
                        .as_object_mut()
                {
                    for field in [
                        "id",
                        "component",
                        "@catalogId",
                        "@metadata",
                        "@accessibility",
                    ] {
                        properties.entry(field).or_insert(Value::Bool(true));
                    }
                }
            }
        }

        let registry = Registry::new()
            .draft(Draft::Draft202012)
            .extend([(CATALOG, local), (COMMON, common_types.clone())])
            .and_then(jsonschema::RegistryBuilder::prepare)
            .map_err(|error| invalid(error.to_string()))?;
        let components = compile_entries(&registry, "components", "ComponentCommon", components)?;
        let functions = compile_entries(&registry, "functions", "FunctionCommon", functions)?;
        Ok(Self {
            id,
            document,
            common_types: published_common_types,
            components,
            functions,
        })
    }

    /// Returns the catalog ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the catalog JSON with its original references.
    ///
    /// Explicit unions control which entries the catalog exports.
    #[must_use]
    pub fn as_json(&self) -> &Value {
        &self.document
    }

    /// Checks a component against this catalog.
    ///
    /// # Errors
    /// Returns an error if the component type is unknown or a property is invalid.
    pub fn validate_component(&self, component: &Component) -> Result<(), CatalogError> {
        self.validate_component_at(component, None)?;
        let mut values: Vec<_> = component.properties.values().collect();
        while let Some(value) = values.pop() {
            match value {
                Value::Object(object) if object.contains_key("@call") => {
                    let call = serde_json::from_value::<FunctionCall>(value.clone())
                        .map_err(|source| invalid(source.to_string()))?;
                    crate::resolution::validate_call_shape(&call)
                        .map_err(|error| invalid(error.to_string()))?;
                    if call.name != "@index"
                        && call.catalog_id.as_deref().is_none_or(|id| id == self.id)
                    {
                        self.validate_function(&call)?;
                    }
                    values.extend(object.get("args"));
                }
                Value::Object(object) => values.extend(object.values()),
                Value::Array(array) => values.extend(array),
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn validate_component_at(
        &self,
        component: &Component,
        base: Option<&str>,
    ) -> Result<(), CatalogError> {
        let checks = self.checks_key(component).map_err(|mut error| {
            if let CatalogError::InvalidPayload { path, .. } = &mut error {
                *path = base.map(|base| format!("{base}/@checks"));
            }
            error
        })?;
        if component.properties.contains_key("checks") && checks == Some("checks") {
            let mut normalized = component.clone();
            if let Some(checks) = normalized.properties.remove("checks") {
                normalized.properties.insert("@checks".to_owned(), checks);
            }
            return Self::validate_entry(&self.components, &component.component, &normalized, base)
                .map_err(|mut error| {
                    if let (
                        Some(base),
                        CatalogError::InvalidPayload {
                            path: Some(path), ..
                        },
                    ) = (base, &mut error)
                        && let Some(suffix) = path
                            .strip_prefix(base)
                            .and_then(|path| path.strip_prefix("/@checks"))
                        && (suffix.is_empty() || suffix.starts_with('/'))
                    {
                        *path = format!("{base}/checks{suffix}");
                    }
                    error
                });
        }
        Self::validate_entry(&self.components, &component.component, component, base)
    }

    pub(crate) fn component_checks<'a>(
        &self,
        component: &'a Component,
    ) -> Result<Option<&'a Value>, CatalogError> {
        Ok(self
            .checks_key(component)?
            .and_then(|key| component.properties.get(key)))
    }

    fn checks_key(&self, component: &Component) -> Result<Option<&'static str>, CatalogError> {
        let canonical = component.properties.contains_key("@checks");
        let bare = component.properties.contains_key("checks");
        if !canonical && !bare {
            return Ok(None);
        }
        let (checkable, custom_checks) =
            self.check_traits(self.component_schema(&component.component));
        if !checkable {
            return Ok(None);
        }
        if canonical && bare && !custom_checks {
            return Err(CatalogError::InvalidPayload {
                name: component.component.clone(),
                message: "Component provides both '@checks' and its 'checks' alias".to_owned(),
                path: None,
            });
        }
        Ok(if canonical {
            Some("@checks")
        } else if !custom_checks {
            Some("checks")
        } else {
            None
        })
    }

    fn check_traits(&self, schema: &Value) -> (bool, bool) {
        let mut pending = vec![(schema, false)];
        let mut visited = BTreeSet::new();
        let mut checkable = false;
        let mut custom_checks = false;
        while let Some((schema, common)) = pending.pop() {
            if !visited.insert(std::ptr::from_ref(schema)) {
                continue;
            }
            if !common && schema["properties"].get("checks").is_some() {
                custom_checks = true;
            }
            if let Some(reference) = schema["$ref"].as_str() {
                let (pointer, common) =
                    common_ref(reference).map_or((reference, common), |pointer| (pointer, true));
                if common && pointer == "#/$defs/Checkable" {
                    checkable = true;
                } else if let Some(target) = self.schema_target(pointer, common) {
                    pending.push((target, common));
                }
            }
            if let Some(schemas) = schema["allOf"].as_array() {
                pending.extend(schemas.iter().map(|schema| (schema, common)));
            }
        }
        (checkable, custom_checks)
    }

    /// Checks a function call against this catalog.
    ///
    /// # Errors
    /// Returns an error if the function name is unknown or an argument is invalid.
    pub fn validate_function(&self, call: &FunctionCall) -> Result<(), CatalogError> {
        self.validate_function_at(call, None)
    }

    pub(crate) fn validate_function_at(
        &self,
        call: &FunctionCall,
        base: Option<&str>,
    ) -> Result<(), CatalogError> {
        Self::validate_entry(&self.functions, &call.name, call, base)
    }

    fn validate_entry(
        entries: &BTreeMap<String, Validator>,
        name: &str,
        instance: &impl serde::Serialize,
        base: Option<&str>,
    ) -> Result<(), CatalogError> {
        let validator = entries
            .get(name)
            .ok_or_else(|| CatalogError::UnknownEntry(name.to_owned()))?;
        let instance =
            serde_json::to_value(instance).map_err(|error| invalid(error.to_string()))?;
        validator.validate(&instance).map_err(|error| {
            if matches!(error.kind(), ValidationErrorKind::Required { .. })
                && let Some(extra) = validator.iter_errors(&instance).find(|extra| {
                    matches!(
                        extra.kind(),
                        ValidationErrorKind::AdditionalProperties { .. }
                            | ValidationErrorKind::UnevaluatedProperties { .. }
                    ) && extra.instance_path() == error.instance_path()
                })
            {
                let location = error.instance_path();
                return CatalogError::InvalidPayload {
                    name: name.to_owned(),
                    message: format!("at '{location}': {}. {}", error.masked(), extra.masked()),
                    path: base.map(|base| format!("{base}{location}")),
                };
            }
            let location = match error.kind() {
                ValidationErrorKind::AdditionalProperties { unexpected }
                | ValidationErrorKind::UnevaluatedProperties { unexpected }
                    if unexpected.len() == 1 =>
                {
                    error.instance_path().join(unexpected[0].as_str())
                }
                ValidationErrorKind::Required { property } if property.is_string() => error
                    .instance_path()
                    .join(property.as_str().unwrap_or_default()),
                _ => error.instance_path().clone(),
            };
            CatalogError::InvalidPayload {
                name: name.to_owned(),
                message: format!("at '{location}': {}", error.masked()),
                path: base.map(|base| format!("{base}{location}")),
            }
        })
    }

    pub(crate) fn component_schema(&self, name: &str) -> &Value {
        &self.document["components"][name]
    }

    pub(crate) fn has_custom_component_property(&self, kind: &str, name: &str) -> bool {
        let mut pending = vec![self.component_schema(kind)];
        let mut visited = BTreeSet::new();
        while let Some(schema) = pending.pop() {
            if !visited.insert(std::ptr::from_ref(schema)) {
                continue;
            }
            let properties = &schema["properties"];
            if let Some(property) = properties.get(name) {
                let inline_common = properties.get("id").is_some()
                    && match name {
                        "catalogId" => property["type"] == "string",
                        "metadata" => {
                            property["type"] == "object"
                                && property["properties"].as_object().is_some_and(|fields| {
                                    fields.len() == 1 && fields.contains_key("extensions")
                                })
                        }
                        _ => false,
                    };
                if !inline_common {
                    return true;
                }
            }
            for keyword in ["allOf", "anyOf", "oneOf"] {
                if let Some(branches) = schema[keyword].as_array() {
                    pending.extend(branches);
                }
            }
            if let Some(reference) = schema["$ref"].as_str()
                && let Some(target) = self.schema_target(reference, false)
            {
                pending.push(target);
            }
        }
        false
    }

    pub(crate) fn function_schema(&self, name: &str) -> &Value {
        &self.document["functions"][name]
    }

    pub(crate) fn preserve_required_local_arguments(
        &self,
        call: &FunctionCall,
        args: &mut JsonObject,
    ) {
        let Some(authored) = &call.args else { return };
        let required = self.function_schema(&call.name)["properties"]["args"]["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        for name in required.filter(|name| authored.contains_key(*name)) {
            args.entry(name.to_owned()).or_insert(Value::Null);
        }
    }

    pub(crate) fn apply_function_defaults(&self, call: &mut FunctionCall) {
        let schema = self.function_schema(&call.name);
        let mut payload = serde_json::json!({});
        if let Some(args) = call.args.take() {
            payload["args"] = Value::Object(args);
        }
        self.apply_defaults(schema, &mut payload, false, 0, &mut BTreeSet::new());
        if let Some(args) = payload["args"].as_object_mut() {
            call.args = Some(std::mem::take(args));
        }
    }

    fn apply_defaults(
        &self,
        schema: &Value,
        value: &mut Value,
        common: bool,
        depth: usize,
        active: &mut BTreeSet<*const Value>,
    ) {
        let address = std::ptr::from_ref(schema);
        if depth >= 50 || !active.insert(address) {
            return;
        }
        if let Some(reference) = schema["$ref"].as_str() {
            let (pointer, common) =
                common_ref(reference).map_or((reference, common), |pointer| (pointer, true));
            if let Some(target) = self.schema_target(pointer, common) {
                self.apply_defaults(target, value, common, depth + 1, active);
            }
        }
        if let Some(schemas) = schema["allOf"].as_array() {
            for schema in schemas {
                self.apply_defaults(schema, value, common, depth + 1, active);
            }
        }
        if let (Some(properties), Some(object)) =
            (schema["properties"].as_object(), value.as_object_mut())
        {
            for (key, schema) in properties {
                if !object.contains_key(key)
                    && let Some(default) = schema.get("default")
                {
                    object.insert(key.clone(), default.clone());
                }
                if let Some(value) = object.get_mut(key) {
                    self.apply_defaults(schema, value, common, depth + 1, &mut BTreeSet::new());
                }
            }
        }
        if let Some(array) = value.as_array_mut() {
            let prefix = schema["prefixItems"]
                .as_array()
                .map_or(&[][..], Vec::as_slice);
            for (index, value) in array.iter_mut().enumerate() {
                let schema = prefix.get(index).unwrap_or(&schema["items"]);
                self.apply_defaults(schema, value, common, depth + 1, &mut BTreeSet::new());
            }
        }
        active.remove(&address);
    }

    pub(crate) fn is_agent_only(&self, name: &str) -> bool {
        self.document["functions"][name]["allowedCallers"] == "agentOnly"
    }

    pub(crate) fn requires_activation(&self, name: &str) -> bool {
        self.document["functions"][name]["requiresUserActivation"] == true
    }

    pub(crate) fn child_references<'a>(
        &'a self,
        schema: &'a Value,
        payload: &'a Value,
    ) -> BTreeMap<&'a str, (String, Location)> {
        let mut references = BTreeMap::new();
        for child in self.inspect_payload(schema, payload).children {
            references
                .entry(child.id)
                .or_insert((child.field, child.pointer));
        }
        references
    }

    pub(crate) fn inspect_payload<'a>(
        &'a self,
        schema: &'a Value,
        payload: &'a Value,
    ) -> PayloadShape<'a> {
        let mut shape = PayloadShape::default();
        let mut candidates = vec![(schema, payload, false, String::new(), Location::new())];
        let mut visited = BTreeSet::new();
        let mut fields = BTreeSet::new();
        while let Some((schema, data, common, path, location)) = candidates.pop() {
            if !visited.insert((std::ptr::from_ref(schema), std::ptr::from_ref(data))) {
                continue;
            }
            if let Some(reference) = schema["$ref"].as_str() {
                let (pointer, common) =
                    common_ref(reference).map_or((reference, common), |pointer| (pointer, true));
                let kind = match (common, pointer) {
                    (true, "#/$defs/Action") => {
                        if data.as_object().is_some_and(|object| {
                            object.len() == 1
                                && object
                                    .get("event")
                                    .or_else(|| object.get("functionCall"))
                                    .is_some_and(Value::is_object)
                        }) {
                            shape
                                .properties
                                .insert(std::ptr::from_ref(data), PropertyKind::Action);
                        }
                        continue;
                    }
                    (true, "#/$defs/DynamicString") => Some(PropertyKind::String),
                    (true, "#/$defs/DynamicNumber") => Some(PropertyKind::Number),
                    (true, "#/$defs/DynamicBoolean") => Some(PropertyKind::Boolean),
                    _ => None,
                };
                if let Some(kind) = kind {
                    shape.properties.insert(std::ptr::from_ref(data), kind);
                    continue;
                }
                let child = match (common, pointer) {
                    (true, "#/$defs/ComponentId") => data.as_str().map(|id| (id, None)),
                    (true, "#/$defs/ChildList") => data["componentId"]
                        .as_str()
                        .zip(data["path"].as_str())
                        .map(|(id, path)| (id, Some(path))),
                    _ => None,
                };
                if let Some((id, collection)) = child {
                    if fields.insert((path.clone(), id)) {
                        let field = if collection.is_some() {
                            format!("{path}.componentId")
                        } else {
                            path
                        };
                        shape.children.push(ChildReference {
                            id,
                            field,
                            pointer: if collection.is_some() {
                                location.join("componentId")
                            } else {
                                location
                            },
                            collection,
                        });
                    }
                    continue;
                }
                if let Some(target) = self.schema_target(pointer, common) {
                    candidates.push((target, data, common, path.clone(), location.clone()));
                }
            }
            if let Some(kind) = inline_dynamic_kind(schema, common) {
                shape
                    .properties
                    .entry(std::ptr::from_ref(data))
                    .or_insert(kind);
            }
            inspect_nested_payload(schema, data, common, &path, &location, &mut candidates);
        }
        shape
    }

    fn schema_target(&self, reference: &str, common: bool) -> Option<&Value> {
        let pointer = percent_decode_str(reference.strip_prefix('#')?)
            .decode_utf8()
            .ok()?;
        let document = if common {
            &self.common_types
        } else {
            &self.document
        };
        document.pointer(&pointer)
    }
}

fn inspect_nested_payload<'a>(
    schema: &'a Value,
    data: &'a Value,
    common: bool,
    path: &str,
    location: &Location,
    candidates: &mut Vec<PayloadCandidate<'a>>,
) {
    if let Some(properties) = schema["properties"].as_object() {
        for (key, property) in properties.iter().rev() {
            if let Some(value) = data.get(key) {
                let field = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                candidates.push((property, value, common, field, location.join(key.as_str())));
            }
        }
    }
    if let Some(array) = data.as_array() {
        let prefix = schema["prefixItems"]
            .as_array()
            .map_or(&[][..], Vec::as_slice);
        for (index, value) in array.iter().enumerate().rev() {
            let item = prefix.get(index).unwrap_or(&schema["items"]);
            candidates.push((
                item,
                value,
                common,
                format!("{path}[{index}]"),
                location.join(index),
            ));
        }
    }
    for key in ["allOf", "anyOf", "oneOf"] {
        if let Some(branches) = schema[key].as_array() {
            candidates.extend(
                branches
                    .iter()
                    .map(|branch| (branch, data, common, path.to_owned(), location.clone())),
            );
        }
    }
}

fn inline_dynamic_kind(schema: &Value, common: bool) -> Option<PropertyKind> {
    let branches = schema["oneOf"]
        .as_array()
        .or_else(|| schema["anyOf"].as_array())?;
    let mut kind = None;
    let mut dynamic = false;
    for branch in branches {
        if let Some(reference) = branch["$ref"].as_str() {
            let (pointer, common) =
                common_ref(reference).map_or((reference, common), |pointer| (pointer, true));
            if common && matches!(pointer, "#/$defs/DataBinding" | "#/$defs/FunctionCall") {
                dynamic = true;
                continue;
            }
        }
        if kind.is_some() {
            return None;
        }
        kind = Some(match branch["type"].as_str()? {
            "boolean" => PropertyKind::Boolean,
            "number" => PropertyKind::Number,
            "string" => PropertyKind::String,
            _ => return None,
        });
    }
    if dynamic { kind } else { None }
}

/// A catalog or catalog payload error.
#[derive(Debug, Error)]
pub enum CatalogError {
    /// The catalog document or schema is invalid.
    #[error("{0}")]
    InvalidCatalog(String),
    /// The metadata in a component definition is invalid.
    #[error("Invalid metadata for component '{name}': {source}")]
    InvalidComponentMetadata {
        /// The component name.
        name: String,
        /// The metadata decode error.
        #[source]
        source: serde_json::Error,
    },
    /// The catalog has no entry with this name.
    #[error("Catalog does not define '{0}'")]
    UnknownEntry(String),
    /// The payload does not match its JSON schema.
    #[error("Invalid payload for '{name}': {message}")]
    InvalidPayload {
        /// The component or function name.
        name: String,
        /// The schema validation error.
        message: String,
        /// The field location in the incoming component batch, when available.
        path: Option<String>,
    },
}

fn invalid(message: impl Into<String>) -> CatalogError {
    CatalogError::InvalidCatalog(message.into())
}

fn entries<'a>(document: &'a Value, key: &str) -> Result<&'a Map<String, Value>, CatalogError> {
    if let Some(entries) = document.get(key) {
        entries
            .as_object()
            .ok_or_else(|| invalid(format!("{key} must be an object")))
    } else {
        static EMPTY: std::sync::LazyLock<Map<String, Value>> = std::sync::LazyLock::new(Map::new);
        Ok(&EMPTY)
    }
}

fn identifier(name: &str, kind: &str) -> Result<(), CatalogError> {
    let mut chars = name.chars();
    if chars
        .next()
        .is_some_and(|ch| ch == '_' || unicode_ident::is_xid_start(ch))
        && chars.all(unicode_ident::is_xid_continue)
    {
        Ok(())
    } else {
        Err(invalid(format!(
            "Invalid UAX #31 {kind} identifier: {name}"
        )))
    }
}

fn validate_entries(entries: &Map<String, Value>, kind: &str) -> Result<(), CatalogError> {
    for (name, schema) in entries {
        identifier(name, kind)?;
        if kind == "function" {
            validate_function_metadata(name, schema)?;
        }
        if let Some(parameters) = schema["parameters"].as_object() {
            for name in parameters.keys() {
                identifier(name, "argument")?;
            }
        }
        if kind == "component"
            && let Some(metadata) = schema.get("metadata")
        {
            serde_json::from_value::<Metadata>(metadata.clone()).map_err(|source| {
                CatalogError::InvalidComponentMetadata {
                    name: name.clone(),
                    source,
                }
            })?;
        }
        for key in ["allowedParents", "allowedChildren"] {
            if let Some(value) = schema.get(key)
                && !value
                    .as_array()
                    .is_some_and(|names| names.iter().all(Value::is_string))
            {
                return Err(invalid(format!(
                    "{name}.{key} must be an array of component names"
                )));
            }
        }
        for part in schema_nodes(schema) {
            if let Some(properties) = part["properties"].as_object() {
                for name in properties.keys().filter(|name| {
                    !matches!(
                        name.as_str(),
                        "@call" | "@catalogId" | "@metadata" | "@accessibility"
                    )
                }) {
                    identifier(
                        name,
                        if kind == "function" {
                            "argument"
                        } else {
                            "property"
                        },
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn validate_function_metadata(name: &str, schema: &Value) -> Result<(), CatalogError> {
    let callers = schema.get("allowedCallers");
    if callers.is_some_and(|value| {
        !matches!(
            value.as_str(),
            Some("rendererOnly" | "agentOnly" | "rendererOrAgent")
        )
    }) {
        return Err(invalid(format!("Invalid allowedCallers for '{name}'")));
    }
    if let Some(activation) = schema.get("requiresUserActivation")
        && (!activation.is_boolean()
            || (activation == true && callers.is_some_and(|value| value != "rendererOnly")))
    {
        return Err(invalid(format!(
            "Invalid requiresUserActivation for '{name}'"
        )));
    }
    Ok(())
}

fn schema_nodes(root: &Value) -> impl Iterator<Item = &Value> {
    let mut nodes = vec![root];
    std::iter::from_fn(move || {
        let node = nodes.pop()?;
        for key in [
            "$defs",
            "properties",
            "patternProperties",
            "dependentSchemas",
        ] {
            if let Some(children) = node[key].as_object() {
                nodes.extend(children.values());
            }
        }
        for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
            if let Some(children) = node[key].as_array() {
                nodes.extend(children);
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
            nodes.extend(node.get(key));
        }
        Some(node)
    })
}

fn common_ref(reference: &str) -> Option<&str> {
    let (base, fragment) = reference.split_once('#')?;
    if matches!(base, "common_types.json" | COMMON) && fragment.starts_with("/$defs/") {
        Some(&reference[base.len()..])
    } else {
        None
    }
}

fn check_schema_refs(entries: &Map<String, Value>) -> Result<(), CatalogError> {
    for entry in entries.values() {
        for schema in schema_nodes(entry) {
            for keyword in ["$ref", "$dynamicRef"] {
                if let Some(reference) = schema[keyword].as_str()
                    && !reference.starts_with('#')
                    && common_ref(reference).is_none()
                {
                    return Err(invalid(format!(
                        "Unsupported external {keyword}: {reference}"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn compile_entries(
    registry: &Registry<'_>,
    kind: &str,
    common: &str,
    names: Vec<String>,
) -> Result<BTreeMap<String, Validator>, CatalogError> {
    let options = jsonschema::draft202012::options()
        .with_registry(registry)
        .offline();
    names
        .into_iter()
        .map(|name| {
            let encoded = utf8_percent_encode(&name, NON_ALPHANUMERIC);
            let entry = format!("{CATALOG}#/{kind}/{encoded}");
            let envelope = format!("{COMMON}#/$defs/{common}");
            let tag = if kind == "components" {
                "component"
            } else {
                "@call"
            };
            let discriminator = json!({"properties": {(tag): {"const": name}}, "required": [tag]});
            let wrapper = json!({
                "allOf": [{"$ref": envelope}, {"$ref": entry}, discriminator],
                "unevaluatedProperties": false
            });
            let validator = options
                .build(&wrapper)
                .map_err(|error| invalid(format!("Invalid schema for {name}: {error}")))?;
            Ok((name, validator))
        })
        .collect()
}
