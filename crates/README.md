# Crate map

| Crate | Purpose | Published |
| --- | --- | --- |
| `a2ui-rust-core` | Public protocol and state foundations | No |

`a2ui-rust-core` exposes the protocol version boundary and typed v1.0 wire messages.
Its dependency list must not contain a UI framework, model provider, transport, or async runtime.
`tests/upstream.rs` checks the upstream submodule pin, executes supported common-type cases, and reports pending cases.
Its YAML parser is a dev-dependency. It does not enter the SDK runtime dependency tree.

Add agent helpers or a GPUI adapter only when their implementation starts.
Both must depend on core. Core must not depend on either.
