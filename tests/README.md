# Test Layout

- `contracts/` — portable platform/ASR/pipeline/IPC contracts
- `integration/` — daemon/core controlled-adapter integration
- `e2e/` — full application scenarios; clearly tag real-environment tests
- `failure/` — fault injection/recovery
- `performance/` — reproducible latency/resource benchmarks
- `accessibility/` — automated + manual evidence harnesses
- `schema/` — manifest/mode/profile/privacy validation and malicious-input cases
- `fixtures/` — non-sensitive redistributable fixtures only

Tests that require real desktop/hardware must record environment metadata and may be run in a qualification lab/host, but they are still automated/scripted as far as practical. Do not silently skip them and claim support.
