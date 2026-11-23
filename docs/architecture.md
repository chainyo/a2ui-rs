# Architecture

## Reference baseline

Use the pinned upstream Core, Agent, and Framework Adapter blueprints in `fixtures/upstream/blueprints/modules/`.
The Git submodule pointer records their revision. See [conformance](conformance.md) for setup and updates.
The protocol contract starts at v1.0. Legacy adapters are outside scope.

## Dependency direction

```text
Agent helpers ──> a2ui-rs <── Framework adapters
                     ↑
              Application code
```

The upstream Core blueprint defines these responsibilities.

- Protocol models and serialization.
- Catalog schemas and component metadata.
- Surface, component, and data model state.
- Message processing and incremental updates.
- JSON Pointer scopes, reactive bindings, and expression resolution.
- Structural validation, reference checks, and recursion checks.
- Multiple catalogs and v1.0 bidirectional function RPC lifecycle.

Add modules as these responsibilities acquire tested behavior.
Do not add a separate crate for each module without a concrete need.

Core must remain independent of host UI frameworks, network clients, model providers, and async runtimes.
Use explicit caller-owned interfaces when a host service becomes necessary.
Do not select an async runtime before the RPC contract requires an integration boundary.

## SDK layers

Agent helpers are responsible for capability negotiation, prompts, and inference compilation.
They use the core to validate outgoing messages.
Framework adapters map resolved state to native widgets. They also route user events.

## Surface lifecycle

Create a `SurfaceStore` for each renderer. Pass decoded messages to `SurfaceStore::apply`.
Use `SurfaceStore::get` to read a surface by ID.

- `createSurface` stores the configuration and initial data. An active ID causes a duplicate surface error.
- `updateDataModel` changes the data of the destination surface. An absent ID causes an unknown surface error.
- `deleteSurface` removes the surface. A delete for an absent ID leaves the store unchanged.

Component definitions from `createSurface` stay in the order in the message.
Catalog and component tree validation belong to the validation layer.
After a delete, a create message can use the same surface ID.

## Data model

Each surface has one `DataModel`. The store uses the object from `createSurface` as its initial data.
If an update omits the path, the store passes `/` to `DataModel::set`.

Use `DataModel::get` to read values at absolute A2UI JSON Pointers.
The data model accepts the `~0` and `~1` escapes from [RFC 6901](https://www.rfc-editor.org/rfc/rfc6901.html).
An empty path or a path with only slashes selects the root.
The data model ignores empty path segments.

A write creates missing objects and arrays. Missing array positions hold JSON null.
The maximum array index for a write is 10,000.
A null write removes an object field or sets an array position to null.
A null write to the root replaces the data with an empty object.
A null write to a missing path leaves the data unchanged.

## Public API checks

Integration tests exercise the public crate during local validation and package-release validation.
Rustdoc checks public documentation. Clippy checks all targets.
The upstream integration test uses Git and a YAML parser.
The YAML parser is a dev-dependency.
See [Protocol types](protocol.md) for the message types.

Run Hawk only for an internal product with a suitable analysis target.
Keep the public SDK available to callers outside the workspace.

## Repository precedent

The workspace follows [rudolint](https://github.com/kubeply/rudolint) at revision `bb153e24b9b0398f7bddf0631e7b2641444c3e46`.
It uses `crates/`, `.cargo/`, `docs/`, realistic fixtures, a pinned toolchain, Clippy, cargo-deny, and Hawk.
Mise installs pinned repository tools for local development and CI.
The SDK adapts the application visibility policy to an external library boundary.
