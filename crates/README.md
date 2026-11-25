# Crate map

[`a2ui-rs`](a2ui-rs/) contains the headless Core SDK and JSON Schema validation.
Enable the `basic-catalog` feature to use the v1.0 Basic Catalog functions.

See [Protocol types](../docs/protocol.md) for the public message types.
Its dependency list must not contain a UI framework, model provider, transport, or async runtime.
`tests/upstream.rs` checks the upstream submodule pin, executes supported common-type cases, and reports pending cases.
Use the `basic-catalog` feature to run the upstream conformance tests.
The harness uses a YAML parser as a dev dependency.

Add agent helpers or a GPUI adapter only when their implementation starts.
Both must depend on core. Core must not depend on either.
