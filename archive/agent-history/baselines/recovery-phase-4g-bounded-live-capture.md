# Recovery Phase 4G — Bounded live capture transport

**Recorded:** 2026-08-28  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** portable/Linux adapter boundary implemented and controlled-tested;
non-release; all 98 Linux requirements remain blocked

## Objective and scope

Create the continuity-preserving, bounded PipeWire-to-ASR transport missing between the
native streaming canary and product orchestration. Scope is limited to portable
streaming-capture contracts, the Linux recovery adapter, controlled lifecycle
tests, correction of an overclaiming audio document, ADR/state records, and
validation.

No microphone was opened. Core/daemon ASR composition, VAD/endpointer, pre-roll
activation, device lifecycle, partial UI, evidence, requirement state, manifest,
package, commit, push, and release behavior are excluded.

## Implemented boundary

- Separate batch and streaming capture factories prevent buffered one-shot
  capture from being mislabeled as live streaming.
- Streaming startup binds the transaction `SessionControl` into the producer.
- A non-real-time parser emits contiguous 1,280-sample/80-ms 16 kHz mono f32
  chunks into an eight-slot `sync_channel` (40 KiB queued payload, 640 ms).
- Queue full is terminal `capture_backpressure`; saturation is never silently
  accepted as a shorter stream.
- Producer-side Stop/Cancel observation is independent of synchronous ASR feed
  duration. Stop emits only already-accepted partial PCM; Cancel discards it.
- The parent owns child reap and parser join. End follows both. Downstream
  failure has an explicit abort using the caller's existing absolute deadline.
- Sequence, layout, finite sample, total-budget, early EOF, and completion
  invariants fail closed with content-free typed codes.

## Controlled validation

The focused recovery suite passed 41 tests. New cases cover exact 16,000-sample
safety completion across 13 ordered chunks, retained readiness, bounded
finalization, explicit abort, deterministic queue overflow, producer-side
Stop/Cancel without consumer polling, cancellation after receive, and child /
parser cleanup. The full Linux adapter passed 207 tests with one attended
portal observation ignored. No native microphone or ASR content was used.

| Command | Exit | Material result |
| --- | ---: | --- |
| targeted Rust format / `git diff --check` | 0 | pass |
| focused recovery tests | 0 | 41 passed |
| full Linux adapter tests | 0 | 207 passed; one attended portal test ignored |
| strict platform/API Clippy | 0 | no diagnostics |
| workspace check / all-target, all-feature tests | 0 / 0 | pass |
| transcribe.cpp all-feature tests | 0 | 44 passed, including exact canary pin |
| doctor | 0 | current Linux/COSMIC/PipeWire toolchain observed |
| governance suite | 0 | 29 passed |
| architecture/source/dependency boundaries | 0 | pass |
| action pins / traceability | 0 | 5 pins; 136 mapped; all 98 Linux requirements blocked |
| production evidence | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| pack / release | 1 / 1 | dirty/unsealed; all requirements blocked; read-only |
| P11 gate | 2 | quarantined and non-writing |

The governance window was non-mutating. Exact matching before/after hashes:

- Git status:
  `60dc6a6a1a73ac64102ede232fa710a558b19a209a93600919bee6e3fa9388d7`;
- binary diff:
  `a339e5578cfb9664420b488b259550a2fb01a3b4c7501fbbd8e0ed204db297d3`;
- tracked/unignored inventory/content:
  `150b40350fd7af32f6d66b9c181fcafd3ea80323c06002222320e8dd3d6d4a1a`.

This baseline table was updated after that measured window. Protected
identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

An independent read-only re-review returned PASS after three rework rounds:
queued pre-poll audio is preserved on Stop; child/parser cleanup shares one
deadline and attempts both owners; and every producer fault stops the recorder
before the parent reaps it. No release evidence is created from these
controlled tests.

## Truth boundary and remaining work

Recovery 4G proves an adapter transport contract only. The daemon does not use
it yet, status must remain `streaming=disabled:not_composed`, and no latency,
accuracy, loss, native-device, or delivery requirement passes.

Next work is a separate composition phase: capture start/read/abort must be
owned in the same recovery transaction as resident ASR begin/feed/finalize;
only the final ASR snapshot may enter privacy, deterministic correction,
journal, and delivery. Every ASR failure must abort/reap capture and every
capture failure must cancel/reset the ASR stream.

## Rollback and cleanup

Rollback is limited to the portable streaming-capture types/traits, Linux
bounded stream implementation/tests, ADR-0037, the corrected audio-boundary
document, state/decision/research records, and this baseline. The verified 4F
model cache is intentionally retained. No temporary audio/transcript file,
controlled child, parser, daemon, recorder, ASR worker, package, or evidence
artifact is created or retained by this phase.
