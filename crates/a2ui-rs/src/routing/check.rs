use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::EvaluationState;
use crate::protocol::JsonObject;
use crate::resolution::to_boolean;

/// The severity of a validation result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationSeverity {
    /// An error.
    Error,
    /// A warning.
    Warning,
    /// An informational result.
    Info,
}

/// The result of a mounted check rule.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ValidationResult {
    /// Whether the check passes.
    pub valid: bool,
    /// The optional machine-readable code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The optional message for the renderer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The optional severity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<ValidationSeverity>,
    #[serde(flatten)]
    extra: JsonObject,
}

impl ValidationResult {
    /// Returns true if the check fails with error severity.
    #[must_use]
    pub fn is_blocking(&self) -> bool {
        !self.valid
            && !matches!(
                self.severity,
                Some(ValidationSeverity::Warning | ValidationSeverity::Info)
            )
    }

    /// Returns additional fields from a structured result.
    #[must_use]
    pub fn extra(&self) -> &JsonObject {
        &self.extra
    }
    pub(super) fn from_check(state: &EvaluationState, message: Option<String>) -> Self {
        let (mut result, fallback) = match state {
            EvaluationState::Ready(value) => (Self::from_value(value), message),
            EvaluationState::Failed(error) => (
                Self::new(false),
                message.or_else(|| Some(error.message.clone())),
            ),
            EvaluationState::Pending | EvaluationState::Cancelled => (Self::new(true), None),
        };
        if !result.valid {
            result.message = result.message.or(fallback);
            result.severity = result.severity.or(Some(ValidationSeverity::Error));
        }
        result
    }

    fn new(valid: bool) -> Self {
        Self {
            valid,
            code: None,
            message: None,
            severity: None,
            extra: JsonObject::new(),
        }
    }

    fn from_value(value: &Value) -> Self {
        if let Some(valid) = value.get("valid").and_then(Value::as_bool) {
            let mut result = Self::new(valid);
            result.code = value["code"].as_str().map(str::to_owned);
            result.message = value["message"].as_str().map(str::to_owned);
            result.severity = match value["severity"].as_str() {
                Some("error") => Some(ValidationSeverity::Error),
                Some("warning") => Some(ValidationSeverity::Warning),
                Some("info") => Some(ValidationSeverity::Info),
                _ => None,
            };
            if let Some(object) = value.as_object() {
                result.extra.extend(
                    object
                        .iter()
                        .filter(|(key, _)| {
                            !matches!(key.as_str(), "valid" | "code" | "message" | "severity")
                        })
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
            result
        } else {
            Self::new(to_boolean(value))
        }
    }
}

#[derive(Deserialize)]
pub(super) struct CheckRule {
    pub(super) condition: Value,
    pub(super) message: Option<String>,
}
