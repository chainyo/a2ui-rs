# Conformance

## Current coverage

The Core runner executes three v1.0 cases from `core/common_types.yaml`:

- `test_v10_extensions_keys_are_unicode_identifiers`
- `test_v10_function_response_has_exactly_one_outcome`
- `test_v10_function_response_rejects_null_error`

Every positive and expected-error step in those cases runs through public Rust types.
The executable common-type suite rejects non-finite YAML values instead of converting them to strings.
Valid inputs must serialize to the same JSON value.
Invalid inputs must fail deserialization. A serialization failure does not satisfy a rejection expectation.
Detailed validation error paths are not asserted by these cases.

The pinned Core inventory reports 3 passed, 702 pending, and 165 legacy cases outside scope.
The test writes every case to `target/conformance/wire.jsonl` using the upstream executed-case manifest format.
Unsupported actions and definitions are recorded as skipped with an explicit pending reason.
Legacy cases are recorded as skipped with an outside-scope reason.
A supported case failure is recorded as failed and fails the test.

Run the Core harness with:

```sh
mise exec -- cargo test --package a2ui-rs --test upstream --locked -- --nocapture
```

The harness verifies the Git pin, clean checkout, and required reference paths before executing cases.
It inventories every case in all 29 `conformance/core/*.yaml` suites.

The wire tests also execute all 22 cases in the upstream `renderer_messages.json` and `function_response.json` specification fixtures.
These check valid and invalid renderer envelopes and preserve valid values through serialization.
They are separate from the Core YAML case counts.
Other specification fixtures and all stateful processing behavior remain pending.
The [wire API](wire.md) documents the current validation limits.

The audit uses a worker with a 16 MiB stack for deeply nested upstream fixtures.
The metadata parser permits nesting up to 256 levels to read legacy rejection fixtures.
These limits permit the same inventory check on Windows, Linux, and macOS.
Shared suites can contain legacy steps even when a case has no protocol version.
A future runner must classify those steps explicitly before executing the case.

## Upstream reference

`fixtures/upstream/` is a Git submodule of [a2ui-project/a2ui](https://github.com/a2ui-project/a2ui).
The parent repository records its exact commit. There is no separate revision or checksum manifest.

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
The presence of legacy files does not add legacy support to the SDK.
Upstream files retain their own license notices. Do not edit them inside this repository.

Git is required for the inventory test. The test reads local Git metadata and does not fetch from the network.
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
| Outside scope | A precise version or layer reason excludes the case |

Do not skip a case because it expects an error or uses strict validation.
Recursion, invalid topology, disposal, notifications, data scopes, and RPC routing remain part of the relevant contract.
Report partial coverage as partial coverage.
A conformance gate must fail when required v1.0 cases are pending or failed.
Inventory success must never become that gate.

## Updates

Dependabot checks the submodule weekly and opens a pull request for an updated pin.
The branch setting in `.gitmodules` selects the upstream branch for update proposals.
Normal initialization uses the recorded commit rather than the latest branch head.

For each update:

1. Review changes to the v1.0 specification, SDK blueprints, and relevant suites.
2. Initialize the proposed checkout with `git submodule update --init`.
3. Run the integration tests and implemented behavioral tests.
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
