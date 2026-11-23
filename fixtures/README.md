# Fixtures

`local/` contains small project-owned message examples.
The contact example covers create, component update, data update, and surface deletion.
The bootstrap reads only its version fields. It does not process its lifecycle.

`upstream/` is the official A2UI repository as a pinned Git submodule.
Initialize it with `git submodule update --init` before running tests.
Read [the conformance policy](../docs/conformance.md) before updating the pin.
Do not edit files inside the upstream checkout.

Do not add credentials, personal records, private endpoints, or production payloads.
