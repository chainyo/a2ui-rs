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
- `updateComponents` adds or replaces component definitions by ID. An absent surface ID causes an unknown surface error.
- `updateDataModel` changes the data of the destination surface. An absent ID causes an unknown surface error.
- `deleteSurface` removes the surface. A delete for an absent ID leaves the store unchanged.

Component definitions stay in order of insertion. A replacement keeps its position and uses all fields from the new definition.
A duplicate component ID in one message causes an error. The store keeps its previous state.
After a delete, a create message can use the same surface ID.

## Catalog and topology validation

Load each v1.0 catalog with `Catalog::from_json`. Supply the v1.0 common types schema.
Register each catalog with `MessageProcessor::add_catalog`.
Pass decoded messages to `MessageProcessor::apply` to validate them before storage.
The processor uses the catalog ID on the component or function call.
If the ID is absent, the processor uses the surface default.
Schema validation uses only schemas in memory.

The processor checks component relationships on the graph that would result from the message.
The checks include parent and child rules, missing references, cycles, depth, and reachability from `root`.
An error leaves the surface unchanged.
Use `ValidationConfig` to permit incomplete graphs during streaming.

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

## Binding and expression resolution

Use `DataScope::default` for the root data scope.
Use `DataScope::collection` to get one scope per array item.
An absolute path starts at the root. A relative path starts at the current scope.
Use `MessageProcessor::write_binding` to write data from a renderer input.

Use `MessageProcessor::resolve_value` to evaluate a binding or expression in one scope.
Use `MessageProcessor::resolve_components` to get a snapshot of the component tree.
The snapshot expands collection templates and keeps actions for event dispatch.
Read a new snapshot after the surface data changes.

Register renderer function handlers with `LocalFunctions::insert`.
Use `basic_catalog::register_validation_functions` for the v1.0 Basic Catalog.
It registers `required`, `email`, `numeric`, `length`, `regex`, `and`, `or`, and `not`.
`numeric` compares the resolved `DynamicNumber` with inclusive bounds.
The protocol converts an invalid numeric binding to zero.
`length` counts Unicode code points in the resolved string.
`regex` checks the A2UI dialect and safety rules before it matches a value.
Use the catalog ID and function name for each registration.
The processor checks calls against the catalog schemas before it invokes handlers.
The SDK evaluates `formatString` and the collection function `@index`.
Calls in a `formatString` template use its catalog. The system function `@index` uses the nearest collection scope.

## Bidirectional RPC

Create one `RpcHandler` for each transport session.
Send incoming messages through `RpcHandler::receive` or `RpcHandler::receive_json`.
Send each returned renderer message on the transport.
Use `ReceiveError::renderer_error` to get a correlated error for an invalid RPC envelope.

Use `RpcHandler::call_agent_function` for a renderer-initiated call.
Supply a function call ID that the session has never used.
Supply an outbound callback to send the request. Report transport failures through this callback.
The returned `PendingCall` completes when the handler receives the matching response.
Cancel a pending call explicitly or drop its handle.

Use `RpcHandler::next_deadline` to schedule a timer. Call `RpcHandler::expire` when the timer expires.
Use the same monotonic clock for incoming messages and deadlines.
The default timeout is 30 seconds.
Surface deletion cancels pending calls for that surface.
Disposal cancels all pending calls and closes the session.

The handler resolves arguments in the named surface. A renderer call without a surface uses an empty data model.
The handler checks catalog permissions before it calls a renderer function.
An agent cannot invoke a function that requires user activation.
Use `CallOptions` to mark a renderer-initiated call that comes from an active user gesture.

## Agent routing

Use `RoutingSession` to evaluate bindings, checks, and actions that can call the agent.
Mount a component property with `mount_property`.
Use `mount_binding` for a standalone expression. Use `mount_check` for a standalone check condition.
Use `mount_check_rule` for a component's `@checks` entry. Supply its zero-based index.
Use `validation_result` to read the rule result, including its message, severity, and additional fields.
Use `ValidationResult::is_blocking` to find failures that prevent submission.
Use `run_action` for a function action. Use `dispatch_event` for an event with ordered context entries.
Use `unmount` when the renderer discards a binding, check, or completed action.

Pass inbound messages to `receive`. Call `advance` after a message, data change, or deadline.
Supply the outbound transport callback to `advance`.
Use its result to update the changed evaluations and send renderer errors.
A pending check passes until the call completes. An agent error makes the check fail.

The session uses the local implementation when the catalog has one.
If no local implementation exists, the session sends the validated call to the agent.
Bindings and checks share identical requests on the same surface and can use cached answers.
An action sends a new request for each invocation.
Use `LocalFunctions::insert_action` to register a local function that changes surface data.
The action context reads and writes paths in the action's scope.
The session applies a successful function's writes before it runs the next step.
If a function fails, the action stops. The session keeps the writes from earlier successful functions.
Event context entries resolve in order. An error stops the event before dispatch.

## Reactive node trees

Call `RoutingSession::mount_tree` to track a surface.
Call `advance` after each message to publish node changes in message order.
Read the current root and nodes with `node_tree`.
Each node gives the IDs and field paths of its children.

A node keeps its ID across property changes and explicit sibling reordering.
A collection template mounts one node per array item.
A template node keeps its ID while its array index stays present.
A nested template resolves its path in the parent node's scope.
Use `write_node_property` to write a value through a node's data binding.
The session resolves a relative binding path in that node's scope.
An update reports each change type once for a node.
Removing a node also removes its descendants and their property evaluations.
Use `unmount_tree` to stop tracking a surface.

Read the component's `@metadata` envelope from `MountedNode::metadata`.
`NodeChangeKind::MetadataUpdated` reports a change to this envelope.
An update that omits `@metadata` clears the node's metadata.
A catalog property named `metadata` stays in the node's properties.

The session evaluates `@checks` in each mounted node's data scope.
Read `isValid`, `validationErrors`, and `validationResults` from the node's properties.
Error results make the node invalid. Warning and info results remain in `validationResults`.
The session reports a validation change as a property update.

Read the resolved `@accessibility` envelope from the node's properties.
The session resolves its label, description, and hidden values in the node's data scope.
The envelope also supplies the live announcement policy.
An accessibility change uses the same property update as other node changes.
A catalog property named `accessibility` stays in the node's properties.

Use `MountedProperty::is_action` to find action properties.
Call `dispatch_node_action` with the node ID and property name.
The session resolves arguments in the node's scope when it runs the action.
Set the timestamp and optional metadata in `ActionDispatchOptions` for an event.
Function actions use the function routing path.

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
