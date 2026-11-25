use serde_json::{Map, Value, json};

use crate::protocol::{IMPLICIT_SURFACE_COMPONENT, Live};

use super::{described, document, kind, local, object, reference};

/// Builds the v1.0 Common Types JSON Schema.
///
/// Register the active catalog to resolve the function references.
#[must_use]
pub fn common_types_schema() -> Value {
    let definitions = [
        (
            "ComponentId",
            described(
                kind("string"),
                "The unique identifier for a component, used for both definitions and references within the same surface.",
            ),
        ),
        (
            "CallId",
            described(kind("string"), "The unique identifier for a function call."),
        ),
        ("AccessibilityAttributes", accessibility()),
        ("Extensions", extensions()),
        ("ComponentCommon", component_common()),
        (
            "Child",
            described(
                local("ComponentId"),
                "A reference to a single child component ID.",
            ),
        ),
        ("ChildList", child_list()),
        (
            "DataBinding",
            object(
                [(
                    "@path",
                    described(
                        kind("string"),
                        "A JSON Pointer path to a value in the data model.",
                    ),
                )],
                &["@path"],
            ),
        ),
        ("DynamicValue", dynamic_value()),
        (
            "DynamicString",
            dynamic([kind("string")], "Represents a string"),
        ),
        (
            "DynamicNumber",
            dynamic(
                [kind("number")],
                "Represents a value that can be either a literal number, a path to a number in the data model, or a function call returning a number.",
            ),
        ),
        (
            "DynamicBoolean",
            dynamic(
                [kind("boolean")],
                "A boolean value that can be a literal, a path, or a function call returning a boolean or a validation result object (coerced to its 'valid' boolean).",
            ),
        ),
        (
            "DynamicStringList",
            dynamic(
                [array(kind("string"))],
                "Represents a value that can be either a literal array of strings, a path to a string array in the data model, or a function call returning a string array.",
            ),
        ),
        ("FunctionCommon", function_common()),
        ("IndexSystemFunction", index_function()),
        ("FunctionCall", function_call()),
        ("CheckRule", check_rule()),
        ("Checkable", checkable()),
        ("Action", action()),
        ("Surface", surface()),
        ("FunctionResponse", function_response()),
    ];
    document(
        "common_types.json",
        "A2UI Common Types",
        "Common type definitions used across A2UI schemas.",
        definitions
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<Map<_, _>>(),
    )
}

fn array(items: Value) -> Value {
    let mut schema = kind("array");
    schema["items"] = items;
    schema
}

fn one_of<const N: usize>(variants: [Value; N]) -> Value {
    json!({"oneOf": Value::Array(Vec::from(variants))})
}

fn dynamic<const N: usize>(literals: [Value; N], description: &str) -> Value {
    let alternatives: Vec<_> = literals
        .into_iter()
        .chain([local("DataBinding"), local("FunctionCall")])
        .collect();
    json!({"description": description, "oneOf": alternatives})
}

fn accessibility() -> Value {
    described(
        object(
            [
                (
                    "label",
                    described(
                        local("DynamicString"),
                        "A short string, typically 1 to 3 words, used by assistive technologies to convey the purpose or intent of an element. For example, an input field might have an accessible label of 'User ID' or a button might be labeled 'Submit'.",
                    ),
                ),
                (
                    "description",
                    described(
                        local("DynamicString"),
                        "Additional information provided by assistive technologies about an element such as instructions, format requirements, or result of an action. For example, a mute button might have a label of 'Mute' and a description of 'Silences notifications about this conversation'.",
                    ),
                ),
                (
                    "live",
                    described(
                        json!({
                            "type": "string", "enum": [Live::Off, Live::Polite, Live::Assertive], "default": Live::Off,
                        }),
                        "Controls screen reader announcements for dynamic updates (WAI-ARIA aria-live). 'polite' waits for user pause; 'assertive' interrupts immediately for alerts.",
                    ),
                ),
                (
                    "hidden",
                    described(
                        local("DynamicBoolean"),
                        "Hides the element and its children from assistive technologies when true. Default is false.",
                    ),
                ),
            ],
            &[],
        ),
        "Attributes to enhance accessibility when using assistive technologies like screen readers or model understanding.",
    )
}

fn extensions() -> Value {
    described(
        json!({
            "type": "object",
            "patternProperties": {r"^[\p{XID_Start}_][\p{XID_Continue}]*$": {}},
            "additionalProperties": false,
        }),
        "Optional extension metadata. Keys MUST be Unicode identifiers (UAX #31). Keys starting with 'a2ui_' are reserved for official extensions.",
    )
}

pub(super) fn metadata(extensions: Value) -> Value {
    object([("extensions", extensions)], &[])
}

fn component_common() -> Value {
    let mut schema = object(
        [
            ("id", local("ComponentId")),
            (
                "@catalogId",
                described(
                    kind("string"),
                    "The catalog ID for this component, overriding any surface-level default catalogId.",
                ),
            ),
            ("@accessibility", local("AccessibilityAttributes")),
            (
                "@metadata",
                described(
                    metadata(local("Extensions")),
                    "Optional component-level metadata for vendor extensions.",
                ),
            ),
        ],
        &["id"],
    );
    if let Some(fields) = schema.as_object_mut() {
        fields.remove("additionalProperties");
    }
    schema
}

fn child_list() -> Value {
    one_of([
        described(
            array(local("ComponentId")),
            "A static list of child component IDs.",
        ),
        described(
            object(
                [
                    ("componentId", local("ComponentId")),
                    (
                        "path",
                        described(
                            kind("string"),
                            "The path to the list of component property objects in the data model.",
                        ),
                    ),
                ],
                &["componentId", "path"],
            ),
            "A template for generating a dynamic list of children from a data model list. The `componentId` is the component to use as a template.",
        ),
    ])
}

fn dynamic_value() -> Value {
    dynamic(
        [
            kind("string"),
            kind("number"),
            kind("boolean"),
            kind("array"),
            json!({
                "type": "object", "propertyNames": {"not": {"pattern": "^@([^@]|$)"}},
                "not": {"anyOf": [{"required": ["@path"]}, {"required": ["@call"]}]},
            }),
        ],
        "A value that can be a literal, a path, or a function call returning any type.",
    )
}

fn function_common() -> Value {
    described(
        json!({
            "type": "object",
            "properties": {
                "@call": described(kind("string"), "The name of the function to call."),
                "catalogId": described(kind("string"), "The catalog ID for this function, overriding any surface-level default catalogId."),
            },
            "required": ["@call"],
        }),
        "Baseline envelope properties common to all function calls. Function-specific argument schemas ('args') are defined individually by each function in the active catalog.",
    )
}

fn index_function() -> Value {
    let mut offset = described(
        local("DynamicNumber"),
        "Optional. An offset to add to the 0-based index (e.g., 1 for 1-based indexing). Defaults to 0.",
    );
    offset["default"] = json!(0);
    described(
        json!({
            "type": "object",
            "properties": {
                "@call": {"const": "@index"},
                "args": {
                    "type": "object", "properties": {"offset": offset}, "unevaluatedProperties": false,
                },
            },
            "required": ["@call"], "unevaluatedProperties": false, "returnType": "number",
        }),
        "Returns the 0-based index of the current item when rendering a dynamic list from a template. This function MUST ONLY be available when evaluating template items within a list context.",
    )
}

fn function_call() -> Value {
    described(
        json!({
            "type": "object",
            "allOf": [local("FunctionCommon"), one_of([
                reference("catalog.json#/$defs/anyFunction"),
                local("IndexSystemFunction"),
            ])],
            "unevaluatedProperties": false,
        }),
        "Invokes a named function, combining common function properties with the catalog function definition.",
    )
}

fn check_rule() -> Value {
    described(
        object(
            [
                (
                    "condition",
                    described(
                        one_of([local("DataBinding"), local("FunctionCall")]),
                        "Path or function call evaluating to a structured validation result object or a boolean.",
                    ),
                ),
                (
                    "message",
                    described(kind("string"), "Optional fallback error message."),
                ),
            ],
            &["condition"],
        ),
        "A single validation check rule applied to an input component. The condition function or path evaluates to a structured validation result object or a boolean (which is coerced to a validation result object).",
    )
}

fn checkable() -> Value {
    json!({
        "type": "object",
        "description": "Properties for components that support renderer-side checks.",
        "properties": {
            "@checks": described(array(local("CheckRule")), "A list of validation check rules to perform on the component."),
        },
    })
}

fn action() -> Value {
    let event = described(
        object(
            [
                (
                    "name",
                    described(
                        kind("string"),
                        "The name of the action to be dispatched to the agent.",
                    ),
                ),
                (
                    "userMessage",
                    described(
                        local("DynamicString"),
                        "An optional human-readable message describing the action performed by the user, resolved by the renderer and included in the outbound action message.",
                    ),
                ),
                (
                    "context",
                    described(
                        json!({"type": "object", "additionalProperties": local("DynamicValue")}),
                        "A JSON object containing the key-value pairs for the action context. Values can be literals or paths. Use literal values unless the value must be dynamically bound to the data model. Do NOT use paths for static IDs.",
                    ),
                ),
            ],
            &["name"],
        ),
        "The event to dispatch to the agent.",
    );
    described(
        one_of([
            described(
                object([("event", event)], &["event"]),
                "Triggers an agent-side event.",
            ),
            described(
                object([("functionCall", local("FunctionCall"))], &["functionCall"]),
                "Executes a renderer or agent-side function.",
            ),
        ]),
        "Defines an interaction handler that can either trigger an agent-side event or execute a local renderer-side function.",
    )
}

fn surface() -> Value {
    let mut schema = described(
        object(
            [
                ("component", json!({"const": IMPLICIT_SURFACE_COMPONENT})),
                ("child", json!({"const": "root"})),
            ],
            &[],
        ),
        "The reserved canonical container component representing an A2UI surface. The Surface component is immutable and always has 'child': 'root'.",
    );
    schema["title"] = json!("Surface Container Component");
    schema["allowedParents"] = json!([]);
    schema
}

fn function_response() -> Value {
    let mut schema = described(
        object(
            [
                (
                    "functionCallId",
                    described(
                        local("CallId"),
                        "The unique ID matching the initiating function call.",
                    ),
                ),
                (
                    "value",
                    json!({"description": "The return value of the function."}),
                ),
                (
                    "error",
                    described(
                        object(
                            [("code", kind("string")), ("message", kind("string"))],
                            &["code", "message"],
                        ),
                        "An error object indicating failure of the function execution.",
                    ),
                ),
            ],
            &["functionCallId"],
        ),
        "The return response matching a callAgentFunction or callRendererFunction invocation.",
    );
    schema["oneOf"] = json!([{"required": ["value"]}, {"required": ["error"]}]);
    schema
}
