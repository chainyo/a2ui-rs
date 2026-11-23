# Fixtures

`local/` contains small project-owned message examples.
The contact example contains a create message and a delete message.
It also contains component and data updates.

`upstream/` is the official A2UI repository as a pinned Git submodule.
Initialize it with `git submodule update --init` before running tests.
Read [the conformance policy](../docs/conformance.md) before updating the pin.
Do not edit files inside the upstream checkout.

Do not add credentials, personal records, private endpoints, or production payloads.
