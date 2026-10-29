# narrivy-asr-transcribe — transcribe.cpp engine binding

ASR binding for the transcribe.cpp GGUF runtime lane (CPU / iGPU / dGPU per
owner decision 2026-08-22). Implements `narrivy_asr_api::AsrEngine`.

## License and native boundary

The transcribe.cpp code license is MIT. The `runtime` CLI path links no native
library into Narrivy. The separate `resident-worker` feature exact-pins the MIT
`transcribe-cpp`/`transcribe-cpp-sys` 0.4.1 crates; native unsafe remains inside
those upstream dependencies and the process-isolated worker. Model licenses
are reviewed per artifact. Exact pins and limitations are in
[BUILD-NOTES.md](BUILD-NOTES.md).

## Feature worlds

| Build | Behavior | Streaming truth |
|---|---|---|
| default | typed `runtime_unavailable`; no native toolchain | unavailable |
| `runtime` | bounded versioned CLI sidecar | final-only |
| `resident-worker` | process-isolated model-resident CPU worker | model-attested native begin/feed/finalize; strict controlled composition only |

The CLI sidecar and current Whisper recovery model remain final-only. The
resident engine reports streaming only when the loaded model's native
capability bit is true. The strict recovery lane now composes bounded live
capture and a conservative energy-VAD/endpointer candidate through the daemon,
but remains `controlled_unqualified`. Native VAD quality/thresholds and
latency, daemon/client partial routing, and desktop partial UX remain blocked;
repeated final decode is never presented as streaming.

## Layout

- `src/lib.rs` — `TranscribeCppEngine`, both cfg worlds.
- `src/resident.rs` / `src/resident_protocol.rs` — bounded parent and private
  protocol-v4 resident lifecycle and immutable model binding.
- `src/bin/narrivy-transcribe-worker.rs` — isolated native 0.4.1 worker.
- `src/prep.rs` — pure audio preconditioning (`ensure_mono_16k`: mono +
  16 kHz linear resample + clamp), always compiled, unit-tested.
- `src/catalog.rs` — serde `GgufModelSpec` catalog for the GGUF variants in
  scope. Metadata only: no URLs (PORT-008) and no embedded weights.
- `src/sidecar.rs` — private file-based CLI rollback path with secure temporary
  storage, bounded cancellation, and no fabricated partials.

## Environment contract

| Variable | Effect |
|---|---|
| `TRANSCRIBE_CPP_BIN` | explicit `transcribe-cli` executable for `runtime` |
| `NARRIVY_TRANSCRIBE_WORKER` | explicit current-user-owned resident worker build artifact |
| `NARRIVY_TRANSCRIBE_WORKER_SHA256` | required exact SHA-256 for the controlled worker artifact |
| `NARRIVY_TRANSCRIBE_WORKER_SIZE` | required exact byte size for the controlled worker artifact |
| `NARRIVY_TRANSCRIBE_MODEL` | explicit GGUF source path consumed only during resident startup |
| `NARRIVY_TRANSCRIBE_MODEL_SHA256` | required exact SHA-256 for the resident model |
| `NARRIVY_TRANSCRIBE_MODEL_SIZE` | required exact resident-model byte size |

Resident startup opens the artifact without following a symlink, validates its
owner, safe mode, size, digest, and native ELF structure, and copies it into a
sealed executable snapshot. The engine reuses that immutable snapshot for all
worker generations; it does not reopen the pathname after startup. This proves
a controlled digest-bound build only. Installed-package provenance is not yet
composed and remains unavailable for release claims.

Resident model startup applies the same controlled identity principle to data:
it checks the explicit size/digest and safe source identity, copies the bytes
once into a mode-0400 sealed memfd, and passes only that inherited descriptor
to the worker. The exact worker independently verifies descriptor form, mode,
seals, size, digest and GGUF identity before native load. The daemon retains the
snapshot across worker recovery and never reopens the original model path.
This is still controlled-build identity, not model-manager installation or
package/SBOM provenance.
