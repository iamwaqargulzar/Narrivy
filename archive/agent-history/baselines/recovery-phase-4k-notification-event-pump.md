# Recovery Phase 4K — Notification-driven daemon event pump

**Recorded:** 2026-08-29  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled latency/lifecycle repair implemented; native timing,
power, packaged lifecycle and release qualification blocked; all 98 Linux
requirements remain blocked

## Objective and scope

Remove the daemon event pump's fixed five-millisecond polling delay and idle
wake cadence without weakening bounded backpressure, partial privacy, IPC
authorization, shutdown ownership, or release governance.

Scope is the internal bounded event bus, the daemon event-pump owner and
shutdown ordering, integration harness shutdown, architecture records,
controlled tests, read-only validation, and generated-cache cleanup. No
microphone, transcript, evidence/index, requirement status, manifest reseal,
packaging, commit, push, or release action occurred.

## Implemented boundaries

- Event publication now wakes the one owned pump with a condition variable.
  The 250-ms wait timeout is a defensive fallback only; it is no longer the
  normal dispatch schedule. The prior five-millisecond sleep was removed.
- Queue state and terminal publication admission share one mutex. Closing the
  bus wakes the pump, preserves draining of already-admitted queue items, and
  rejects every later publication. This prevents a publisher from enqueueing
  behind an exited pump.
- Daemon shutdown waits for IPC connection ownership and attempts bounded
  worker reaping before closing the bus. A `WorkerStop::TimedOut`, worker error,
  or event-pump panic produces a failing daemon exit.
- If IPC shutdown itself leaves an admitted connection behind, the closing
  pump consumes its bounded queue without attempting additional subscriber
  writes. A reconnect starts from a new status snapshot and transport epoch.
- Backpressure accounting now distinguishes queued eviction, incoming partial
  drop, same-stream coalescing, and post-close rejection. The previous single
  `dropped_oldest` description could misclassify an incoming drop.
- Priority/coalescing predicates remain internal, pure, non-blocking closures
  under the queue lock. Lifecycle events still evict volatile partials first;
  an incoming partial still cannot evict a lifecycle-only queue.

## Independent review and correction

A parallel read-only adversarial review confirmed the condition-variable
publication handshake, then found that the first draft used an external stop
flag without closing publication admission. That allowed a late publisher to
strand an event after pump exit. The implementation was reworked to the
close-linearized bus above. The review also identified ignored timed-out worker
outcomes, semantically inaccurate drop metrics, unbounded spin coordination in
tests, and possible closing writes to a stalled subscriber; each was corrected
before final validation.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| EventBus repeat loop | 0 | 50/50 runs passed; 11 tests each |
| focused daemon library | 0 | 11 passed |
| focused socket E2E / extension suites | 0 | 63 / 64 passed with all features |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | pass |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | complete pass; one attended portal observation remains ignored |
| workspace all-target/all-feature Clippy with `-D warnings` | 0 | no diagnostics |
| `cargo fmt --all -- --check` / `git diff --check` | 0 / 0 | pass |
| desktop Vitest / TypeScript | 0 / 0 | 17 files, 25 tests; pre-existing React `act(...)` warnings |
| doctor | 0 | current Linux/COSMIC/PipeWire toolchain observed; no permission granted |
| governance suite | 0 | 29 tests passed |
| dependency/source boundaries | 0 / 0 | 9 protected packages |
| action pins / traceability | 0 / 0 | 5 pins; 136 mapped; all 98 Linux requirements blocked |
| production evidence | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| pack / release | 1 / 1 | dirty/unsealed; all requirements blocked; read-only |
| P11 gate | 2 | quarantined and non-writing |

`cargo deny` was not rerun because Recovery 4K changes no dependency. The last
accepted result remains the relevant blocker: 20 pre-existing desktop/Tauri
transitive advisory or unmaintained findings, with no exception added.

The governance window was non-mutating. Exact matching before/after hashes:

- Git status:
  `a9db191793851b74164bcb7c67597d01d492d7d4fd0b953e9075c8c6d08caf04`;
- binary diff:
  `2150adffaf95e4b7e9dbf9a2763992368b7ea16a84f1c60983f5b4fcb0d6811c`;
- tracked/unignored inventory/content, with explicit missing-file markers:
  `433815ee271f91f359a3cd20bb4b64f4d31e97e261dbfa46980bbde6775c2f6b`.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Qualification boundary and remaining work

The structural fixed polling term is removed. Controlled wake tests do not
measure audio acceptance to model partial, model partial to IPC, IPC to webview
paint, scheduling under native load, idle CPU/package power, or p50/p95 user
latency. PERF-005 and every other release requirement remain blocked.

The next performance work should measure content-free native stage timestamps
only after renewed microphone permission, or continue without microphone on
model-independent ownership, pre-roll/segmentation, resource, security and
packaging blockers. No controlled fixture should be converted into release
evidence.

## Cleanup

Generated Python/pytest caches from governance and exact package-manager logs
from this validation are removed after recording. Intentional model and build
caches remain. No Narrivy daemon, recorder, ASR worker, or sidecar process may
remain.
