# Crate map

| Crate | Purpose | Published |
| --- | --- | --- |
| `a2ui-core` | Public protocol and state foundations | No |

`a2ui-core` currently exposes only the protocol version boundary.
Its dependency list must not contain a UI framework, model provider, transport, or async runtime.
`tests/upstream.rs` verifies reference integrity and inventories pending conformance cases.
Its YAML and hashing dependencies are dev-dependencies. They do not enter the SDK runtime dependency tree.

Add agent helpers or a GPUI adapter only when their implementation starts.
Both must depend on core. Core must not depend on either.
