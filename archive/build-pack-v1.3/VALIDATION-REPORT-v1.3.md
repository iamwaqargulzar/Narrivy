# Narrivy v1.3 Scaffold Validation Report

> **Historical snapshot (sealed at the v1.3 scaffold checkpoint).** Catalog
> counts below reflect the original 18-engine/56-module taxonomy; on
> 2026-08-22 the catalogs grew to 27 engines / 68 bindings after the verified
> research sweep (see `agent/RESEARCH_LOG.md`). Current counts live in
> `docs/00-ARCHITECTURE-MAP.md` and are enforced by CI.

This report covers the v1.3 autonomous scaffold/build pack. It does **not** claim that Narrivy is implemented. P0F and later native qualification remain fail-closed until executed on the required environments.

## Canonical mapping

- 136 normative requirements: 98 Linux 1.0 release-gated + 38 roadmap.
- 20 feature groups with exactly one owner for every normative requirement.
- 12 built-in/reserved Pipeline recipes.
- Engine families per current catalogs; OpenVINO ASR is admitted per qualified device/model tuple, with transcribe.cpp GGUF lanes as the proven fallback.
- 56 Module bindings across 31 capability slots.
- 8 Runtime families.
- ASR lanes: OpenVINO 2026.4 (Qwen3-ASR CPU/GPU supported upstream; NPU candidate-only) plus transcribe.cpp GGUF lanes.
- Canonical P2/P3/P6 names and filenames.
- No shipped legacy build-contract references; superseded planning artifacts are intentionally omitted from the v1.3 distribution.

## Static/control-plane checks

- `verify_architecture_map.py`: PASS.
- `verify_traceability.py`: PASS.
- `verify_phase_model.py`: PASS.
- `validate_pack.py`: PASS after manifest reseal.
- `amendment_gate.py`: 67 checks passed, 0 failed.
- Manifest verifies every shipped non-excluded file by size and SHA-256.

## Deliberately not claimed

Rust compilation, native COSMIC/GNOME/PipeWire feasibility, accelerator qualification and final release qualification require the appropriate external toolchain/hardware/session. Those checks must fail closed or remain unclaimed until genuinely executed by the coding/build agent.
