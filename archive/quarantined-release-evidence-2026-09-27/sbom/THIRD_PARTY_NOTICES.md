# Third-Party Notices for Narrivy

This file tracks licenses for bundled third-party components. Narrivy itself is MIT (see `LICENSE`).

## Rust dependencies (Cargo)
- No external crates with copyleft are bundled in Linux 1.0 foundation (P0). Workspace crates are `publish=false` until release. Allowed licenses per `deny.toml`: `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `MIT`, `MPL-2.0`, `Unicode-3.0`, `Zlib`. Verified via `cargo deny check` (advisories/bans/licenses/sources OK).

- Build tooling: `cargo-deny 0.20.2` (Apache-2.0/MIT), `rustc 1.97.1` (MIT/Apache-2.0).

## Models (P5 onward, catalog `spec/schemas/model-manifest.schema.json`)
- `openai/whisper-large-v3` — MIT (upstream). Converted OpenVINO IR `narrivy/whisper-large-v3-*` retains upstream license notice.
- `nvidia/parakeet-tdt-1.1b` — CC-BY-4.0 (NVIDIA). Requires attribution.
- `Qwen/Qwen3-8B` — Qwen License (Alibaba). Verify redistribution before bundling.

Each `model-manifest` records `license.spdx_or_name`, `source`, `redistribution` and is verified before download. Compiled caches are not redistributed.

## System / packaging
- Linux system libraries (PipeWire, Wayland) via dynamic linking — LGPL/MIT, not bundled.
- No third-party code is vendored without license verification. See `cargo deny` and `BUILD-PACK-MANIFEST.json` for provenance.

Generated 2026-08-21, updated each release. See `agent/RESOLVED_VERSIONS.json` for exact toolchain versions.
