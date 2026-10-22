# Recovery Phase 2A — NON-RELEASE REWORK RECORD

Date: 2026-08-27. Base HEAD remains
`ff6d864012adb1468d02c6b5f80b9ce92258fa04`; the worktree is intentionally
dirty. This is a content-free engineering record, not qualification evidence.
No commit, push, package build, manifest reseal, schema-v2 evidence record,
evidence-index update, or traceability status update occurred.

## Scope and truth boundary

Recovery 2A remains a narrow, opt-in Linux microphone-to-Inbox slice:
authenticated same-user IPC → one daemon-owned worker → portable recovery
ordering → PipeWire capture → local transcribe.cpp CPU → deterministic
correction → durable Inbox primary delivery. It does not qualify a
requirement, a phase, a package, a desktop integration, or a release.

Strict `NARRIVY_RECOVERY_MODE=linux-mic-inbox` accepts no mock, sine fixture,
recorded-file, null delivery, general router, clipboard, wtype, or uinput
fallback. The CLI forwards no global recovery default; a normal daemon chooses
its normal default. Strict events carry the selected `linux-mic-inbox` id and
strict pipeline listing contains only that id, disabled whenever readiness
fails.

## Sol rework corrections

- Capture and sidecar directories use exclusive random creation below a
  checked non-symlink parent, mode 0700, and RAII cleanup. Captured audio and
  sidecar transcript files are mode 0600. Inbox and journal temporary files
  are exclusive mode 0600, atomically renamed, and their directory is fsynced.
  Unix tests cover modes, symlink resistance, collision rejection, successful
  cleanup, cancellation cleanup, and controlled child termination/reaping.
- One `SessionCancellation` belongs to the session slot, worker supervisor,
  PipeWire child, sidecar child, correction, and delivery. Completed workers
  remain owned until a supervisor observation or admission externally joins
  and removes them; daemon exit calls bounded `shutdown_all`. Controlled IPC
  shutdown leaves zero supervised workers, no delivery, and a cancelled—not
  pending—journal record.
- GGUF readiness validates the container magic and supported version rather
  than deriving an identity from a path name. It distinguishes missing,
  invalid, and unsupported models; sidecar and PipeWire checks distinguish
  missing and non-executable binaries. Status additionally marks the slice
  `recovery.behavioral_qualification=unqualified`.
- Inbox is the primary destination in this slice. A successful Inbox report
  is backend `inbox`, one matching attempt, and `Delivered`; it is never
  labelled `InboxFallback`. The general platform delivery ladder is unchanged.
- Raw-transcript policy is checked before local journal persistence. Spawn
  failure closes the pending record as `worker_spawn_failed` and removes the
  active slot. A final journal-write failure after confirmed primary delivery
  is `journal_write_after_delivery_uncertain` and leaves the earlier committed
  intent for explicit recovery rather than fabricating a delivered row.
- Worker admission and the transition into shutdown share an explicit
  fail-closed lifecycle boundary. Admission retains that boundary through OS
  thread creation, registry insertion, and start-gate release; shutdown first
  closes admission and then snapshots/cancels every already-admitted worker.
  Deterministic channel-controlled tests cover admission/shutdown overlap,
  observed cancellation/join, post-close rejection without job execution,
  completion cleanup, timeout restart refusal without overlapping generations,
  atomic timeout-state restoration during reap/shutdown contention, bounded
  concurrent stop/reap/shutdown coalescing without retained waiter
  registrations, typed panic publication before removal, and bounded panic
  diagnostics.
  The admission-overlap test now proves contention directly: while admission
  holds the lifecycle mutex and the registry is empty, the shutdown path's
  test-only `try_lock` must return `WouldBlock` before the test releases
  admission. Production lifecycle behavior is unchanged by this test hook.

## Controlled verification

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo test -p narrivy-core worker::tests --locked` | 0 | 19 worker lifecycle tests, including deterministic admission/shutdown, timeout-state, concurrent-stop, no-waiter-retention, and panic-publication coverage. |
| `cargo test -p narrivy-core -p narrivy-platform-linux -p narrivy-asr-transcribe --lib --all-features --locked` | 0 | 287 passed and one native portal observation ignored; journal, recovery policy, secure storage, and worker lifecycle pass. |
| `cargo test -p narrivy-asr-transcribe --features runtime sidecar::tests --lib` | 0 | 5 runtime sidecar tests, including controlled cancellation/reap. |
| `cargo test -p narrivy-platform-linux recovery::tests --lib` | 0 | 6 recovery adapter tests, including controlled `pw-record` cancellation/reap. |
| `cargo test -p narrivyd -p narrivy-cli` | 0 | daemon, IPC, mock-path, CLI, and controlled shutdown tests pass. |
| `cargo test -p narrivyd --features asr-transcribe-cpp --bin narrivyd` | 0 | 21 strict-runtime policy/event tests pass. |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | No diagnostics. |
| `cargo test --workspace --all-features --locked` | incomplete | Historical Phase 2A result: 33/34 IPC tests passed, then the former `ipc_server_accept_once_handles_single_connection` disconnect wait exceeded the 30-second command limit. This row is superseded by `recovery-quality-gate-cleanup.md`, where the authenticated request/response test and full workspace sweep pass. |
| `rustfmt --edition 2024 --check crates/narrivy-core/src/worker.rs` | 0 | Worker file formatted. |
| `git diff --check` | 0 | No whitespace errors. |
| `npm test -- --run` in `apps/narrivy-desktop` | 0 | 17 files / 24 tests; existing React `act` warnings only. |
| `./node_modules/.bin/tsc --noEmit --incremental false` | 0 | No diagnostics. |

## Native observations (not evidence)

An isolated strict daemon reported `capture.pipewire=available`,
`model.gguf=available`, `asr.transcribe-cpp.cpu=available`, Inbox-only
delivery, `pipeline.linux-mic-inbox=enabled`, and `recovery.ready=yes`.
Its configured model and executable were the previously reviewed local assets;
this record intentionally does not claim their names as product identity.

- An earlier six-second microphone run ended `failed_terminal/empty_transcript`
  without an Inbox item, delivery intent, delivery report, or child process.
  No transcript or audio content is recorded here.
- A later real six-second strict run reached `Delivered`: it produced one
  Inbox item, matching journal/delivery/Inbox transaction identifiers, and
  equal in-process delivered/Inbox text hashes. It left no worker or capture/
  ASR child process, and secure temporary payload files were removed. The
  observation is content-free, independently corroborated by Sol, and is not
  schema-v2 evidence or a release qualification claim. This final lifecycle
  rework performed no additional microphone run.
- An immediate native cancellation ended `cancelled`, with no Inbox item,
  delivery intent/report, or remaining `pw-record`/transcribe child.
- An isolated missing-model configuration advertised
  `model.gguf=unavailable:model_missing`, marked the recovery pipeline
  unavailable, and rejected StartSession with exit 1 before creating a journal
  entry.
- Controlled daemon shutdown during a delayed active session exited within the
  two-second bound with worker count `1 → 0`, no delivery, no child, and a
  cancelled journal record.

The genuine Delivered observation does not change requirement status, create
evidence, qualify a phase, package, or release. The release remains blocked.

## Governance and non-mutation

| Command | Exit | Material result |
| --- | ---: | --- |
| `python3 -m unittest tests.governance.test_recovery_validators -v` | 0 | 12 tests passed. |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; 98 Linux 1.0 requirements blocked. |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical schema-v1 records quarantined; zero v2 candidates. |
| `python3 scripts/validate_pack.py` | 1 | Dirty, unsealed manifest; read-only. |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | Fail-closed: pack failure and all 98 requirements blocked. |
| `python3 scripts/run_gate.py --phase P11` | 2 | Quarantined; no files changed. |
| P9/P11 runners and refresh scripts, direct and imported | 2 each | All eight calls quarantined; evidence and manifest inventory unchanged. |

The non-mutation measurement window begins immediately before, and ends
immediately after, exactly these six commands: the governance unittest,
`verify_traceability.py`, `verify_evidence.py --production`,
`validate_pack.py`, `release_gate.py --target linux-1.0`, and
`run_gate.py --phase P11`. At both boundaries,
`git ls-files -co --exclude-standard -z | sort -z | xargs -0 sha256sum | sha256sum`
was `a905e280e6ba8a544aad1e56df7af1cf2fc638cb0b966608245ed599cf84e2f3`,
`git diff --binary | sha256sum` was
`9f61650410ca024c334510803503f086fa868f1b9cd168c007b54d7e71db8e5a`,
and `git status --porcelain=v1 | sha256sum` was
`21b4801ab83cf0b65717da9c2312780e13c377dcf00afaa3725ea6c66a805de6`.
The protected `.deb` and `BUILD_LOG.md` hashes remained respectively
`77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0` and
`0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
traceability was `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`
and the empty evidence index was
`dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.
These are validator-window measurements only. They do not claim to be the
current final diff or status hashes after the subsequent permitted baseline
and state-record edits.

## Stop condition

**REWORK_COMPLETE_AWAITING_SOL_REVIEW.** Recovery 2A lifecycle rework and
controlled validation are complete. The genuine native Delivered observation
is independently corroborated, but remains content-free and non-evidence. The
release stays blocked, the index stays empty, all 98 Linux 1.0 requirements
remain blocked, and no Phase 2B work may start.
