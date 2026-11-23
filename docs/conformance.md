# Conformance

## Current coverage

**No upstream behavioral conformance case runs against the Core SDK yet.**
The bootstrap tests only the public protocol version type.

`cargo xtask upstream` performs two repository checks:

1. Verify each imported file against its recorded SHA-256 checksum.
2. List every case in the imported Core YAML suites.

The command labels cases as pending or outside scope for an explicitly declared legacy protocol.
Its success means the reference files are intact and readable.
It does not mean the SDK implements their behavior.
Shared suites can contain legacy steps even when a case has no protocol version.
A future runner must classify those steps explicitly before executing the case.

## Reference inventory

`fixtures/upstream/manifest.json` pins the upstream Git revision and the exact file inventory.
Imported files preserve upstream bytes and license notices.
The reference set contains:

- The Core, Agent, and Framework Adapter blueprints.
- The v1.0 protocol JSON schemas.
- The v1 Basic Catalog and two realistic examples.
- The shared Core YAML suites and the v1.0 processor and validator suites.
- The upstream conformance guide and manifest schemas.

Dedicated v0.8 and v0.9 processor and validator suites are not imported.
Legacy content inside shared suites remains unchanged for attribution and review.
It does not add legacy support to the SDK.
This is a reference subset. Additional dependencies may be needed before a case can run.
The manifest does not claim to contain the complete upstream harness.

## Execution contract

Implement each case against the same public Core SDK used by applications.
Do not add an alternative test-only state engine.

A future runner must produce an executed-case manifest with these results:

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

## Update procedure

1. Select an upstream commit by its full SHA.
2. Review the specification and blueprint changes.
3. Retrieve the same reference paths from that commit without modifying their contents.
4. Add required reference dependencies when the suite introduces new paths.
5. Update the revision, file inventory, and SHA-256 checksums together.
6. Review added, removed, renamed, and changed cases.
7. Run `cargo xtask upstream` and the implemented behavioral tests.
8. State the coverage change in the pull request.

Do not refresh upstream references as an unrelated dependency update.
