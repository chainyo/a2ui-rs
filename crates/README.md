# Crate map

| Crate | Purpose |
| --- | --- |
| [`a2ui-rs`](a2ui-rs/) | Headless Core SDK with JSON Schema validation |
| [`a2ui-basic-catalog`](a2ui-basic-catalog/) | Optional v1.0 Basic Catalog functions |

See [Protocol types](../docs/protocol.md) for the public message types.
Its dependency list must not contain a UI framework, model provider, transport, or async runtime.
`tests/upstream.rs` checks the upstream submodule pin, executes supported common-type cases, and reports pending cases.
The conformance tests use the Basic Catalog crate and a YAML parser as dev-dependencies.

Add agent helpers or a GPUI adapter only when their implementation starts.
Both must depend on core. Core must not depend on either.
