//! Tests for the public data model API.

use std::error::Error;

use a2ui_rs::data_model::{DataError, DataModel};
use serde_json::{Value, json};

#[test]
fn reads_escaped_paths_and_preserves_array_positions() -> Result<(), DataError> {
    let mut model = DataModel::default();
    model.set("/a~1b/2/c~0d", json!("value"))?;
    assert_eq!(model.get("/a~1b/2/c~0d")?, Some(&json!("value")));
    assert_eq!(model.get("/a~1b/02")?, None);
    assert_eq!(model.get("/a~1b/0")?, Some(&Value::Null));
    model.set("/a~1b/2", Value::Null)?;
    assert_eq!(model.root(), &json!({"a/b":[null,null,null]}));
    Ok(())
}

#[test]
fn replaces_and_resets_the_root() -> Result<(), Box<dyn Error>> {
    let mut model = DataModel::default();
    model.set("", json!(false))?;
    assert_eq!(model.get("/")?, Some(&json!(false)));
    assert!(matches!(
        model.set("/field", json!(1)),
        Err(DataError::ScalarParent(_))
    ));
    assert_eq!(model.root(), &json!(false));
    model.set("/", Value::Null)?;
    assert_eq!(model.root(), &json!({}));
    Ok(())
}

#[test]
fn rejects_invalid_paths_without_partial_updates() -> Result<(), DataError> {
    let mut model = DataModel::default();
    model.set("/profile", json!({"name":"Ada","details":null}))?;
    let expected = model.clone();
    for path in [
        "/new/~2invalid",
        "/new/constructor/x",
        "/new/10001/x",
        "/profile/details/nested/10001/x",
        "relative",
    ] {
        assert!(model.set(path, json!(true)).is_err(), "{path}");
        assert_eq!(model, expected);
    }
    assert!(matches!(
        model.get("/missing/prototype"),
        Err(DataError::ForbiddenSegment(_))
    ));
    model.set("/missing/0/name", Value::Null)?;
    assert_eq!(model, expected);
    model.set("/profile/details/city", json!("London"))?;
    assert_eq!(model.get("/profile/details/city")?, Some(&json!("London")));
    Ok(())
}
