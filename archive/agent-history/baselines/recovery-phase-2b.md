# Recovery Phase 2B — rework baseline awaiting Sol review

Date: 2026-08-27. Base HEAD is
`ff6d864012adb1468d02c6b5f80b9ce92258fa04`; the worktree remains intentionally
dirty. This is a content-free engineering baseline, not schema-v2 evidence and
not a qualification, phase, package, or release claim.

## Scope and outcome truth

Recovery 2B adds only the opt-in strict
`NARRIVY_RECOVERY_MODE=linux-mic-wtype-inbox` composition:

authenticated CLI → same-user IPC → supervised worker → portable recovery →
PipeWire → local transcribe.cpp CPU with explicit GGUF → local policy and
deterministic correction → `wtype -` stdin attempt → durable Inbox recovery.

The Phase 2A `linux-mic-inbox` lane remains Inbox-primary and independently
selectable. The new lane rejects mock ASR, sine/file audio, null delivery,
the general router, Inbox-only substitution, clipboard, uinput, forced
failure, and conflicting strict modes.

`StartSession` and `SelectPipeline` use one selection rule: strict Phase 2A
accepts only `linux-mic-inbox`; strict Phase 2B accepts only
`linux-mic-wtype-inbox`; both require readiness. The opposite strict id and
mock-dictation/raw/clean are rejected in either strict mode. Normal mode
continues to accept only the normal ids and rejects both strict ids.

`wtype` success is `SubmittedUnverified`, never `Delivered`. Corrected text
and the `wtype-then-inbox` intent are durable before direct dispatch. The
adapter records an actual direct attempt followed by exactly one matching
atomic Inbox write unless cancellation happens before the parent-owned pump
accepts its first payload byte. Its
truthful terminal journal state is `InboxRecovered`; failures to persist the
Inbox or the final transition remain recoverable uncertainty with actual
attempt-report data retained. Payload reaches the child only through stdin;
it is absent from argv, ordinary diagnostics, and reports.

The direct submitter and reusable standard command runner use a parent-owned,
nonblocking stdin pump instead of a writer thread. The one owner closes stdin,
kills, and reaps the child within one absolute two-second deadline shared by
submission, cancellation, errors, timeout, and cleanup. There are no detached
transcript-bearing writers, writer-spawn paths, deadline/ticker threads, or
unbounded joins. If reaping cannot be observed by that original deadline, the
result is a typed termination-timeout failure, not a silent abandonment. Core
admits Inbox recovery only for the exact two-attempt `wtype-stdin` then `inbox`
report shape with matching route and transaction identity; malformed reports
remain recoverable failures.

The pump's explicit dispatch fact is a positive OS write. Cancellation while
the prepared pump has accepted zero bytes is a clean cancellation after child
termination/reap: no direct attempt and no Inbox item are reported. Once one
or more bytes are accepted, cancellation is delivery-uncertain and produces
the required failed direct attempt plus exactly one Inbox recovery item.

The wtype executable is reported only as an available submission mechanism
when resolved executable; verified direct delivery remains explicitly
unavailable and Inbox recovery is explicitly enabled. The only prospective
native target is COSMIC Edit. No compatibility claim exists for another
application, desktop, browser, terminal, Electron, or XWayland.

## Controlled verification (not native proof or evidence)

- Deterministic controlled tests cover fixed argv `[-]`, exact stdin bytes
  (Unicode, multiline, leading dash, empty, and maximum permitted size),
  typed direct failures, one matching Inbox artifact, transaction identity,
  durable intent-before-attempt ordering, partial-report preservation, and
  pre-/post-dispatch cancellation truth.
- A controlled long-running helper verifies late cancellation returns a typed
  post-dispatch result inside the two-second bound and reaps the child. No
  user audio or transcript was used.
- Controlled parent-owned-stdin lifecycle tests prove child reaping after
  success, stdin write failure while the child is alive, a synchronized former
  writer-panic boundary, late cancellation, timeout, nonzero exit, a
  non-draining child, and controlled stdin setup failure. These tests use an
  explicit barrier where child admission must precede fault injection. Timeout
  assertions permit only a documented 300ms scheduler allowance above the one
  original two-second deadline; no test allows a second cleanup window. The
  reusable command runner proves the equivalent success, stdin failure,
  non-draining timeout, and process-spawn cases without any background writer.
- Two additional two-barrier tests pause the real pump after preparation and
  after its first positive write. They prove that zero accepted bytes produce
  clean cancellation with no Inbox item, while positive progress produces a
  delivery-uncertain direct failure and exactly one matching Inbox recovery.
- A core negative matrix proves wrong attempt counts, direct/inbox backends or
  outcomes, fallback-reason combinations, final outcome/failure fields, and
  route identity cannot reach `InboxRecovered`.
- The shared PipeWire latency histogram is serialized only in its test path;
  p99 and microbenchmark measurements use per-thread CPU time to exclude
  unrelated scheduler preemption while retaining the existing 50µs bound.
  `pipewire_monitor::tests` passed 11 tests in each of ten consecutive normal
  concurrency runs.

## Noninteractive commands

| Command | Exit | Material result |
| --- | ---: | --- |
| `rustfmt --edition 2024 --check` on the five affected Rust files | 0 | Every affected file is formatted. |
| `git diff --check` | 0 | No whitespace errors. |
| `cargo test -p narrivy-core --lib --locked` | 0 | 65 tests pass. |
| `cargo test -p narrivy-platform-linux --lib --locked` | 0 | 141 tests pass, including parent-owned stdin lifecycle, reaping, and the exact cancellation boundary. |
| `cargo test -p narrivyd --locked` / `cargo test -p narrivy-cli --locked` | 0 | Daemon (6+23+26+27) and CLI (3) tests pass. |
| `cargo test -p narrivy-platform-linux pipewire_monitor::tests --locked` | 0 | 11 latency/p99 and microbenchmark tests pass. |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | No diagnostics. |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | Full workspace suite passed, including 65 core, 141 Linux-platform, and 24 daemon unit tests. |
| `python3 -m unittest tests.governance.test_recovery_validators -v` | 0 | 12 governance tests pass. |
| `verify_traceability.py` | 0 | 136 requirements mapped; 98 Linux requirements blocked. |
| `verify_evidence.py --production` | 1 | 148 schema-v1 records quarantined; zero qualifying v2 records. |
| `validate_pack.py` | 1 | Dirty/unsealed manifest; read-only. |
| `release_gate.py --target linux-1.0` | 1 | Fail closed; all 98 requirements blocked. |
| `run_gate.py --phase P11` | 2 | Quarantined; no files changed. |

## Historical pre-second-review governance measurement

Immediately before and after exactly these commands:

`python3 -m unittest tests.governance.test_recovery_validators -v`,
`python3 scripts/verify_traceability.py`,
`python3 scripts/verify_evidence.py --production`,
`python3 scripts/validate_pack.py`,
`python3 scripts/release_gate.py --target linux-1.0`, and
`python3 scripts/run_gate.py --phase P11`, the following measurement commands
agreed:

```text
git status --porcelain=v1 | sha256sum
  236d96d41eceb2e2e5f15f975c52eddfd7a883e717fd9050e6de6564ee1f5bc6
git diff --binary | sha256sum
  944570acd3276397767246efbf06880854e7172b95fbae99434da2f8f1588ba7
git ls-files -co --exclude-standard -z | sort -z | xargs -0 sha256sum | sha256sum
  1baf4082771910c7784097882baf9e69f121a88fa936d94fa7da1b6ca70ae9e8
```

These are the preceding-review validator-window measurements. They predate the
parent-owned stdin correction described above, and are intentionally not
presented as its final current diff hash. The second-review command window is
reported separately to Sol with its own before/after measurement boundary.
Protected hashes in this historical window remained:

```text
artifacts/releases/narrivy_0.2.3_amd64.deb  77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0
packaging/deb/BUILD_LOG.md                   0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112
spec/requirements-traceability.csv           ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f
agent/evidence/INDEX.json                     dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5
```

## Native window status and stop condition

No Phase 2B microphone, COSMIC Edit, product `wtype`, or native observation
was run. No audio/transcript temporary directory was created by the product;
controlled tests used only automatic temporary directories and left no
persistent validator log. There are no Phase 2B worker, PipeWire, transcribe,
or wtype children remaining from controlled tests.

**REWORK_COMPLETE_AWAITING_SOL_REVIEW.** This rework did not open a native
window. Any future COSMIC Edit observation requires a separate owner
authorization and remains content-free non-evidence. Phase 2C has not
started; the empty evidence index, all 98 blocked requirements, and the
blocked release remain unchanged.

## Subsequent owner-authorized native observation — additive correction

The stop-condition text above accurately described the rework at the time, but
is no longer the latest Phase 2B fact. A later, separately authorized strict
native run completed on 2026-08-27:

- authenticated strict `linux-mic-wtype-inbox` reached a nonempty 29-byte local
  transcription without disclosing its content;
- one transaction ID matched the durable intent, direct-attempt report, and
  exactly one Inbox item; journal and Inbox payload hashes both equaled
  `329bd3f4…7eaf68a0`;
- the direct `wtype-stdin` attempt was truthfully
  `SubmittedUnverified`; final state was `InboxRecovered / InboxFallback`;
- the isolated COSMIC Edit target was saved and remained empty, so external
  insertion was **not** observed or qualified;
- no worker, recorder, ASR, or wtype child remained; the isolated daemon/editor
  stopped, temporary storage was removed, and ordinary logs remained
  content-free;
- no evidence, requirement status, evidence index, manifest, package, commit,
  or push changed.

This negative target result is the reason Phase 2C evaluates a
permission-mediated portal/EIS candidate. It is observation, not schema-v2
evidence or a release claim.
