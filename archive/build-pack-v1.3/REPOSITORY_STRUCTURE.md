# Repository Structure

- `apps/` — desktop/CLI clients; clients render/use application APIs and do not own domain policy.
- `services/narrivyd/` — long-running daemon composition/lifecycle.
- `crates/narrivy-core` — portable domain orchestration.
- `crates/narrivy-*-api` — portable capability/protocol contracts.
- `crates/narrivy-types`, `pipeline`, `policy`, `diagnostics`, `ipc` — portable shared layers.
- `adapters/platform-*` — OS/desktop implementation boundaries.
- `engines/asr-*` — ASR/runtime-specific adapters.
- `spec/` — normative schemas, examples, acceptance specs and requirement traceability.
- `docs/` — product/architecture/UX/platform/ML/security/testing/operations/phase/release specifications.
- `agent/` — resumable autonomous state, decisions, blockers, research, reports and metrics.
- `scripts/` — doctor, validation, architecture, phase and release gates.
- `tests/` — contract/integration/E2E/failure/performance/accessibility/schema harness locations.
- `tools/` — reproducible conversion/reference-model/build helper tooling.
- `packaging/` — Linux release packaging.

Dependency direction is enforced by architecture checks; portable crates do not import Tauri/PipeWire/Wayland/OpenVINO or concrete OS/runtime implementations.
