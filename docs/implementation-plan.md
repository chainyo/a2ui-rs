# Implementation plan

The bootstrap establishes repository checks and a public version boundary.
Each later step must use the pinned blueprint and upstream cases for its behavior contract.

1. Add typed v1.0 messages, catalog models, and canonical serialization.
   Include positive and negative schema cases.
2. Add data models and scoped JSON Pointer operations.
   Test replacement, deletion, notification, and disposal behavior.
3. Add surface and component state with message processing.
   Test complete create, update, and delete sequences through the public API.
4. Add catalog validation, topology checks, and reference limits.
   Include strict validation and expected-error cases.
5. Add binding and expression resolution.
   Include collections, relative scopes, and local function validation.
6. Add v1.0 bidirectional RPC behavior.
   Test correlation, permissions, cancellation, timeout, and disposal boundaries.
7. Complete the Core conformance runner and its executed-case manifest.
   Connect each action as its public implementation becomes available.
8. Review package names, public APIs, documentation, and release policy.
   Keep publication disabled until this review is complete.
9. Implement Agent helpers or framework adapters in separate crates when requested.

Do not wait until step 7 to run available upstream cases.
Add each behavioral test when its production API is implemented.
A GPUI adapter must consume the same Core SDK as other clients.
