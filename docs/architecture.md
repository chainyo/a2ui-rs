# Architecture

## Reference baseline

Use the pinned upstream Core, Agent, and Framework Adapter blueprints in `fixtures/upstream/blueprints/modules/`.
The Git submodule pointer records their revision. See [conformance](conformance.md) for setup and updates.
The protocol contract starts at v1.0. Legacy adapters are outside scope.

## Dependency direction

```text
Future agent helpers ──> a2ui-core <── Future framework adapters
                              ↑
                       Application code
```

Core will own these responsibilities from the upstream blueprint:

- Protocol models and serialization.
- Catalog schemas and component metadata.
- Surface, component, and data model state.
- Message processing and incremental updates.
- JSON Pointer scopes, reactive bindings, and expression resolution.
- Structural validation, reference checks, and recursion checks.
- Multiple catalogs and v1.0 bidirectional function RPC lifecycle.

These are planned responsibilities. Only the version type is implemented.
Add modules as these responsibilities acquire tested behavior.
Do not add a separate crate for each module without a concrete need.

Core must remain independent of host UI frameworks, network clients, model providers, and async runtimes.
Use explicit caller-owned interfaces when a host service becomes necessary.
Do not select an async runtime before the RPC contract requires an integration boundary.

## Future layers

Agent helpers will own capability negotiation, prompts, inference compilation, and outgoing validation through core.
Framework adapters will map resolved state to native widgets and route user events.
GPUI is a possible adapter. It is not a Core SDK dependency.

## Public API checks

Integration tests exercise the public crate.
Rustdoc checks public documentation. Clippy checks all targets.
The upstream audit runs as an integration test with dev-dependencies.
Its YAML parser is a dev-dependency. The SDK runtime does not depend on Git or the upstream checkout.

Hawk assumes that product callers live inside the workspace.
This workspace contains only a public library, so there is no suitable Hawk target.
Keep the pinned installer available for future internal products.
Do not add an artificial binary or restrict the public SDK to satisfy Hawk.

## Repository precedent

The workspace follows [rudolint](https://github.com/kubeply/rudolint) at revision `bb153e24b9b0398f7bddf0631e7b2641444c3e46`.
It uses `crates/`, `.cargo/`, `docs/`, realistic fixtures, a pinned toolchain, Clippy, cargo-deny, and Hawk.
The SDK adapts the application visibility policy to an external library boundary.
