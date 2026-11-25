# Conformance

## Test procedure

Initialize the upstream submodule before you run the tests.
The Core harness uses the public Rust APIs for supported upstream cases.
The lifecycle cases send decoded messages to `SurfaceStore::apply`.
Catalog cases load schemas with `Catalog::from_json`.
Validation cases send decoded messages to `MessageProcessor::apply_with_config`.
Parser cases send templates to `expression::parse_template`.
Resolution cases use `MessageProcessor` to read resolved properties and check function calls.
RPC cases use `RpcHandler` to send calls and receive responses.
The RPC tests advance a monotonic clock to check timeouts.
Agent routing cases use `RoutingSession` to mount expressions and execute actions.
The harness checks wire requests, evaluation values, event order, and correlated errors.
Node resolution cases use `RoutingSession::mount_tree` and `advance`.
The harness checks the initial node, bound values, and update counts as messages arrive.
The collection case writes through a node binding and checks sibling values and node IDs.
Mounted action cases check catalog action properties and metadata on emitted events.
Node metadata cases compare property and metadata notification counts for the same node IDs.
The mounted validation cases check warning and info results, email validation, and boolean composition.
Function cases check how `and` and `not` use the result of `required`.
The regex cases check matching, pattern rejection, and input limits.
The formatting cases check date patterns, timestamp offsets, currencies, and plural forms.
They also check catalog selection in templates and how validation results control digit grouping.
The mounted formatting test checks the effects of locale, time zone, and surface data changes.
The `openUrl` cases check user activation, host callback calls, and the absence of agent action events.
The accessibility cases compare resolved attributes on mounted nodes.
The report keeps these cases pending until an adapter verifies the platform and axe-core assertions.
The pointer and deletion suites apply message payloads to one data model and compare the final JSON.
It compares the JSON value before and after serialization for each valid input.
An invalid input must fail deserialization.

Run the Core harness with:

```sh
mise exec -- cargo test --package a2ui-rs --features catalog --test upstream --locked -- --nocapture
```

The harness verifies the Git pin, the clean checkout, and the required reference paths.
It reads each suite in `conformance/core/*.yaml`.
Read `target/conformance/protocol.jsonl` for the results.
The report uses the upstream executed-case manifest format.

Run the protocol tests for the upstream renderer examples and JSON fixtures:

```sh
mise exec -- cargo test --package a2ui-rs --test protocol --locked
```

The protocol tests use separate fixtures. The Core report contains only the Core YAML cases.
See [Protocol types](protocol.md) for the validation boundaries.

## Processor timing

Run the timing tests in an optimized build.

```sh
mise run timing
```

The test expands the pinned payload templates before timing.
It loads the catalogs before each run.
It measures message processing after a warmup and compares the median times for the two input sizes.
For the upstream `none` setting, the test permits unresolved references during streaming.
The Rust processor uses its other default validation rules for this setting.
Package validation runs the timing tests in a separate Linux job.

## Upstream reference

`fixtures/upstream/` is a Git submodule of [a2ui-project/a2ui](https://github.com/a2ui-project/a2ui).
The parent repository records the exact commit.

Initialize the recorded checkout before running tests:

```sh
git submodule update --init
```

Read the recorded revision with:

```sh
git ls-tree HEAD fixtures/upstream
```

The checkout contains the protocol specifications, SDK blueprints, catalogs, examples, and complete upstream conformance suites.
The tests use v1.0 references and classify dedicated legacy suites outside scope.
For Catalog, Data Model, and Data Context cases without a case version, the harness uses the catalog version.
The harness also excludes fixtures that require successful use of a pre-v1.0 message, action format, or data model query.
Rejection of an older message or catalog remains part of v1.0 coverage.
Node fixtures that use a v0.9.1 setup catalog are outside scope.
Upstream files retain their own license notices. Do not edit them inside this repository.

Install Git before you run the inventory test. The test uses local Git metadata.
A missing checkout, mismatched commit, or local modification causes the test to fail.
Version example tests read the initialized submodule at runtime.

## Execution contract

Implement each case against the same public Core SDK used by applications.
Do not add an alternative test-only state engine.

The runner uses these results. Its JSONL status is `skipped` for pending and outside-scope cases:

| Result | Required evidence |
| --- | --- |
| Passed | Public implementation produced the expected state, event, or error |
| Failed | The actual result differs from the upstream expectation |
| Pending | The action or feature is not implemented |
| Expected failure (`xfailed`) | The harness verifies the protocol behavior and records why the fixture expects a different result |
| Outside scope | A precise version or layer reason excludes the case |

Do not skip a case because it expects an error or uses strict validation.
Recursion, invalid topology, disposal, notifications, data scopes, and RPC routing remain part of the relevant contract.
Report partial coverage as partial coverage.
A conformance gate must require a passing result for each required v1.0 case.
Inventory success must never become that gate.

The RPC fixtures exercise errors in mounted checks and action pipelines.
Check cases compare the validation result and the correlated renderer error.
Action cases compare the surface data and the correlated renderer error.

## Validation transactions

The processor cases check permissive streaming and validation rules for one message.
The harness compares the surface before and after a rejected update.
The duplicate-ID and cycle fixtures check rejection, rollback, and the renderer error envelope.
The harness checks the error code, message, and surface ID.
The invalid pointer and component validation cases also check field paths.

The composition cases check `Surface` as the implicit parent of the root.
They compare error codes and field paths for disallowed parents and children.
A rejected create leaves the surface absent. A rejected update preserves the existing surface.

The catalog lookup cases check unknown catalog IDs and absent surface defaults.
The incompatible catalog case checks registration rejection and the error for a v1.0 surface request.
The incoming envelope cases check the field path or function call ID in the renderer error.

## Catalog registration

The `add_catalog` cases register a catalog between messages and check which surfaces can use it.
The case for an earlier surface uses `catalogId` for a component override.
The harness passes that override as `@catalogId`, the v1.0 component envelope field.

## Catalog documents

The harness loads the published Basic, MCP, and VegaChart catalogs.
It checks entries selected by the catalog unions and validates component samples.
The export cases compare catalog JSON before and after loading it again.
The validation schema cases compare the bundled schemas with the pinned files.
The derivation cases check entry selection, replacement, and errors for unknown entries.

## Multiple catalogs

The selection cases build surfaces from the catalog IDs in the fixtures.
They check component updates and function calls through `MessageProcessor`.
The incompatible catalog case checks rejection when the catalog loads.
The tree cases check resolved properties and the catalog used by calls in a template.
The harness passes a component override as `@catalogId`. Function calls keep `catalogId`.

## Reserved keys

The harness checks acceptance of `@path`, `@call`, and `@index` in v1.0 component properties.
It checks rejection of unescaped single-`@` keys in dynamic objects.
The resolution cases compare literal keys in both component snapshots and mounted nodes.

## Event actions

The mounted event cases check scoped context, escaped keys, and dispatch metadata.
The direct dispatch case checks the error for a non-string `userMessage`.
The Basic Catalog rejects a missing event name before it stores the component.
The missing-name fixture expects `INVALID_ACTION` during dispatch.
The harness verifies catalog rejection and keeps the dispatch assertion pending.

## Fixture discrepancies

### RPC activation

The v1.0 catalog schema requires `rendererOnly` for functions that require user activation.
The RPC activation fixtures supply `rendererOrAgent` for those functions.
The harness checks catalog rejection, RPC denial, and gesture handling through a local renderer action.

Use renderer actions to test execution with and without a gesture.
Use a renderer RPC case to test denial of a `rendererOnly` function.
Use an ungated `rendererOrAgent` function to test successful renderer RPC.

### Non-finite arguments

The Core blueprint requires local rejection of resolved arguments that have no JSON form.
The Rust JSON boundary rejects NaN and infinity before routing.
The harness checks this boundary and verifies that finite arguments still route from a binding and an action.

Use the routing diagnostic for runtimes that accept non-finite values during expression evaluation.
Use JSON boundary rejection for runtimes with JSON-only values.

### Check rule message precedence

The v1.0 protocol uses a failed function's message when the function supplies one.
The rule message supplies a fallback when the function omits its message.
The rule order fixture expects the fallback in place of the email function's message.
The harness checks the function's message and the fixture's rule order.
Use the email function's message in the corresponding error and result expectations.
Keep the order, warning, and length fallback assertions.

## Updates

Dependabot checks the submodule weekly and opens a pull request for an updated pin.
The branch setting in `.gitmodules` selects the upstream branch for update proposals.
Initialization uses the commit recorded in the parent repository.

For each update:

1. Review changes to the v1.0 specification, SDK blueprints, and relevant suites.
2. Initialize the proposed checkout with `git submodule update --init`.
3. Run the integration tests and read the conformance report.
4. Review added, removed, renamed, and changed cases.
5. State any protocol or coverage change in the pull request.

For a manual update, select a full upstream commit SHA and stage the submodule pointer:

```sh
git -C fixtures/upstream fetch origin UPSTREAM_SHA
git -C fixtures/upstream checkout --detach UPSTREAM_SHA
git add fixtures/upstream
```

The inventory compares the checkout with the staged pin during local validation.
Keep the pin update separate from unrelated changes.
