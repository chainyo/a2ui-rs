# Crate map

| Crate | Purpose |
| --- | --- |
| [`a2ui-rs`](a2ui-rs/) | Core SDK |

See [Protocol types](../docs/protocol.md) for the public message types.
Its dependency list must not contain a UI framework, model provider, transport, or async runtime.
`tests/upstream.rs` checks the upstream submodule pin, executes supported common-type cases, and reports pending cases.
Its YAML parser is a dev-dependency. It does not enter the SDK runtime dependency tree.

Add agent helpers or a GPUI adapter only when their implementation starts.
Both must depend on core. Core must not depend on either.
