# Crate map

| Crate | Purpose | Published |
| --- | --- | --- |
| `a2ui-core` | Public protocol and state foundations | No |
| `xtask` | Reference integrity checks and conformance inventory | No |

`a2ui-core` currently exposes only the protocol version boundary.
Its dependency list must not contain a UI framework, model provider, transport, or async runtime.
`xtask` is repository tooling. Its dependencies do not enter the SDK dependency tree.

Add agent helpers or a GPUI adapter only when their implementation starts.
Both must depend on core. Core must not depend on either.
