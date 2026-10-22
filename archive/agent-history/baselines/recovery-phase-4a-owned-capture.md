# Recovery Phase 4A — Truthful Stop/Cancel and owned Linux capture

**Recorded:** 2026-08-28  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled implementation and validation complete; non-release;
all 98 Linux requirements remain blocked

## Objective and scope

Repair the strict Linux microphone recovery slice so `StopSession` finalizes
accepted audio while `CancelSession` aborts it, remove the capture WAV/tempfile
round trip, establish capture readiness from a real frame rather than process
spawn, and make the older PipeWire monitor own and boundedly stop its child and
parser. This is the bounded Recovery 4A decision in ADR-0032.

Permitted product scope was limited to portable session control/capture
contracts, portable recovery orchestration, Linux PipeWire capture ownership,
daemon strict-session composition/state timing, controlled tests, and this
ADR/baseline/state record. No ASR runtime replacement, streaming claim, VAD,
desktop change, evidence, requirement status, manifest, package, commit, or
push was permitted.

## Implemented behavior

- Portable `SessionControl` holds independent shared finalization and
  cancellation signals. Stop requests only finalization; Cancel requests
  cancellation. Capture checks cancellation on both sides of finalization so
  cancellation wins a concurrent control race.
- `AudioCapturePort` creates one owned `AudioCaptureSession`. `start` proves
  ownership/spawn; `wait_ready` requires one complete frame and retains it;
  `collect` returns in-memory audio plus content-free monotonic measurements.
- Strict capture invokes `pw-record` with explicit
  `@DEFAULT_AUDIO_SOURCE@`, raw 16 kHz mono f32 stdout, exact fractional frame
  budgets, null stdin/stderr, and a maximum frame count. It creates no audio or
  transcript file.
- The parent owns child/stdout synchronously. Fixed 16 KiB reads decode into a
  bounded preallocated vector with a 0–3-byte carry. Non-finite, overflow,
  partial, unexpected early-end, process, and cleanup failures are typed and
  fail closed.
- Stop kills/reaps and drains under one stored two-second cleanup deadline,
  then returns the prefetched/captured audio for policy and ASR. Cancel reaps
  and returns `cancelled`. Cleanup timeout is never hidden by a concurrent
  cancellation flag.
- Strict daemon sessions remain `Preparing` until core observes readiness;
  only then is `Listening` published/timed. `FinalizingAudio` occurs after
  capture collection. Source selection is reported as a dynamic selection
  expression, not fabricated hardware identity.
- The general PipeWire monitor now retains both parser `JoinHandle` and child,
  uses nonblocking batched reads, carries complete multichannel frames, stops
  and reaps under one caller deadline, and reports Awaiting/Streaming/Error/
  Finished truthfully. Non-finite/partial input, layout rejection, and
  sustained ring contention fail instead of silently dropping data.
- Core stage observers now return typed errors. Rejection stops before the
  related side effect, durably records `state_observation_failed`, and retains
  committed corrected text/intent as recoverable when delivery has not begun.
  Controlled regressions prove a rejected `Listening` never invokes ASR and a
  rejected `Delivering` never invokes the delivery adapter.
- The retained one-shot ASR sidecar no longer falls through from its two-second
  termination loop to an unbounded child `wait`. Cancellation, wait failure,
  and decode timeout either reap within the bound or return a typed cleanup
  failure.

## Independent design research and latency facts

The pinned transcribe.cpp v0.2.1 safe Rust wrapper supports shared resident
models, mutable sessions, direct 16 kHz mono f32 input, streaming APIs,
cancellation, device selection, and capability queries. Its published
concurrency contract serializes in-flight runs per model. whisper.cpp's
real-time example uses repeated inference and optional basic VAD; nerd-dictation
keeps its recognizer resident and distinguishes end from cancel. These were
used as design inputs only and are not Narrivy qualification.

A content-free local exploratory benchmark ran the currently configured
transcribe CLI on six seconds of isolated silence without printing text:

- five cold processes: 520.0, 426.5, 376.8, 416.8, and 356.9 ms;
- one process with five repetitions: 1335.9 ms total, 267.2 ms mean;
- output was observed only as a 4-byte count; the isolated temporary directory
  was removed.

This indicates meaningful avoidable process/model startup cost, but does not
qualify latency or justify a streaming claim. Recovery 4B must separately
decide and review the resident-model dependency/FFI/packaging boundary.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| targeted `rustfmt` / full `cargo fmt --all -- --check` / `git diff --check` | 0 | formatting and whitespace pass |
| strict Linux recovery tests | 0 | 28 passed |
| full Linux platform library | 0 | 194 passed, 1 attended native observation ignored |
| `cargo test -p narrivy-core --lib --locked --offline -q` | 0 | 70 passed, including observer-rejection barriers |
| `cargo test -p narrivy-asr-transcribe --features runtime --locked --offline -q` | 0 | 25 passed, including controlled sidecar cancellation/reaping |
| daemon strict readiness and Stop/Cancel regressions | 0 | both pass in main and duplicated integration harnesses |
| full narrivyd suite | 0 | 6 + 58 + 61 + 62 passed |
| forced strict Clippy for core/transcribe/daemon after follow-up repair | 0 | all targets/features, `-D warnings`; no diagnostics |
| `cargo check --workspace --all-targets --all-features --locked --offline` | 0 | workspace compiles |
| `cargo test --workspace --all-targets --all-features --locked --offline -q` | 0 | complete workspace passes; one attended portal observation ignored |
| governance suite | 0 | 29 passed |
| dependency/source boundary validators | 0 / 0 | 9 protected packages |
| `verify_traceability.py` | 0 | 136 mapped; 98 Linux requirements blocked |
| `verify_evidence.py --production` | 1 | 148 v1 quarantined; zero v2 candidates |
| `validate_pack.py` | 1 | deliberately dirty/unsealed manifest; read-only |
| `release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `run_gate.py --phase P11` | 2 | quarantined and non-writing |

The eight governance/boundary/release validators were measured immediately
before and after. They did not mutate the repository:

- Git status hash:
  `fd359d1021805de81a17da594e367fe9068cd981e3c4e9b4798943fd136fa54c`;
- binary diff hash:
  `3677b2a35d27d66c21a82589f92b31d53359dca8bc7f4624b0402e0adb61b1b5`;
- tracked/unignored workspace inventory/content hash:
  `34a52723e56cb0ef8aa0f40e2b0cb04230aaa706cdcf193d8a1f8ac78d84ed15`.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

No ordinary-log payload search found a new transcript/audio artifact. No
non-ignored `.tmp`, `.part`, `.wav`, or `.transcript` file was created by this
slice. Cargo outputs remain ignored build products and were not added to Git.

## Truth boundaries and next work

- No native microphone run occurred in Recovery 4A. Controlled subprocesses
  prove ownership, state, byte framing, cancellation, and cleanup—not real
  hardware behavior.
- Capture is incremental/fileless, but ASR is still a cold one-shot CLI
  sidecar after Stop/safety completion. There is no live partial transcript,
  VAD endpointing, resident model, or end-to-end latency qualification.
- The PipeWire default-source token is explicit but dynamic. Resolved hardware
  identity/device-change reporting is unimplemented.
- The general monitor's explicit `stop` retains ownership after timeout so a
  caller can retry. Its Drop path has only a bounded best-effort cleanup; a
  daemon-level orphan reaper policy remains required before release.
- Core/daemon state-observer error propagation and the sidecar's unbounded
  cleanup fallback are repaired and controlled-tested. Neither is release or
  native evidence.
- Supply-chain advisories, cross-target qualification, IPC privileged-command
  authorization, durable state location, recovery-vs-History separation,
  desktop truth/UX, native portal/delivery qualification, packaging, evidence,
  and every release requirement remain blocked.

## Rollback

Rollback is limited to the SessionControl/capture contract additions, portable
recovery orchestration changes, strict Linux raw capture, PipeWire monitor
lifecycle repair, daemon Stop/Cancel/readiness composition, ADR-0032, this
baseline, and the corresponding state record. Accepted Recovery 1–3B work and
all protected files must be preserved.
