use serde_json::{Map, Value, json};

use crate::protocol::{
    CallRendererFunction, Components, CreateSurface, DeleteSurface, IMPLICIT_SURFACE_COMPONENT,
    JsonObject, Metadata, RendererFunctionCall, UpdateComponents, UpdateDataModel,
};

use super::{described, document, external, kind, local, object, reference, version};

/// Builds the v1.0 agent-to-renderer JSON Schema.
///
/// Register the common types and the active catalog to resolve its references.
#[must_use]
pub fn agent_to_renderer_schema() -> Value {
    let operations = [
        ("CreateSurfaceMessage", "createSurface", create_surface()),
        (
            "UpdateComponentsMessage",
            "updateComponents",
            update_components(),
        ),
        (
            "UpdateDataModelMessage",
            "updateDataModel",
            update_data_model(),
        ),
        ("DeleteSurfaceMessage", "deleteSurface", delete_surface()),
        (
            "CallRendererFunctionMessage",
            "callRendererFunction",
            call_renderer_function(),
        ),
        (
            "AgentFunctionResponseMessage",
            "agentFunctionResponse",
            external("FunctionResponse"),
        ),
    ];
    let variants: Vec<_> = operations.iter().map(|(name, _, _)| local(name)).collect();
    let mut defs: Map<_, _> = operations
        .into_iter()
        .map(|(name, operation, payload)| {
            (
                name.to_owned(),
                object(
                    [("version", version()), (operation, payload)],
                    &[operation, "version"],
                ),
            )
        })
        .collect();
    let component = described(
        json!({"allOf": [
            external("ComponentCommon"),
            reference("catalog.json#/$defs/anyComponent"),
            {
                "type": "object",
                "properties": {"component": {"not": {"const": IMPLICIT_SURFACE_COMPONENT}}}
            }
        ], "unevaluatedProperties": false}),
        "A UI component for the surface, combining baseline envelope properties with the catalog component definition.",
    );
    defs.insert("Component".to_owned(), component);
    defs.insert(
        "ComponentsList".to_owned(),
        described(
            json!({"type": "array", "minItems": 1, "items": local("Component")}),
            "A list containing UI components for the surface.",
        ),
    );
    let mut schema = document(
        "agent_to_renderer.json",
        "A2UI Message Schema",
        "Describes a JSON payload for an A2UI message, which is used to dynamically construct and update user interfaces.",
        defs,
    );
    schema["type"] = json!("object");
    schema["oneOf"] = json!(variants);
    schema
}

fn create_surface() -> Value {
    model([
        field("surfaceId", |p: &CreateSurface| &p.surface_id).describe(
            "The unique identifier for the UI surface to be rendered. It must be globally unique for the renderer's lifetime.",
        ),
        field("catalogId", |p: &CreateSurface| &p.catalog_id).describe(
            "A string that uniquely identifies the default catalog for this surface. It is recommended to prefix this with an internet domain that you own, to avoid conflicts e.g. 'mycompany.com:somecatalog'. Components that do not explicitly specify @catalogId and function calls that do not explicitly specify catalogId will use this surface-level default catalogId.",
        ),
        field("sendDataModel", |p: &CreateSurface| &p.send_data_model).describe(
            "If true, the renderer will send the full data model of this surface in the metadata of every A2A message sent to the agent that created the surface. Defaults to false.",
        ),
        field("components", |p: &CreateSurface| &p.components),
        field("dataModel", |p: &CreateSurface| &p.data_model).describe(
            "The initial root data model object for the surface.",
        ),
        field("metadata", |p: &CreateSurface| &p.metadata).describe("Optional surface-level metadata."),
    ], "Signals the renderer to create a new surface and begin rendering it. Creating a surface implicitly instantiates the canonical 'Surface' container component ('common_types.json#/$defs/Surface') with 'child': 'root'. It is an error to try to create a surface with an existing ID without first deleting it; surfaceId MUST be globally unique for the renderer's lifetime. The component tree and initial data model may be provided inline via 'components' and 'dataModel', or sent later in 'updateComponents' and/or 'updateDataModel' messages for the same surfaceId.")
}

fn update_components() -> Value {
    model([
        field("surfaceId", |p: &UpdateComponents| &p.surface_id).describe(
            "The unique identifier for the UI surface to be updated. It must be globally unique for the renderer's lifetime.",
        ),
        field("components", |p: &UpdateComponents| &p.components),
    ], "Updates a surface with a new set of components. This message can be sent multiple times to update the component tree of an existing surface. One of the components in one of the components lists MUST have an 'id' of 'root' to serve as the root of the component tree. The createSurface message MUST have been previously sent for this surfaceId.")
}

fn update_data_model() -> Value {
    model([
        field("surfaceId", |p: &UpdateDataModel| &p.surface_id).describe(
            "The unique identifier for the UI surface this data model update applies to. It must be globally unique for the renderer's lifetime.",
        ),
        field("path", |p: &UpdateDataModel| &p.path).describe(
            "An optional path to a location within the data model (e.g., '/user/name'). If omitted, or set to '/', refers to the entire data model.",
        ),
        field("value", |p: &UpdateDataModel| &p.value).describe(
            "The data to be updated in the data model. To delete the key/value at 'path', set 'value' explicitly to null.",
        ),
    ], "Updates the data model for an existing surface. This message can be sent multiple times to update the data model. The createSurface message MUST have been previously sent for this surfaceId.")
}

fn delete_surface() -> Value {
    model([field("surfaceId", |p: &DeleteSurface| &p.surface_id).describe(
        "The unique identifier for the UI surface to be deleted. It must be globally unique for the renderer's lifetime.",
    )], "Signals the renderer to delete the surface identified by 'surfaceId'. The createSurface message MUST have been previously sent for this surfaceId.")
}

fn call_renderer_function() -> Value {
    model([
        field("surfaceId", |p: &CallRendererFunction| &p.surface_id).describe(
            "Optional ID of the surface whose data model provides the evaluation context for this call. If omitted, the function executes in a surface-independent root context with an empty data model.",
        ),
        field("functionCallId", |p: &CallRendererFunction| &p.function_call_id)
            .shape(external("CallId"))
            .describe("Unique ID for this instance of the function call. The renderer MUST copy this ID into the return response."),
        field("callFunction", |p: &CallRendererFunction| &p.call_function),
    ], "Signals the renderer to execute a function locally on behalf of the agent.")
}

struct Field {
    name: &'static str,
    schema: Value,
    required: bool,
}

impl Field {
    fn shape(mut self, schema: Value) -> Self {
        self.schema = schema;
        self
    }

    fn describe(mut self, description: &str) -> Self {
        self.schema = described(self.schema, description);
        self
    }
}

// The projection ties field type and presence to the Serde model.
fn field<M, T: FieldType>(name: &'static str, _member: fn(&M) -> &T) -> Field {
    Field {
        name,
        schema: T::schema(),
        required: T::REQUIRED,
    }
}

fn model<const N: usize>(fields: [Field; N], description: &str) -> Value {
    let required: Vec<_> = fields
        .iter()
        .filter(|field| field.required)
        .map(|field| field.name)
        .collect();
    described(
        object(fields.map(|field| (field.name, field.schema)), &required),
        description,
    )
}

trait FieldType {
    const REQUIRED: bool = true;
    fn schema() -> Value;
}

impl<T: FieldType> FieldType for Option<T> {
    const REQUIRED: bool = false;
    fn schema() -> Value {
        T::schema()
    }
}

impl FieldType for String {
    fn schema() -> Value {
        kind("string")
    }
}

impl FieldType for bool {
    fn schema() -> Value {
        kind("boolean")
    }
}

impl FieldType for Value {
    fn schema() -> Value {
        json!({})
    }
}

impl FieldType for JsonObject {
    fn schema() -> Value {
        json!({"type": "object", "additionalProperties": true})
    }
}

impl FieldType for Components {
    fn schema() -> Value {
        local("ComponentsList")
    }
}

impl FieldType for Metadata {
    fn schema() -> Value {
        super::common::metadata(external("Extensions"))
    }
}

impl FieldType for RendererFunctionCall {
    fn schema() -> Value {
        json!({"allOf": [external("FunctionCall"), {"required": ["catalogId"]}]})
    }
}
