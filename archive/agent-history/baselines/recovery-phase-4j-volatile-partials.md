# Recovery Phase 4J — Volatile partial presentation

**Recorded:** 2026-08-29  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled core-to-desktop partial path implemented; native model,
microphone, repaint/latency and release qualification blocked; all 98 Linux
requirements remain blocked

## Objective and scope

Expose native-model partial hypotheses through the existing strict streaming
composition without converting tentative text into journal, history,
correction, delivery, logs, evidence, or a release claim.

Scope is the optional portable observer, bounded daemon routing, additive IPC
event/transport authorization and ordering, Tauri forwarding, accessible
memory-only desktop presentation, controlled tests, ADR/protocol/catalog/state
records, read-only validation, and exact generated-cache cleanup. No microphone,
native observation, evidence/index, requirement status, manifest reseal,
packaging, commit, push, or release action occurred.

## Implemented boundaries

- Core validates ASR event order, non-empty text, a 16-KiB UTF-8 bound, and
  local raw-transcript policy before invoking an optional transaction-scoped
  observer. Only the final transcript enters the existing shared tail.
- Observer rejection is a typed terminal failure and reuses the one-deadline
  capture/ASR cleanup. Controlled observation proves the journal has no raw,
  corrected, delivered, intent, or report representation while partials flow.
- The daemon attaches session, transaction and pipeline identity. Latest
  partials coalesce per transaction in the bounded queue. Lifecycle events
  evict partials first; incoming partials cannot evict an all-lifecycle queue.
- `Debug` redacts partial text at daemon and IPC layers; `Envelope` retains its
  payload redaction. Ordinary log sites were inspected and receive only typed,
  content-free errors/identities.
- IPC protocol minor 3 adds a bounded `transcript_partial` event. Handshake
  authentication does not subscribe a client: an accepted explicit Subscribe
  is required, Unsubscribe removes eligibility, and each event write has a
  50-ms bound before the stalled client is disconnected.
- Per-client minor filtering prevents minor-2 subscribers from receiving the
  new variant while compatible lifecycle events continue.
- The Unix client requires a present, strictly increasing envelope event
  sequence and clears buffered traffic across reconnect epochs. Tauri preserves
  this sequence instead of discarding the envelope ordering field.
- React accepts partials only for fresh global and ASR sequence plus exact
  session/transaction/pipeline identity. Tentative text is memory-only,
  separate from final history, cleared on terminal/new-session boundaries, and
  rendered in a polite atomic live region.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| focused core | 0 | 90 passed |
| focused Unix IPC | 0 | 26 passed, including explicit subscription, minor filtering and sequence rejection |
| focused daemon | 0 | library 8; daemon 60; socket E2E 63; extension 64 passed |
| Tauri all-target/all-feature check | 0 | pass |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | pass |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | complete pass; one attended portal observation remains ignored |
| workspace all-target/all-feature Clippy with `-D warnings` | 0 | no diagnostics |
| `cargo fmt --all -- --check` / `git diff --check` | 0 / 0 | pass |
| desktop Vitest / TypeScript | 0 / 0 | 17 files, 25 tests; pre-existing React `act(...)` warnings |
| doctor | 0 | current Linux/COSMIC/PipeWire toolchain observed; no permission granted |
| governance suite | 0 | 12 tests plus 32 parameterized subtests passed |
| dependency/source boundaries | 0 / 0 | 9 protected packages |
| action pins / traceability | 0 / 0 | 5 pins; 136 mapped; all 98 Linux requirements blocked |
| production evidence | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| pack / release | 1 / 1 | dirty/unsealed; all requirements blocked; read-only |
| P11 gate | 2 | quarantined and non-writing |

`cargo deny` was not rerun because 4J changes no dependency. The last accepted
result remains the relevant blocker: 20 pre-existing desktop/Tauri transitive
advisory or unmaintained findings, with no exception added.

The governance window was non-mutating. Exact matching before/after hashes:

- Git status:
  `ea81aee945e13dab1aa6d2b4751f0bd7f6ec78a454f2eadd412e9b9f4f835b89`;
- binary diff:
  `781f5e791e8b656c3400a9017be04b18bc44da2d5ce01300f65a3daccc844b99`;
- tracked/unignored inventory/content, with explicit missing-file markers:
  `f7978432106d9df13dae6f6d90658abe1722fcaa1283c41c8fb793c880d9946a`.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Review findings and remaining qualification

The final local review found and closed three fail-open risks before this
record: handshake-implied subscription, loss of envelope ordering in the
desktop bridge, and sending the minor-3 event to minor-2 subscribers. It also
verified that partial queue pressure cannot displace lifecycle-only state and
that callback failure tears down both streaming owners.

FUN-003 and PERF-005 remain blocked. Native qualification still needs an
authorized non-content-reporting run measuring capture acceptance to model
partial and model partial to accessible webview presentation, plus hypothesis
stability, word-edge preservation, cancellation, slow-client behavior, model
families, device/acoustic conditions, RSS/thermal load, package upgrade skew,
and clean-machine behavior. Controlled fixtures cannot establish those facts.

## Cleanup

Removed only regenerated `__pycache__`/`.pytest_cache` directories and the
single npm diagnostic log created by an earlier wrong-directory invocation.
No Narrivy, `pw-record`, transcribe sidecar, or resident worker process remained.
The intentional model caches were retained.
