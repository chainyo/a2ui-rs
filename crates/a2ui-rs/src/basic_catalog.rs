//! Local functions for the A2UI v1.0 Basic Catalog.

use serde_json::{Value, json};

use crate::protocol::JsonObject;
use crate::resolution::{LocalFunctions, to_boolean};

/// Registers the Basic Catalog `required`, `email`, `and`, `or`, and `not` functions.
///
/// Supply the ID of the loaded v1.0 Basic Catalog.
pub fn register_validation_functions(functions: &mut LocalFunctions<'_>, catalog_id: &str) {
    functions.insert(catalog_id, "required", |args| {
        let valid = match args.get("value") {
            None | Some(Value::Null) => false,
            Some(Value::String(value)) => !value.is_empty(),
            Some(Value::Array(items)) => !items.is_empty(),
            Some(_) => true,
        };
        Ok(json!({"valid": valid}))
    });
    functions.insert(catalog_id, "email", |args| {
        let value = args["value"].as_str().ok_or("Expected an email string")?;
        Ok(if is_email(value) {
            json!({"valid": true})
        } else {
            json!({
                "valid": false,
                "code": "INVALID_EMAIL",
                "severity": "error",
                "message": "Please enter a valid email address (e.g. user@example.com).",
            })
        })
    });
    functions.insert(catalog_id, "and", |args| {
        Ok(Value::Bool(boolean_values(args)?.iter().all(to_boolean)))
    });
    functions.insert(catalog_id, "or", |args| {
        Ok(Value::Bool(boolean_values(args)?.iter().any(to_boolean)))
    });
    functions.insert(catalog_id, "not", |args| {
        let value = args.get("value").ok_or("Expected a boolean value")?;
        Ok(Value::Bool(!to_boolean(value)))
    });
}

fn boolean_values(args: &JsonObject) -> Result<&[Value], String> {
    args["values"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "Expected a list of boolean values".to_owned())
}

fn is_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    let Some((host, suffix)) = domain.rsplit_once('.') else {
        return false;
    };
    !local.is_empty()
        && !host.is_empty()
        && suffix.len() >= 2
        && local
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._%+-".contains(&byte))
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
        && suffix.bytes().all(|byte| byte.is_ascii_alphabetic())
}
