//! A2UI v1.0 messages and catalog-independent payload types.
//!
//! Catalog-defined component properties and function arguments remain JSON data.
//! Use the corresponding SDK layers for catalog validation and state processing.

use std::collections::BTreeMap;

use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use thiserror::Error;

/// An arbitrary JSON object, used where the protocol permits application data.
pub type JsonObject = Map<String, Value>;

/// An agent-to-renderer envelope with an explicit protocol version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "version")]
pub enum AgentToRendererMessage {
    /// An A2UI v1.0 operation.
    #[serde(rename = "1.0")]
    V1(AgentToRenderer),
}

/// One A2UI v1.0 agent-to-renderer operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentToRenderer {
    /// Creates a surface with optional initial state.
    CreateSurface(CreateSurface),
    /// Adds or replaces component definitions.
    UpdateComponents(UpdateComponents),
    /// Replaces data or deletes it with an explicit null value.
    UpdateDataModel(UpdateDataModel),
    /// Removes a surface.
    DeleteSurface(DeleteSurface),
    /// Requests execution of a renderer function.
    CallRendererFunction(CallRendererFunction),
    /// Returns an agent function outcome.
    AgentFunctionResponse(FunctionResponse),
}

/// A renderer-to-agent envelope with an explicit protocol version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "version")]
pub enum RendererToAgentMessage {
    /// An A2UI v1.0 event or function message.
    #[serde(rename = "1.0")]
    V1(RendererToAgent),
}

/// One A2UI v1.0 renderer-to-agent operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RendererToAgent {
    /// Reports a user action.
    Action(Action),
    /// Requests execution of an agent function.
    CallAgentFunction(CallAgentFunction),
    /// Returns a renderer function outcome.
    RendererFunctionResponse(FunctionResponse),
    /// Reports a renderer error.
    Error(RendererError),
}

/// Creates a surface. An absent catalog ID leaves the default catalog unset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSurface {
    /// The surface identifier.
    pub surface_id: String,
    /// The optional default catalog identifier.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub catalog_id: Option<String>,
    /// Whether to include the data model in outgoing transport metadata.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub send_data_model: Option<bool>,
    /// Optional initial component definitions.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub components: Option<Components>,
    /// The optional initial root data object.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub data_model: Option<JsonObject>,
    /// Optional surface metadata.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub metadata: Option<Metadata>,
}

/// Adds or replaces a nonempty set of components.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateComponents {
    /// The destination surface.
    pub surface_id: String,
    /// Component definitions to add or replace.
    pub components: Components,
}

/// Updates a data model. The value is required, including for explicit null deletion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDataModel {
    /// The destination surface.
    pub surface_id: String,
    /// The optional JSON Pointer. An absent path selects the root.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub path: Option<String>,
    /// The replacement value. Null deletes the selected key.
    pub value: Value,
}

/// Deletes a surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteSurface {
    /// The destination surface.
    pub surface_id: String,
}

/// A component envelope with open catalog-defined properties.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Component {
    /// The component identifier.
    pub id: String,
    /// The catalog component name. Catalog validation is a separate operation.
    #[serde(
        deserialize_with = "component_name",
        serialize_with = "serialize_component_name"
    )]
    pub component: String,
    /// An optional catalog override.
    #[serde(
        rename = "@catalogId",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub catalog_id: Option<String>,
    /// Optional accessibility attributes.
    #[serde(
        rename = "@accessibility",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub accessibility: Option<Accessibility>,
    /// Optional component metadata.
    #[serde(
        rename = "@metadata",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub metadata: Option<Metadata>,
    /// Properties defined by the resolved component catalog.
    #[serde(flatten, serialize_with = "serialize_component_properties")]
    pub properties: BTreeMap<String, Value>,
}

/// A nonempty list of component definitions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<Component>")]
pub struct Components(Vec<Component>);

impl Components {
    /// Returns the component definitions in message order.
    #[must_use]
    pub fn as_slice(&self) -> &[Component] {
        &self.0
    }
}

impl TryFrom<Vec<Component>> for Components {
    type Error = ProtocolError;
    fn try_from(components: Vec<Component>) -> Result<Self, Self::Error> {
        if components.is_empty() {
            return Err(ProtocolError::EmptyComponents);
        }
        Ok(Self(components))
    }
}

/// Optional metadata with Unicode identifier extension keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    /// Extension values keyed by a UAX #31 identifier.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub extensions: Option<Extensions>,
}

/// Extension values whose keys satisfy the protocol's XID identifier rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "JsonObject")]
pub struct Extensions(JsonObject);

impl Extensions {
    /// Returns the extension values.
    #[must_use]
    pub fn as_object(&self) -> &JsonObject {
        &self.0
    }
}

impl TryFrom<JsonObject> for Extensions {
    type Error = ProtocolError;
    fn try_from(values: JsonObject) -> Result<Self, Self::Error> {
        for key in values.keys() {
            let mut characters = key.chars();
            if !characters
                .next()
                .is_some_and(|character| character == '_' || unicode_ident::is_xid_start(character))
                || !characters.all(unicode_ident::is_xid_continue)
            {
                return Err(ProtocolError::InvalidExtensionKey(key.clone()));
            }
        }
        Ok(Self(values))
    }
}

/// Accessibility attributes before expression resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accessibility {
    /// A short accessible label.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub label: Option<Dynamic<String>>,
    /// An accessible description.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub description: Option<Dynamic<String>>,
    /// The announcement policy.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub live: Option<Live>,
    /// Whether to hide the component from accessibility APIs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub hidden: Option<Dynamic<bool>>,
}

/// A live-region announcement policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Live {
    /// Do not announce changes.
    Off,
    /// Announce when the user is idle.
    Polite,
    /// Announce immediately.
    Assertive,
}

/// A literal or unresolved binding/function expression.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dynamic<T> {
    /// A literal value.
    Literal(T),
    /// A data model binding.
    Binding(DataBinding),
    /// A catalog or system function expression.
    Function(FunctionCall),
}

/// A binding expression. Resolution is outside the protocol layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataBinding {
    /// The data model pointer.
    #[serde(rename = "@path")]
    pub path: String,
}

/// A function expression with catalog-defined arguments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FunctionCall {
    /// The function name.
    #[serde(rename = "@call")]
    pub name: String,
    /// An optional catalog override.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub catalog_id: Option<String>,
    /// Catalog-defined arguments. Their schema is not validated here.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub args: Option<JsonObject>,
}

/// A renderer RPC expression with the required explicit catalog ID.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RendererFunctionCall {
    /// The function name.
    #[serde(rename = "@call")]
    pub name: String,
    /// The function's catalog identifier.
    pub catalog_id: String,
    /// Catalog-defined arguments.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub args: Option<JsonObject>,
}

/// Requests local renderer function execution without executing it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallRendererFunction {
    /// The optional surface evaluation context.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub surface_id: Option<String>,
    /// The invocation correlation identifier.
    pub function_call_id: String,
    /// The renderer function expression.
    pub call_function: RendererFunctionCall,
}

/// Requests agent function execution without selecting a transport or runtime.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallAgentFunction {
    /// The originating surface.
    pub surface_id: String,
    /// The invocation correlation identifier.
    pub function_call_id: String,
    /// The function expression.
    pub call_function: FunctionCall,
}

/// Exactly one function outcome. Explicit null is a successful value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum FunctionResponse {
    /// A successful return value.
    Value {
        /// The invocation correlation identifier.
        function_call_id: String,
        /// The return value, including explicit null.
        value: Value,
    },
    /// A failed function invocation.
    Error {
        /// The invocation correlation identifier.
        function_call_id: String,
        /// The structured failure.
        error: FunctionError,
    },
}

/// A function failure payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionError {
    /// The machine-readable error code.
    pub code: String,
    /// The error description.
    pub message: String,
}

/// A user action after bindings have been resolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    /// The action name.
    pub name: String,
    /// The originating surface.
    pub surface_id: String,
    /// The originating component.
    pub source_component_id: String,
    /// The timestamp string. Date-time format validation is not performed here.
    pub timestamp: String,
    /// Resolved application context.
    pub context: JsonObject,
    /// Optional user-facing description.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub user_message: Option<String>,
    /// Optional renderer metadata.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional"
    )]
    pub metadata: Option<Metadata>,
    /// Additional action properties permitted by the schema.
    #[serde(flatten, serialize_with = "serialize_action_properties")]
    pub extra: BTreeMap<String, Value>,
}

/// A validation error code with required surface and field location.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ValidationCode {
    /// A value failed validation.
    ValidationFailed,
    /// The parent is not permitted.
    UnallowedParent,
    /// The child is not permitted.
    UnallowedChild,
}

/// A field validation error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationError {
    /// The validation code.
    pub code: ValidationCode,
    /// The affected surface.
    pub surface_id: String,
    /// The field's JSON Pointer.
    pub path: String,
    /// The validation description.
    pub message: String,
}

/// Exactly one generic error correlation target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorTarget {
    /// An error associated with a surface.
    SurfaceId(String),
    /// An error associated with a function invocation.
    FunctionCallId(String),
}

/// A generic renderer error with one correlation target.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "RawGenericError")]
pub struct GenericError {
    /// A code other than the three reserved validation codes.
    pub code: String,
    /// The error description.
    pub message: String,
    /// Exactly one surface or function target.
    pub target: ErrorTarget,
    /// Additional schema-permitted error properties.
    pub extra: BTreeMap<String, Value>,
}

/// A renderer-side validation or generic error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RendererError {
    /// A validation failure with a field path.
    Validation(ValidationError),
    /// A generic error with one correlation target.
    Generic(GenericError),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGenericError {
    code: String,
    message: String,
    #[serde(default, deserialize_with = "optional")]
    surface_id: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    function_call_id: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl TryFrom<RawGenericError> for GenericError {
    type Error = &'static str;
    fn try_from(raw: RawGenericError) -> Result<Self, Self::Error> {
        if is_validation_code(&raw.code) {
            return Err("validation codes require a validation error payload");
        }
        let target = match (raw.surface_id, raw.function_call_id) {
            (Some(surface), None) => ErrorTarget::SurfaceId(surface),
            (None, Some(call)) => ErrorTarget::FunctionCallId(call),
            _ => return Err("generic error requires exactly one correlation target"),
        };
        Ok(Self {
            code: raw.code,
            message: raw.message,
            target,
            extra: raw.extra,
        })
    }
}

#[derive(Serialize)]
struct GenericErrorFields<'a> {
    code: &'a str,
    message: &'a str,
    #[serde(flatten)]
    target: &'a ErrorTarget,
    #[serde(flatten)]
    extra: &'a BTreeMap<String, Value>,
}

impl Serialize for GenericError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if is_validation_code(&self.code)
            || self.extra.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "code" | "message" | "surfaceId" | "functionCallId"
                )
            })
        {
            return Err(S::Error::custom("generic error contains reserved fields"));
        }
        GenericErrorFields {
            code: &self.code,
            message: &self.message,
            target: &self.target,
            extra: &self.extra,
        }
        .serialize(serializer)
    }
}

fn component_name<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let name = String::deserialize(deserializer)?;
    if name == "Surface" {
        return Err(D::Error::custom("Surface is an implicit container"));
    }
    Ok(name)
}

fn serialize_component_name<S: Serializer>(name: &str, serializer: S) -> Result<S::Ok, S::Error> {
    if name == "Surface" {
        return Err(S::Error::custom("Surface is an implicit container"));
    }
    name.serialize(serializer)
}

fn serialize_component_properties<S: Serializer>(
    properties: &BTreeMap<String, Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serialize_properties(
        properties,
        serializer,
        &[
            "id",
            "component",
            "@catalogId",
            "@accessibility",
            "@metadata",
        ],
    )
}

fn serialize_action_properties<S: Serializer>(
    properties: &BTreeMap<String, Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serialize_properties(
        properties,
        serializer,
        &[
            "name",
            "surfaceId",
            "sourceComponentId",
            "timestamp",
            "context",
            "userMessage",
            "metadata",
        ],
    )
}

fn serialize_properties<S: Serializer>(
    properties: &BTreeMap<String, Value>,
    serializer: S,
    reserved: &[&str],
) -> Result<S::Ok, S::Error> {
    if properties
        .keys()
        .any(|key| reserved.contains(&key.as_str()))
    {
        return Err(S::Error::custom(
            "additional properties cannot overwrite envelope fields",
        ));
    }
    properties.serialize(serializer)
}

fn is_validation_code(code: &str) -> bool {
    matches!(
        code,
        "VALIDATION_FAILED" | "UNALLOWED_PARENT" | "UNALLOWED_CHILD"
    )
}

fn optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// A catalog-independent JSON value failed validation.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    /// A component list was empty.
    #[error("component lists must not be empty")]
    EmptyComponents,
    /// A metadata extension key was not a Unicode identifier.
    #[error("invalid extension identifier: {0}")]
    InvalidExtensionKey(String),
}

/// A typed `messages` list wrapper. Lists can be empty.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageBatch<T> {
    /// Messages in transport order.
    pub messages: Vec<T>,
}
