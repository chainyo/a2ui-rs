//! Data model storage and A2UI JSON Pointer operations.

use serde_json::{Map, Value};
use thiserror::Error;

const MAX_ARRAY_INDEX: usize = 10_000;

/// The JSON data for one surface.
#[derive(Clone, Debug, PartialEq)]
pub struct DataModel {
    root: Value,
}

impl Default for DataModel {
    fn default() -> Self {
        Self::new(Map::new())
    }
}

impl DataModel {
    /// Creates a data model from the initial JSON object.
    #[must_use]
    pub fn new(initial: Map<String, Value>) -> Self {
        Self {
            root: Value::Object(initial),
        }
    }

    /// Returns the complete JSON value.
    #[must_use]
    pub fn root(&self) -> &Value {
        &self.root
    }

    /// Reads a value at an absolute A2UI JSON Pointer.
    ///
    /// An empty path or a path with only slashes selects the root.
    /// A missing value or an invalid array index returns `None`.
    ///
    /// # Errors
    ///
    /// Returns an error for a relative path, an invalid escape, or a forbidden key.
    pub fn get(&self, path: &str) -> Result<Option<&Value>, DataError> {
        let segments = parse_pointer(path)?;
        let mut current = &self.root;
        for segment in &segments {
            let next = match current {
                Value::Object(object) => object.get(segment),
                Value::Array(array) => array_index(segment).and_then(|index| array.get(index)),
                _ => None,
            };
            let Some(value) = next else { return Ok(None) };
            current = value;
        }
        Ok(Some(current))
    }

    /// Replaces the value at an absolute A2UI JSON Pointer.
    ///
    /// A null value removes an object field, clears an array position, or resets the root.
    /// A non-null value creates missing containers as necessary.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid path or a write through an incompatible value.
    /// An error leaves the data model unchanged.
    pub fn set(&mut self, path: &str, value: Value) -> Result<(), DataError> {
        let segments = parse_pointer(path)?;
        if segments.is_empty() {
            self.root = if value.is_null() {
                Value::Object(Map::new())
            } else {
                value
            };
        } else if value.is_null() {
            delete(&mut self.root, &segments);
        } else {
            set_nested(&mut self.root, &segments, value, path)?;
        }
        Ok(())
    }
}

/// A data model operation failed.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DataError {
    /// The path is relative or contains an invalid escape.
    #[error("Invalid path syntax in JSON Pointer '{0}'")]
    InvalidPointer(String),
    /// The path contains a forbidden key.
    #[error("Forbidden path segment in '{0}'")]
    ForbiddenSegment(String),
    /// A path segment is not a valid array index.
    #[error("Cannot use non-numeric segment on an array in path '{0}'")]
    InvalidArrayIndex(String),
    /// The write would exceed the maximum array index.
    #[error("Cannot set path '{0}': array index exceeds maximum supported index (10000)")]
    ArrayIndexLimit(String),
    /// The path attempts to write through a scalar.
    #[error("Cannot set path '{0}': a parent value is a scalar")]
    ScalarParent(String),
}

pub(crate) fn parse_pointer(path: &str) -> Result<Vec<String>, DataError> {
    if !path.is_empty() && !path.starts_with('/') {
        return Err(DataError::InvalidPointer(path.to_owned()));
    }
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(|raw| {
            let mut segment = String::with_capacity(raw.len());
            let mut characters = raw.chars();
            while let Some(character) = characters.next() {
                segment.push(if character == '~' {
                    match characters.next() {
                        Some('0') => '~',
                        Some('1') => '/',
                        _ => return Err(DataError::InvalidPointer(path.to_owned())),
                    }
                } else {
                    character
                });
            }
            if matches!(segment.as_str(), "__proto__" | "constructor" | "prototype") {
                return Err(DataError::ForbiddenSegment(path.to_owned()));
            }
            Ok(segment)
        })
        .collect()
}

fn is_index(segment: &str) -> bool {
    segment == "0"
        || (!segment.is_empty()
            && !segment.starts_with('0')
            && segment.bytes().all(|byte| byte.is_ascii_digit()))
}

fn array_index(segment: &str) -> Option<usize> {
    is_index(segment).then(|| segment.parse().ok()).flatten()
}

fn write_index(segment: &str, path: &str) -> Result<usize, DataError> {
    if !is_index(segment) {
        return Err(DataError::InvalidArrayIndex(path.to_owned()));
    }
    array_index(segment)
        .filter(|index| *index <= MAX_ARRAY_INDEX)
        .ok_or_else(|| DataError::ArrayIndexLimit(path.to_owned()))
}

fn set_nested(
    mut current: &mut Value,
    mut segments: &[String],
    value: Value,
    path: &str,
) -> Result<(), DataError> {
    while let Some((segment, tail)) = segments.split_first() {
        match current {
            Value::Object(object) => {
                if tail.is_empty() || !object.contains_key(segment) {
                    object.insert(segment.clone(), build_subtree(tail, value, path)?);
                    return Ok(());
                }
                current = object.entry(segment.clone()).or_insert(Value::Null);
            }
            Value::Array(array) => {
                let index = write_index(segment, path)?;
                if index >= array.len() {
                    let replacement = build_subtree(tail, value, path)?;
                    array.resize(index, Value::Null);
                    array.push(replacement);
                    return Ok(());
                }
                current = &mut array[index];
            }
            Value::Null => {
                *current = build_subtree(segments, value, path)?;
                return Ok(());
            }
            _ => return Err(DataError::ScalarParent(path.to_owned())),
        }
        segments = tail;
    }
    *current = value;
    Ok(())
}

// Build the missing part before it becomes part of the data model.
fn build_subtree(segments: &[String], mut value: Value, path: &str) -> Result<Value, DataError> {
    for segment in segments.iter().rev() {
        value = if is_index(segment) {
            let index = write_index(segment, path)?;
            let mut array = vec![Value::Null; index];
            array.push(value);
            Value::Array(array)
        } else {
            Value::Object(Map::from_iter([(segment.clone(), value)]))
        };
    }
    Ok(value)
}

fn delete(mut current: &mut Value, segments: &[String]) {
    let Some((last, parents)) = segments.split_last() else {
        return;
    };
    for segment in parents {
        let next = match current {
            Value::Object(object) => object.get_mut(segment),
            Value::Array(array) => array_index(segment).and_then(|index| array.get_mut(index)),
            _ => None,
        };
        let Some(value) = next else { return };
        current = value;
    }
    match current {
        Value::Object(object) => {
            object.remove(last);
        }
        Value::Array(array) => {
            if let Some(value) = array_index(last).and_then(|index| array.get_mut(index)) {
                *value = Value::Null;
            }
        }
        _ => {}
    }
}
