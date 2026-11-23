# Conformance

## Test procedure

Initialize the upstream submodule before you run the tests.
The Core harness uses the public Rust APIs for supported upstream cases.
The lifecycle cases send decoded messages to `SurfaceStore::apply`.
Catalog cases load schemas with `Catalog::from_json`.
Validation cases send decoded messages to `MessageProcessor::apply_with_config`.
Parser cases send templates to `expression::parse_template`.
Resolution cases use `MessageProcessor` to read resolved properties and check function calls.
The pointer and deletion suites apply message payloads to one data model and compare the final JSON.
It compares the JSON value before and after serialization for each valid input.
An invalid input must fail deserialization.

Run the Core harness with:

```sh
mise exec -- cargo test --package a2ui-rs --test upstream --locked -- --nocapture
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
| Outside scope | A precise version or layer reason excludes the case |

Do not skip a case because it expects an error or uses strict validation.
Recursion, invalid topology, disposal, notifications, data scopes, and RPC routing remain part of the relevant contract.
Report partial coverage as partial coverage.
A conformance gate must fail when required v1.0 cases are pending or failed.
Inventory success must never become that gate.

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
