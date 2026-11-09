# Third-Party Notices for Narrivy

This file tracks licenses for bundled third-party components. Narrivy itself is MIT (see `LICENSE`).

What the Linux `.deb` ships (`scripts/build_deb_installable.py`): the
`narrivyd`, `narrivy-cli` and `narrivy-desktop` binaries, the resident ASR
worker `/usr/lib/narrivy/narrivy-transcribe-worker`, the desktop front end
(compiled into `narrivy-desktop`), icons, the systemd user unit and this file. No model
ships in the package (D40).
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
- Wayland virtual keyboard: `wayland-protocols-misc 0.3.12` (MIT) is
  exact-pinned with default features disabled and only `client` on; Narrivy
  uses its `zwp_virtual_keyboard_v1` bindings to type text in process.
- Desktop host: `tauri 2` and its plugins (Apache-2.0 OR MIT), including
  `tauri-plugin-single-instance`. The Linux overlay uses `gtk 0.18` and
  `gtk-layer-shell 0.8` (MIT). These Rust crates are compiled into
  `narrivy-desktop`. The system GTK, WebKitGTK and gtk-layer-shell libraries
  they bind are dynamically linked package dependencies and are not shipped.
- Resident ASR worker: exact `transcribe-cpp 0.4.1` and
  `transcribe-cpp-sys 0.4.1` (MIT) are optional, default-features-disabled
  dependencies. The sys crate compiles its bundled transcribe.cpp/ggml C++
  sources as a static library (CPU and Vulkan) through `cmake 0.1.58`; Narrivy
  isolates that native code in `narrivy-transcribe-worker`, which the `.deb`
  ships. The sys crate is a patched copy in `vendor/transcribe-cpp-sys`
  (upstream LICENSE and THIRD-PARTY-LICENSES.md kept; the changes are listed in
  its NARRIVY-PATCH.md). Crates.io checksums and the full transitive graph are
  locked in `Cargo.lock`.
- Windows "Ignore this computer's sound": exact `sonora 0.3.0` and its
  `sonora-*` crates (BSD-3-Clause), a pure-Rust port of WebRTC audio
  processing (echo canceller, noise suppression, automatic gain), compiled
  into `narrivyd` on Windows only. Copyright (c) 2011, The WebRTC Project
  Authors; (c) 2016, Arun Raghavan and contributors; (c) 2026,
  dignifiedquire. All rights reserved. Redistribution in binary form keeps
  this notice, the licence's conditions and its disclaimer: the full text
  is `packaging/licenses/sonora/LICENSE`, installed as
  `licenses\sonora\LICENSE` in the Windows install folder.
- Writing checker (D26, SQ-WRITE): exact `harper-core 2.13.0` (Apache-2.0,
  Automattic) with default features off except `concurrent`, used as a
  library in narrivyd. Its dependency `tiny-keccak 2.0.2` is CC0-1.0 (public
  domain), allowed for that crate only in `deny.toml`.
- Voice mode's worker (`narrivy-voice-worker`, D34): exact `sherpa-onnx 2.0.1`
  and `sherpa-onnx-sys 2.0.1` (Apache-2.0, k2-fsa), linked statically with
  the prebuilt sherpa-onnx library and ONNX Runtime (MIT) it downloads at
  build time. Version 2 contains no eSpeak NG (GPL-3.0); English is read from
  the voice's own word list. On Windows it records and plays through `cpal
  0.18.2` (Apache-2.0).

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

No model ships in the `.deb` (D32, D40). Downloaded on request:
- `whisper-tiny.en-Q8_0` (OpenAI Whisper tiny.en, GGUF conversion by
  handy-computer) — Apache-2.0. Source
  `huggingface.co/handy-computer/whisper-tiny.en-gguf` at revision
  `becb8bcb804405dc97b380a523d9975888820986`, sha256
  `e8c9b73c06344307d8b346e07fbe93dd88d894627854bcff31523f1ce44394fa`.
- `whisper-base.en-Q8_0`, `whisper-small.en-Q8_0`,
  `whisper-large-v3-turbo-Q8_0`, `whisper-large-v3-Q5_K_M` (OpenAI Whisper,
  GGUF conversion by handy-computer) — Apache-2.0.
- `parakeet-tdt-0.6b-v3-Q8_0`, `parakeet-unified-en-0.6b-Q8_0` (NVIDIA
  Parakeet, GGUF conversion by handy-computer) — CC-BY-4.0. Attribution is
  required.
- OpenVINO reference packs: `whisper-large-v3-turbo-openvino-int8` — MIT;
  `whisper-large-v3-openvino-int8` — Apache-2.0 (conversion by Intel
  OpenVINO).
- Voice mode's detector, shipped in the package: Silero VAD
  (`silero_vad.onnx`, MIT, Silero Team), checked against its SHA-256.
- Voice mode's voices, Store downloads: Kokoro-82M v1.0 as converted by
  sherpa-onnx (`voice-kokoro-v1.0-en`) — Apache-2.0; Piper's Urdu voice
  `ur_PK-fasih-medium` (`voice-piper-ur-fasih-medium`) — MIT, with the
  eSpeak NG data it needs for Urdu — GPL-3.0-or-later.

Each model manifest records `license.spdx_or_name`, `source` and
`redistribution`, and is verified before download. Compiled caches are not
redistributed.

## System / packaging
- Linux system libraries (PipeWire, Wayland, OpenBLAS, Vulkan loader) via
  dynamic linking — package dependencies, not shipped in the `.deb`.
- The Windows OpenVINO runtime pack (a download, not in the MSI) carries
  Microsoft's Visual C++ runtime DLLs (`msvcp140`, `msvcp140_codecvt_ids`,
  `vcruntime140`, `vcruntime140_1`) under Microsoft's redistributable terms,
  not an open-source licence (D49); the pack's `licenses/` says so.
- No third-party code is vendored without license verification. `cargo deny check` (`deny.toml`) enforces the licence allow-list.

Generated 2026-08-21, updated each release (last: 2026-10-29). Exact dependency versions are in `Cargo.lock` and `apps/narrivy-desktop/package-lock.json`.

## GPL program: the Urdu speaker (D51)
- `narrivy-urdu-speaker` (`engines/voice-urdu`) is a separate program,
  licensed GPL-3.0-or-later, that the voice worker starts and talks to over
  a pipe. It links sherpa-onnx 1.13.8 (Apache-2.0) statically, which
  contains eSpeak NG (GPL-3.0-or-later, https://github.com/espeak-ng/espeak-ng),
  piper-phonemize (MIT) and ONNX Runtime (MIT). Its licence and source note
  ship beside it (`/usr/share/doc/narrivy/narrivy-urdu-speaker/` on Linux,
  `licenses\narrivy-urdu-speaker\` in the Windows install folder). No other
  Narrivy program links GPL code.
