# Protocol types

The `a2ui-rs` package exposes its protocol types under `a2ui_rs::protocol`.

## Message envelopes

`AgentToRendererMessage::V1` contains one `AgentToRenderer` operation:

- `CreateSurface`
- `UpdateComponents`
- `UpdateDataModel`
- `DeleteSurface`
- `CallRendererFunction`
- `AgentFunctionResponse`

`RendererToAgentMessage::V1` contains one `RendererToAgent` operation:

- `Action`
- `CallAgentFunction`
- `RendererFunctionResponse`
- `Error`

Serde reads and writes the upstream `version` tag and camelCase operation names.
Use version `"1.0"` for these message types.
The parser requires a version and exactly one operation. It rejects extra envelope fields.
`MessageBatch<T>` represents a strict `{ "messages": [...] }` wrapper.
Use a `Vec<T>` for a plain message list. Empty lists are valid.

## Presence and validation

Serialization keeps omitted optional fields absent.
Deserialization rejects an explicit null where the schema requires a non-null type.
Required data update values and successful function results retain explicit null.
False booleans and empty objects remain present when supplied.
Compare the JSON values after serialization. Whitespace and key order can change.

`Components` requires a nonempty list. `Surface` is an implicit container and cannot appear as a component definition.
Metadata accepts only its `extensions` field. Extension keys use the Unicode XID identifier rules through `unicode-ident`.
Function outcomes contain exactly one value or error.
Generic renderer errors contain exactly one surface or function correlation target.
Reserved validation error codes require the validation error shape.
Additional component and action properties cannot overwrite typed envelope fields during serialization.

## Validation boundaries

Component properties and function arguments depend on their catalog, so their values remain JSON data.
Use catalog validation for catalog names, component properties, and function arguments.
Use topology validation for component relationships.
Timestamp fields require a string. The caller must supply a valid date-time value.

Parsing errors use Serde's error interface.
See [Conformance](conformance.md) for test commands and report fields.
