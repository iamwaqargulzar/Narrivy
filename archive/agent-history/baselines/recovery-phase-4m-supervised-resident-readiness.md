# Recovery Phase 4M-A — Supervised resident readiness

**Recorded:** 2026-08-30  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled resident lifecycle and strict-admission repair accepted;
native latency, clipping, acoustic, package, evidence, and release qualification
remain blocked; all 98 Linux requirements remain blocked

## Objective and scope

Prevent a terminated resident ASR worker from being synchronously respawned
inside the next capture-first transaction. Make live worker health, recovery,
strict-session admission, and capability reporting explicit and fail-closed.

Scope is the process-isolated resident engine, daemon admission/composition,
controlled lifecycle tests, architecture records, read-only validation, and
generated-cache cleanup. No microphone, audio, transcript, native dictation,
evidence/index, requirement status, manifest reseal, package, commit, push, or
release action occurred.

## Diagnostic basis

A content-free local diagnostic with the cached Moonshine canary observed
approximately 107 ms to process/model READY. Twenty already-loaded
`OP_STREAM_BEGIN` observations were sub-millisecond (about 0.007 ms median),
while later 80-ms feeds showed family-specific compute spikes. This was a
diagnostic observation only, not release evidence. It showed that anonymous
stream preopening would hold transactionless compute ownership without fixing
the real failure path: model reload after worker loss.

## Implemented boundary

- `ResidentWorkerEngine` exposes `Ready`, `Active`, `Warming`,
  `RestartRequired`, and `FailedClosed` lifecycle truth.
- One-shot recognition and `open_stream` never spawn. A missing/dead/uncertain
  worker returns a typed unavailable/restart result.
- Recovery elects one generation while locked, performs cancellable startup
  outside the owner lock, then installs only the elected generation. Concurrent
  callers observe `AlreadyWarming`; they cannot create another replacement.
- Cancellation is checked before startup, after the READY handshake, and under
  the install lock. A candidate can never race shutdown into `Ready`.
- Capability drift across replacement fails closed. Healthy finalize/reset
  returns `Active` to `Ready`.
- Active, replacement, pre-handshake, constructor, and final-drop cleanup paths
  preserve exact child ownership when bounded termination itself fails.
  Retiring children must be reaped before another replacement/constructor is
  admitted; no detached reaper thread exists.
- The daemon acquires one atomic RAII strict-session permit before pending
  journal creation and retains it through every supervised worker terminal or
  panic path. A concurrent strict request is `Busy` before side effects.
- Resident strict requests require live `Ready` before `begin_session` or
  capture. `RestartRequired` schedules engine-deduplicated recovery through the
  existing `WorkerSupervisor` and rejects the current request.
- Status publishes the live worker state. Streaming/VAD/endpoint composition
  and strict pipeline enablement no longer rely on cached model capability
  alone.

## Independent adversarial review

The first review found two P1 defects: cancellation after READY could install a
replacement, and failed termination could drop active/candidate ownership. The
second review found the same ownership gap before the startup handshake. The
third found it during final `SharedWorker` teardown and also found no-default
warnings introduced by conditional daemon wiring.

Each finding received deterministic regression coverage. Candidate
cancellation now rechecks both post-handshake boundaries; active/candidate/
startup/final-drop failures move the exact process handle into an owned retiring
queue; the next recovery/constructor reaps it before spawning. Precise feature
gates remove the no-default warnings without weakening a gate. The final
read-only re-review returned **PASS** and found no remaining lifecycle,
lock-order, lazy-spawn, admission, shutdown, or status-truth blocker in scope.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| resident engine focused tests | 0 | 45 resident lifecycle tests passed; 52 package all-feature tests in independent review |
| resident engine all-target Clippy `-D warnings` | 0 | pass |
| `RUSTFLAGS='-D warnings' cargo check -p narrivyd --no-default-features --locked` | 0 | pass |
| daemon all-feature tests / Clippy | 0 / 0 | 11 library + 63 daemon + 66 e2e + 67 extension tests; no diagnostics |
| `cargo fmt --all -- --check` / `git diff --check` | 0 / 0 | pass |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | pass |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | complete pass; Linux 208 passed and one attended portal observation ignored |
| workspace all-target/all-feature Clippy with `-D warnings` | 0 | pass |
| desktop Vitest / TypeScript | 0 / 0 | 17 files, 25 tests; existing React `act(...)` warnings |
| governance suite | 0 | 29 tests passed |
| source/dependency boundaries | 0 / 0 | 9 protected packages |
| action pins / traceability | 0 / 0 | 5 pins; 136 mapped; all 98 Linux requirements blocked |
| production evidence | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| pack / release | 1 / 1 | dirty/unsealed; all 98 requirements blocked; read-only |
| P11 gate | 2 | quarantined and non-writing |

The first governance command list accidentally referenced the nonexistent
`scripts/verify_action_pins.py` (exit 2). The correct authoritative command,
`python3 scripts/check_action_pins.py`, was then run and passed with five pinned
uses. This operator typo did not change repository content and is not reported
as a product/gate failure.

`cargo deny` was not rerun because Recovery 4M-A changes no dependency. The
last accepted result remains the relevant blocker: 20 pre-existing desktop/
Tauri transitive advisory or unmaintained findings, with no exception added.

The full governance window was non-mutating. Exact matching before/after
hashes were:

- Git status:
  `b5671f41198885697bcd48636eb0c569de4552d2cc70655437f5a4ede59bc451`;
- binary diff:
  `9c0a76555b4f0ea4296367d5e47f569036ddb38ef89a6d8245199033ce86e3c4`;
- tracked/unignored inventory/content, with explicit missing-file markers:
  `e0a77779d4fa3281b7cc37b340661e4169af1e635ce10f8d7db0651401868c6a`.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Qualification boundary and remaining work

This phase proves controlled lifecycle ownership and prevents worker reloading
from consuming the next transaction's bounded early-audio window. It does not
prove native first/partial/final latency, no clipping under load, accuracy,
noise behavior, RSS/thermal limits, device routing, clean-machine behavior, or
package behavior. The formal release remains intentionally blocked.

The next suitable no-microphone phase is content-free monotonic stage timing
with a controlled, provenance-locked speech fixture that exercises the real
resident worker and Inbox transaction without becoming a production fallback
or release evidence. Durable recovery storage/History separation remains a
larger subsequent architectural repair.

## Cleanup

Generated Python `__pycache__` directories from governance were moved to the
desktop trash after validation. No `.pytest_cache`, npm debug log, WAV, or
temporary payload file remained in the repository. No Narrivy daemon,
`pw-record`, resident worker, or transcribe sidecar process remained.
Intentional Rust, Node, and model caches were preserved as reusable build/cache
state, not release artifacts.
