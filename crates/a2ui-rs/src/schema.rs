//! Builds the published JSON Schemas for the A2UI protocol.

use serde_json::{Map, Value, json};

use crate::ProtocolVersion;

mod agent;
mod common;

pub use agent::agent_to_renderer_schema;
pub use common::common_types_schema;

const BASE: &str = "https://a2ui.org/specification/v1_0/";

fn document(file: &str, title: &str, description: &str, definitions: Map<String, Value>) -> Value {
    let mut schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": format!("{BASE}{file}"),
        "title": title,
        "description": description,
    });
    schema["$defs"] = Value::Object(definitions);
    schema
}

fn kind(name: &str) -> Value {
    json!({"type": name})
}

fn reference(path: &str) -> Value {
    json!({"$ref": path})
}

fn local(name: &str) -> Value {
    reference(&format!("#/$defs/{name}"))
}

fn external(name: &str) -> Value {
    reference(&format!("common_types.json#/$defs/{name}"))
}

fn described(mut schema: Value, description: &str) -> Value {
    schema["description"] = json!(description);
    schema
}

fn object(properties: impl IntoIterator<Item = (&'static str, Value)>, required: &[&str]) -> Value {
    let properties: Map<_, _> = properties
        .into_iter()
        .map(|(name, schema)| (name.to_owned(), schema))
        .collect();
    let mut schema =
        json!({"type": "object", "properties": properties, "additionalProperties": false});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

fn version() -> Value {
    json!({"const": ProtocolVersion::V1_0.to_string()})
}
