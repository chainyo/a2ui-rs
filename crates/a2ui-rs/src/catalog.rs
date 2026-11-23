//! Offline A2UI v1.0 catalog validation.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;

use boon::{Compiler, Draft, SchemaIndex, Schemas, UrlLoader};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::protocol::{Component, FunctionCall, Metadata};

const CATALOG: &str = "https://a2ui.org/specification/v1_0/catalog.json";
const COMMON: &str = "https://a2ui.org/specification/v1_0/common_types.json";

pub(crate) struct ChildReference<'a> {
    pub(crate) id: &'a str,
    pub(crate) field: String,
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

/// An immutable catalog with compiled component and function schemas.
pub struct Catalog {
    id: String,
    document: Value,
    common_types: Value,
    schemas: Schemas,
    components: BTreeMap<String, SchemaIndex>,
    functions: BTreeMap<String, SchemaIndex>,
}

impl Catalog {
    /// Loads a v1.0 catalog and the v1.0 common types schema supplied by the caller.
    ///
    /// Schema resolution uses only the supplied documents and built-in JSON Schema resources.
    ///
    /// # Errors
    /// Returns an error if the document has an invalid ID, version, name, or schema.
    pub fn from_json(document: Value, common_types: &Value) -> Result<Self, CatalogError> {
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

        let mut local = document.clone();
        let local_object = local
            .as_object_mut()
            .ok_or_else(|| invalid("Expected a catalog object"))?;
        local_object.remove("$id");
        let component_entries = entries(&local, "components")?;
        let function_entries = entries(&local, "functions")?;
        validate_entries(component_entries, "component")?;
        validate_entries(function_entries, "function")?;
        let components = component_entries.keys().cloned().collect::<Vec<_>>();
        let functions = function_entries.keys().cloned().collect::<Vec<_>>();
        check_schema_refs(entries(&local, "$defs")?)?;
        check_schema_refs(component_entries)?;
        check_schema_refs(function_entries)?;

        let mut common_types = common_types.clone();
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

        let mut compiler = Compiler::new();
        compiler.set_default_draft(Draft::V2020_12);
        compiler.use_loader(Box::new(Offline));
        compiler
            .add_resource(CATALOG, local)
            .map_err(|error| compile_error(&error))?;
        compiler
            .add_resource(COMMON, common_types.clone())
            .map_err(|error| compile_error(&error))?;
        let mut schemas = Schemas::new();
        let components = compile_entries(
            &mut compiler,
            &mut schemas,
            "components",
            "ComponentCommon",
            components,
        )?;
        let functions = compile_entries(
            &mut compiler,
            &mut schemas,
            "functions",
            "FunctionCommon",
            functions,
        )?;
        Ok(Self {
            id,
            document,
            common_types,
            schemas,
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
    #[must_use]
    pub fn as_json(&self) -> &Value {
        &self.document
    }

    /// Checks a component against this catalog.
    ///
    /// # Errors
    /// Returns an error if the component type is unknown or a property is invalid.
    pub fn validate_component(&self, component: &Component) -> Result<(), CatalogError> {
        self.validate_entry(&self.components, &component.component, component)
    }

    /// Checks a function call against this catalog.
    ///
    /// # Errors
    /// Returns an error if the function name is unknown or an argument is invalid.
    pub fn validate_function(&self, call: &FunctionCall) -> Result<(), CatalogError> {
        self.validate_entry(&self.functions, &call.name, call)
    }

    fn validate_entry(
        &self,
        entries: &BTreeMap<String, SchemaIndex>,
        name: &str,
        instance: &impl serde::Serialize,
    ) -> Result<(), CatalogError> {
        let index = entries
            .get(name)
            .ok_or_else(|| CatalogError::UnknownEntry(name.to_owned()))?;
        let instance =
            serde_json::to_value(instance).map_err(|error| invalid(error.to_string()))?;
        self.schemas
            .validate(&instance, *index)
            .map_err(|error| CatalogError::InvalidPayload {
                name: name.to_owned(),
                message: format!("{error:#}"),
            })
    }

    pub(crate) fn component_schema(&self, name: &str) -> &Value {
        &self.document["components"][name]
    }

    pub(crate) fn function_schema(&self, name: &str) -> &Value {
        &self.document["functions"][name]
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
    ) -> BTreeMap<&'a str, String> {
        let mut references = BTreeMap::new();
        for child in self.inspect_payload(schema, payload).children {
            references.entry(child.id).or_insert(child.field);
        }
        references
    }

    pub(crate) fn inspect_payload<'a>(
        &'a self,
        schema: &'a Value,
        payload: &'a Value,
    ) -> PayloadShape<'a> {
        let mut shape = PayloadShape::default();
        let mut candidates = vec![(schema, payload, false, String::new())];
        let mut visited = BTreeSet::new();
        let mut fields = BTreeSet::new();
        while let Some((schema, data, common, path)) = candidates.pop() {
            if !visited.insert((std::ptr::from_ref(schema), std::ptr::from_ref(data))) {
                continue;
            }
            if let Some(reference) = schema["$ref"].as_str() {
                let (pointer, common) =
                    common_ref(reference).map_or((reference, common), |pointer| (pointer, true));
                let kind = match (common, pointer) {
                    (true, "#/$defs/Action") => Some(PropertyKind::Action),
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
                            collection,
                        });
                    }
                    continue;
                }
                if let Some(target) = self.schema_target(pointer, common) {
                    candidates.push((target, data, common, path.clone()));
                }
            }
            if let Some(properties) = schema["properties"].as_object() {
                for (key, property) in properties.iter().rev() {
                    if let Some(value) = data.get(key) {
                        let field = if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        };
                        candidates.push((property, value, common, field));
                    }
                }
            }
            if let Some(array) = data.as_array() {
                let prefix = schema["prefixItems"]
                    .as_array()
                    .map_or(&[][..], Vec::as_slice);
                for (index, value) in array.iter().enumerate().rev() {
                    let item = prefix.get(index).unwrap_or(&schema["items"]);
                    candidates.push((item, value, common, format!("{path}[{index}]")));
                }
            }
            for key in ["allOf", "anyOf", "oneOf"] {
                if let Some(branches) = schema[key].as_array() {
                    candidates.extend(
                        branches
                            .iter()
                            .map(|branch| (branch, data, common, path.clone())),
                    );
                }
            }
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

/// A catalog or catalog payload error.
#[derive(Debug, Error)]
pub enum CatalogError {
    /// The catalog document or schema is invalid.
    #[error("{0}")]
    InvalidCatalog(String),
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
    },
}

fn invalid(message: impl Into<String>) -> CatalogError {
    CatalogError::InvalidCatalog(message.into())
}

fn compile_error(error: &boon::CompileError) -> CatalogError {
    invalid(format!("{error:#}"))
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
            serde_json::from_value::<Metadata>(metadata.clone())
                .map_err(|error| invalid(error.to_string()))?;
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
    compiler: &mut Compiler,
    schemas: &mut Schemas,
    kind: &str,
    common: &str,
    names: Vec<String>,
) -> Result<BTreeMap<String, SchemaIndex>, CatalogError> {
    names
        .into_iter()
        .map(|name| {
            let encoded = utf8_percent_encode(&name, NON_ALPHANUMERIC);
            let entry = format!("{CATALOG}#/{kind}/{encoded}");
            let envelope = format!("{COMMON}#/$defs/{common}");
            let wrapper = format!("https://a2ui.org/specification/v1_0/validate/{kind}/{encoded}");
            let tag = if kind == "components" {
                "component"
            } else {
                "@call"
            };
            let discriminator = json!({"properties": {(tag): {"const": name}}, "required": [tag]});
            compiler
                .add_resource(
                    &wrapper,
                    json!({
                        "allOf": [{"$ref": envelope}, {"$ref": entry}, discriminator],
                        "unevaluatedProperties": false
                    }),
                )
                .map_err(|error| compile_error(&error))?;
            Ok((
                name,
                compiler
                    .compile(&wrapper, schemas)
                    .map_err(|error| compile_error(&error))?,
            ))
        })
        .collect()
}

struct Offline;

impl UrlLoader for Offline {
    fn load(&self, url: &str) -> Result<Value, Box<dyn Error>> {
        Err(format!("Schema resource is unavailable: {url}").into())
    }
}
