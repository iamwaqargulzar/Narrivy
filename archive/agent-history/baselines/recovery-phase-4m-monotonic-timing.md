# Recovery Phase 4M-B — Ephemeral monotonic timing

**Recorded:** 2026-08-30  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled timing boundary and real-resident fixture diagnostic
accepted; all 98 Linux requirements and release gates remain blocked

## Objective and scope

Replace the untrusted legacy wall-clock/persistent timing path with an optional,
transaction-local monotonic observer, then exercise it through the real
resident model and Inbox transaction without a microphone or release claim.

Scope is portable recovery timing, removal of daemon timing persistence,
controlled tests, one ignored explicit-fixture diagnostic, and architecture
records. No production observer, timing IPC, microphone, evidence/index,
requirement status, manifest reseal, package, commit, push, or release action is
included.

## Implemented boundary

- Ten fixed enum stages cover capture request/readiness, ASR open/first audio,
  first accepted partial, capture end, final acceptance, corrected+intent
  durable commit, delivery return, and terminal durable commit.
- `RecoveryTimingTrace` uses one monotonic origin and fixed atomics. First write
  wins; it contains no payload, identity, wall-clock stamp, arbitrary label,
  persistence, or IPC behavior.
- Production composition passes no observer. The protocol-minor-3 legacy
  `stage_timings_us` field is absent from current daemon responses.
- Legacy daemon timing maps, completed timing retention, JSON writer, and
  default runtime timing directory were removed.
- Negative tests prove rejected partials and failed correction cannot
  manufacture downstream markers.
- The ignored diagnostic requires explicit regular-file paths and a pinned
  fixture digest, rejects malformed/duplicate/oversized WAV structure, paces
  80-ms blocks cancellably, and composes the real resident worker,
  deterministic correction, Unix journal, and Inbox-only delivery.
- Diagnostic storage is one temporary 0700 root; journal/Inbox files are 0600,
  exactly one row/item is allowed, hashes must match, and the root is explicitly
  removed before success.

## Controlled real-resident observation

Inputs:

- ignored transcribe.cpp JFK WAV, SHA-256
  `59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e`;
- Moonshine Streaming Tiny Q8_0 GGUF, SHA-256
  `930e4622ad3a24158b91406c30c977fa6a26b34cb32d6ac3e57cfb23383a869e`;
- locally built resident worker, SHA-256
  `64e3c9cd03303c18a4bee665f66d50d8206925ef6e1780839dc86377b4d770af`.

One explicit ignored run passed and observed all ten stages. Relative offsets
from the diagnostic origin were:

| Boundary | Offset (microseconds) |
| --- | ---: |
| capture start | 3,541 |
| capture ready | 83,608 |
| ASR opened | 85,771 |
| first audio accepted | 87,934 |
| first partial accepted | 773,422 |
| capture ended | 11,875,416 |
| raw final accepted | 12,251,960 |
| corrected + intent committed | 12,256,045 |
| delivery returned | 12,258,205 |
| terminal journal committed | 12,260,372 |

The first complete paced block correctly becomes ready at about 80 ms. The
final run consumed the roughly 11-second fixture in about 11.9 seconds. An
earlier run under concurrent build/test activity took about 25.2 seconds, so
host-load sensitivity requires a controlled repeated protocol before any
performance attribution. The 773.4-ms origin-to-partial value is **not** speech
onset to partial: speech onset is unannotated and PipeWire/UI are substituted.
No threshold, percentile, or PERF-005 claim follows.

The run created one temporary journal row and one Inbox item with equal hashes,
then removed the root. Before/after process inventory found no daemon,
`pw-record`, transcribe sidecar, or resident worker. No transcript text appeared
in command output.

## Review and validation

An independent read-only design review found and caused correction of a false
marker: `CorrectedDurablyCommitted` was initially placed after raw persistence.
It now occurs only after corrected text plus delivery intent are durably
written, with a dedicated correction-failure regression test. The same review
required real correction, truthful paced readiness/duration, pinned fixture
identity, strict WAV bounds, private-mode checks, exact row/item counts, and
external process inventory; all were incorporated.

A second final-review subagent could not run because its usage allowance was
exhausted and returned no verdict. The primary agent therefore performed the
final read-only stage/failure/privacy/IPC/document reconciliation; it found no
additional defect. This is not represented as a second independent PASS.

### Validation results

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` / `git diff --check` | 0 / 0 | pass |
| core library tests | 0 | 99 passed, including three timing-truth tests |
| daemon all-feature tests | 0 | complete pass; explicit diagnostic ignored by default |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | pass |
| full workspace test sweep | 0 | complete pass; attended portal and explicit timing diagnostics ignored |
| strict workspace Clippy | 0 | all targets/features, warnings denied |
| no-default daemon check with warnings denied | 0 | pass |
| desktop Vitest / TypeScript | 0 / 0 | 17 files, 25 tests; existing React `act(...)` warnings |
| explicit real-resident timing diagnostic | 0 | one passed; ten stages; private storage removed |
| doctor / action pins / source boundaries / dependency boundaries | 0 | 5 pins and 9 protected packages |
| `cargo deny --offline --locked check` | 1 | advisories fail; bans/licenses/sources pass; no exception added |

The first full workspace test attempt hit the pre-existing scheduler-sensitive
`latency_histogram_p99_under_load_stays_under_50us` assertion: p99 was 21 µs,
but p50 was 14 µs against its 10-µs threshold. The untouched test then passed
five consecutive isolated runs (p50 5–6 µs, p99 6–16 µs), and the final full
workspace sweep passed. No threshold or unrelated source was changed. This is
recorded as test-harness sensitivity, not hidden as a clean first attempt.

### Fail-closed governance window

| Command | Exit | Material result |
| --- | ---: | --- |
| governance suite | 0 | 12 passed |
| `verify_traceability.py` | 0 | 136 mapped; all 98 Linux requirements blocked |
| `verify_evidence.py --production` | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| `validate_pack.py` | 1 | expected dirty/unsealed manifest mismatch; read-only |
| `release_gate.py --target linux-1.0` | 1 | expected fail-closed release; all 98 blocked |
| `run_gate.py --phase P11` | 2 | quarantined; no writes |

The tracked/unignored workspace inventory includes explicit missing-file
markers and excludes only this self-referential baseline. It and the binary
diff/status hashes matched immediately before and after exactly those six
commands:

- Git status:
  `e154b21de76c2a9afebf8a3cd58163ce56f7e9d52a8be22d12393d3837d41a46`;
- binary diff:
  `8c4db6473357272dfd60869f0f79e164e96b1c852df7a95597dceacfc05fead4`;
- tracked/unignored inventory/content:
  `4d290e9d758d85859bd261033276c70fda3cd6ff3ecf8815a23e401bbcdfb0be`.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

Generated Python `__pycache__` directories from governance were moved to the
desktop trash. No `.pytest_cache`, diagnostic temporary root, daemon,
`pw-record`, transcribe sidecar, or resident worker remained. Build/model caches
were retained as reusable developer cache state, not release artifacts.

## Qualification boundary and remaining work

The controlled run proves the real resident streaming transaction can populate
content-free monotonic boundaries and complete durable Inbox delivery. It does
not prove microphone routing, speech onset, trigger/IPC acknowledgement,
PipeWire readiness, clipping/loss under load, partial repaint latency,
hypothesis stability, accuracy, p50/p95, RSS/thermal behavior, package behavior,
or any release requirement. A provenance-reviewed annotated corpus and a
physical native loopback/microphone protocol remain prerequisites for latency
qualification.
