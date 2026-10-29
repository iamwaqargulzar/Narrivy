# Third-Party Notices for Narrivy

This file tracks licenses for bundled third-party components. Narrivy itself is MIT (see `LICENSE`).

What the Linux `.deb` ships (`scripts/build_deb_installable.py`): the
`narrivyd`, `narrivy-cli` and `narrivy-desktop` binaries, the resident ASR
worker `/usr/lib/narrivy/narrivy-transcribe-worker`, the speech model
`/usr/share/narrivy/models/whisper-tiny.en-Q8_0.gguf`, the desktop front end
(compiled into `narrivy-desktop`), icons, the systemd user unit and this file.
System libraries are package dependencies (`Depends:`), not shipped files. The
candidate SBOM (`spdx.json`) lists every locked crate and every shipped npm
package with its declared SPDX licence.

## Rust dependencies (Cargo)
- No external crates with copyleft are bundled in Linux 1.0 foundation (P0). Workspace crates are `publish=false` until release. Allowed licenses per `deny.toml`: `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `MIT`, `MPL-2.0`, `Unicode-3.0`, `Zlib`. License, ban, source and advisory checks pass. The Tauri 2 desktop host (ADR-0075) brings in unmaintained GTK3-rs, `proc-macro-error` and `unic-*` crates; those notices are listed by ID in `deny.toml` with reasons and accepted by the owner (D22). No vulnerability advisory is ignored.

- Build tooling: `cargo-deny 0.20.2` (Apache-2.0/MIT), `rustc 1.97.1` (MIT/Apache-2.0).
- Portal/EI protocol: `reis 0.7.1` (MIT) is exact-pinned with optional
  features disabled; Narrivy uses only its low-level typed wire codec under
  ADR-0026. `rustix 1.1.4` (Apache-2.0 OR MIT) provides safe polling and
  monotonic time. Neither component is treated as native delivery evidence.
- Desktop host: `tauri 2` and its plugins (Apache-2.0 OR MIT), including
  `tauri-plugin-single-instance`. The Linux overlay uses `gtk 0.18` and
  `gtk-layer-shell 0.8` (MIT). These Rust crates are compiled into
  `narrivy-desktop`. The system GTK, WebKitGTK and gtk-layer-shell libraries
  they bind are dynamically linked package dependencies and are not shipped.
- Resident ASR worker: exact `transcribe-cpp 0.4.1` and
  `transcribe-cpp-sys 0.4.1` (MIT) are optional, default-features-disabled
  dependencies. The sys crate compiles its bundled transcribe.cpp/ggml C++
  sources as a static CPU library through `cmake 0.1.58`; Narrivy isolates that
  native code in `narrivy-transcribe-worker`, which the `.deb` ships. Crates.io
  checksums and the full transitive graph are locked in `Cargo.lock`. The
  `.deb` also ships one model, `whisper-tiny.en-Q8_0` (see Models below).
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

## Models

Licences below are the ones the repository records
(`crates/narrivy-model-manager/seed/registry.json`,
`tools/reference-models/packs.json`, `engines/transcribe-cpp/BUILD-NOTES.md`).
Open licence questions are tracked in `agent/BLOCKERS.md` (B-LEGAL-016).

Shipped in the `.deb`:
- `whisper-tiny.en-Q8_0` (OpenAI Whisper tiny.en, GGUF conversion by
  handy-computer) — Apache-2.0. Source
  `huggingface.co/handy-computer/whisper-tiny.en-gguf` at revision
  `becb8bcb804405dc97b380a523d9975888820986`, sha256
  `e8c9b73c06344307d8b346e07fbe93dd88d894627854bcff31523f1ce44394fa`.
  Shipped unmodified.

Downloaded on request, not shipped:
- `whisper-base.en-Q8_0`, `whisper-small.en-Q8_0`,
  `whisper-large-v3-turbo-Q8_0`, `whisper-large-v3-Q5_K_M` (OpenAI Whisper,
  GGUF conversion by handy-computer) — Apache-2.0.
- `parakeet-tdt-0.6b-v3-Q8_0`, `parakeet-unified-en-0.6b-Q8_0` (NVIDIA
  Parakeet, GGUF conversion by handy-computer) — CC-BY-4.0. Attribution is
  required.
- OpenVINO reference packs: `whisper-large-v3-turbo-openvino-int8` — MIT;
  `whisper-large-v3-openvino-int8` — Apache-2.0 (conversion by Intel
  OpenVINO).

Each model manifest records `license.spdx_or_name`, `source` and
`redistribution`, and is verified before download. Compiled caches are not
redistributed.

## System / packaging
- Linux system libraries (PipeWire, Wayland, OpenBLAS, Vulkan loader) via
  dynamic linking — package dependencies, not shipped in the `.deb`.
- No third-party code is vendored without license verification. See `cargo deny` and `BUILD-PACK-MANIFEST.json` for provenance.

Generated 2026-08-21, updated each release (last: 2026-10-29). See `agent/RESOLVED_VERSIONS.json` for exact toolchain versions.
