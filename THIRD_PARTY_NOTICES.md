# Third-Party Notices for Narrivy

This file tracks licenses for bundled third-party components. Narrivy itself is MIT (see `LICENSE`).

## Rust dependencies (Cargo)
- No external crates with copyleft are bundled in Linux 1.0 foundation (P0). Workspace crates are `publish=false` until release. Allowed licenses per `deny.toml`: `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `MIT`, `MPL-2.0`, `Unicode-3.0`, `Zlib`. License, ban, source and advisory checks pass. The Tauri 2 desktop host (ADR-0075) brings in unmaintained GTK3-rs, `proc-macro-error` and `unic-*` crates; those notices are listed by ID in `deny.toml` with reasons and accepted by the owner (D22). No vulnerability advisory is ignored.

- Build tooling: `cargo-deny 0.20.2` (Apache-2.0/MIT), `rustc 1.97.1` (MIT/Apache-2.0).
- Portal/EI protocol: `reis 0.7.1` (MIT) is exact-pinned with optional
  features disabled; Narrivy uses only its low-level typed wire codec under
  ADR-0026. `rustix 1.1.4` (Apache-2.0 OR MIT) provides safe polling and
  monotonic time. Neither component is treated as native delivery evidence.
- Desktop host: `tauri 2` and its plugins (Apache-2.0 OR MIT), including
  `tauri-plugin-single-instance`. The Linux overlay uses `gtk 0.18` and
  `gtk-layer-shell 0.8` (MIT). The system GTK/WebKitGTK/gtk-layer-shell
  libraries remain dynamically linked and are not bundled.
- Resident ASR worker: exact `transcribe-cpp 0.4.1` and
  `transcribe-cpp-sys 0.4.1` (MIT) are optional, default-features-disabled
  dependencies. The sys crate compiles its bundled transcribe.cpp/ggml C++
  sources as a static CPU library through `cmake 0.1.58`; Narrivy isolates that
  native code in `narrivy-transcribe-worker`. Crates.io checksums and the full
  transitive graph are locked in `Cargo.lock`. The model is user-cache data and
  is not bundled by this change.
- Writing checker (D26, SQ-WRITE): exact `harper-core 2.13.0` (Apache-2.0,
  Automattic) with default features off except `concurrent`, used as a
  library in narrivyd. Its dependency `tiny-keccak 2.0.2` is CC0-1.0 (public
  domain), allowed for that crate only in `deny.toml`.

## Desktop frontend (npm)
- `react` and `react-dom` 19 (MIT), `@tauri-apps/api` 2 (Apache-2.0 OR MIT).
- Fonts bundled into the desktop app: Inter (Rasmus Andersson) and
  Instrument Serif (Instrument), via `@fontsource-variable/inter` and
  `@fontsource/instrument-serif` 5.x, under the SIL Open Font License 1.1
  (OFL-1.1). The fonts ship unmodified.

## Models (P5 onward, catalog `spec/schemas/model-manifest.schema.json`)
- `openai/whisper-large-v3` — MIT (upstream). Converted OpenVINO IR `narrivy/whisper-large-v3-*` retains upstream license notice.
- `nvidia/parakeet-tdt-1.1b` — CC-BY-4.0 (NVIDIA). Requires attribution.
- `Qwen/Qwen3-8B` — Qwen License (Alibaba). Verify redistribution before bundling.

Each `model-manifest` records `license.spdx_or_name`, `source`, `redistribution` and is verified before download. Compiled caches are not redistributed.

## System / packaging
- Linux system libraries (PipeWire, Wayland) via dynamic linking — LGPL/MIT, not bundled.
- No third-party code is vendored without license verification. See `cargo deny` and `BUILD-PACK-MANIFEST.json` for provenance.

Generated 2026-08-21, updated each release. See `agent/RESOLVED_VERSIONS.json` for exact toolchain versions.
