# Recovery Phase 4H — Streaming recovery composition

**Recorded:** 2026-08-28  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled core/daemon composition implemented; native and release
qualification blocked; all 98 Linux requirements remain blocked

## Objective and scope

Compose Recovery 4G's bounded live PipeWire transport with Recovery 4E's
resident native-ASR stream while preserving the already-reviewed policy,
correction, durable intent, delivery, and terminal-journal behavior.

Scope is portable orchestration, strict daemon selection, truthful capability
reporting, controlled tests, architecture/ADR/state records, and read-only
validation. No microphone, transcript observation, evidence, requirement
status, manifest reseal, package, commit, push, or release action occurred.

## Implemented transaction

- `StreamingRecoverySlice` enforces raw-audio policy before opening either
  owner, admits the resident model, starts bounded capture, and publishes
  Listening only after actual capture readiness.
- Capture chunks must match the opened format, be nonempty and contiguous, and
  agree with the final sample-count and first-frame summary.
- ASR events require one matching Started event, strictly increasing Partial /
  Final sequences, correct finality flags, no premature Final, and exactly one
  final transcript.
- Partials are validation-only volatile values. They are not journaled,
  corrected, delivered, logged, or written as evidence.
- The final transcript enters the same `RecoveryTail` used by batch recovery:
  raw transcript policy, raw persistence, deterministic correction, durable
  delivery intent, corrected-text policy, exact delivery report validation,
  and terminal journal state remain one implementation.
- Any capture/ASR/lifecycle failure signals ASR immediately and attempts both
  capture abort/reap/join and ASR finish under one absolute two-second
  deadline. Cleanup failure takes precedence over the initiating error.
- `Cancelled` requires both the caller-owned token and the component's typed
  cancelled outcome. A standalone component `cancelled` code is a terminal
  failure, not proof of user cancellation.
- Strict resident lanes select streaming only for a loaded model that attests
  native streaming. Final-only resident models and the sidecar remain batch.
  A selected streaming failure has no batch/mock/delivery substitution.
- Status reports `streaming=composed:controlled_unqualified`; it does not claim
  ready, verified, native-qualified, or release-qualified behavior.

## Controlled validation

Core now passes 79 tests. New streaming cases cover shared-tail success,
volatile partial exclusion, capture failure, ASR failure, clean cancellation,
standalone component-cancellation-code rejection, cleanup-failure precedence
with both attempts, policy-before-owner admission, premature final, summary
mismatch, and lifecycle-observer rejection at Listening/FinalizingAudio.

| Command | Exit | Material result |
| --- | ---: | --- |
| targeted Rust format / `git diff --check` | 0 | pass |
| `cargo test -p narrivy-core --lib --locked` | 0 | 79 passed |
| `cargo test -p narrivyd --all-features --locked` | 0 | daemon and integration suites passed |
| default and all-feature daemon Clippy with `-D warnings` | 0 / 0 | no diagnostics |
| workspace check / all-target, all-feature tests | 0 / 0 | pass |
| repository-wide `cargo fmt --all -- --check` | 0 | pass |
| repository-wide Clippy with `-D warnings` | 0 | pass |
| desktop Vitest / TypeScript | 0 / 0 | 17 files, 24 tests; existing React `act(...)` warnings |
| doctor | 0 | current Linux/COSMIC/PipeWire toolchain observed |
| governance suite | 0 | 29 passed |
| architecture/source/dependency boundaries | 0 | pass |
| action pins / traceability | 0 | 5 pins; 136 mapped; all 98 Linux requirements blocked |
| production evidence | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| pack / release | 1 / 1 | dirty/unsealed; all requirements blocked; read-only |
| P11 gate | 2 | quarantined and non-writing |
| `cargo deny check` | 1 | 20 existing Tauri/GTK transitive advisory or unmaintained findings; no exception added |

An independent read-only agent first blocked 4H on cancellation provenance and
default-feature daemon warnings. After the corrections above, it reran the
focused recovery suite and default/resident-feature Clippy and returned PASS.

The governance window was non-mutating. Exact matching before/after hashes:

- Git status:
  `ca3a5313c35a562a67e9587bec31043766b74827566996eb802cf75440c9d6c0`;
- binary diff:
  `c36a928dfa0ce1cc5aa1d40fd673e2590a8ccc9630661e25549a05afa1441777`;
- tracked/unignored inventory/content (including explicit missing-file
  markers):
  `c797fd227825035a92f7042e62c799768bd8397200e316523807d6d5317bb6c2`.

This baseline and the final state record are written after that validator
window. Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Truth boundary and remaining work

This phase establishes a controlled executable composition, not native
qualification. Recovery 4F already showed that the exact model can emit native
updates, but 4H did not open a microphone or exercise the full daemon lane.
No latency, accuracy, continuity, delivery, or platform requirement passes.

Remaining work includes VAD/endpointer and segmentation ownership, partial IPC
and accessible UI, native real-time queue/latency/loss/accuracy/duplication and
cancellation qualification, device routing/hotplug recovery, RSS/thermal
behavior, core-dump/swap hardening, model-pack lifecycle, dependency advisory
resolution, clean-machine packaging, evidence, and release acceptance.

## Rollback and cleanup

Rollback is limited to the streaming core slice/shared tail, daemon selection
and capability strings, ADR-0038/ADR-0037/audio-boundary updates, decision /
research/state records, and this baseline. Accepted Phase 1–4G work and all
protected files must remain. The verified 4F user-cache model is intentional,
not trash. No temporary audio/transcript file, worker, daemon, recorder, model
staging file, evidence record, or package was created by this phase.
