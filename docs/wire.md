# Wire types

The `a2ui-rust-core` package exposes its wire types under `a2ui_rust_core::wire`.
The package name was available when selected. Publication is still disabled.

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
Only `"1.0"` is accepted. Missing versions, extra envelope fields, and multiple operations are rejected.
`MessageBatch<T>` represents a strict `{ "messages": [...] }` wrapper.
Use a `Vec<T>` for a plain message list. Empty lists are valid.
Nested payload normalization is not implemented.

## Presence and validation

Optional fields preserve omission. Explicit null is rejected where the schema requires a non-null type.
Required data update values and successful function results retain explicit null.
False booleans and empty objects remain present when supplied.
Serialization preserves JSON values, not whitespace or input key order.
It does not implement cryptographic JSON canonicalization.

`Components` requires a nonempty list. `Surface` is an implicit container and cannot appear as a component definition.
Metadata accepts only its `extensions` field. Extension keys use the Unicode XID identifier rules through `unicode-ident`.
Function outcomes contain exactly one value or error.
Generic renderer errors contain exactly one surface or function correlation target.
Reserved validation error codes require the validation error shape.
Additional component and action properties cannot overwrite typed envelope fields during serialization.

## Current limits

Component properties and function arguments depend on their catalog, so their values remain JSON data.
Catalog names, property schemas, function argument schemas, and topology are not validated by these types.
Timestamp fields enforce a string type but do not validate date-time syntax.
The types do not evaluate expressions, mutate surface state, route RPCs, or select a UI or async runtime.

Parsing errors use Serde's error interface.
The complete A2UI validation error taxonomy and field-path diagnostics are not implemented.
Use the [conformance report](conformance.md) to distinguish implemented cases from pending behavior.
