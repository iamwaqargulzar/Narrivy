# cloud/integration-1

Branch `cloud/integration-1`, made from `origin/main` (`9d5a085`, rc13). The
nine `cloud/…` branches were merged in this order, each with `--no-ff`:
findings-status, advisories, msrv-185, honest-tests, inbox-durable,
clean-punct, ci-leftovers, evidence-hygiene, settings-wired.

Conflicts: only `agent/completion-plan/PROGRESS.md`, at eight of the nine
merges (each branch appended its own entry at the end). Every entry is kept,
in merge order; a script checked that every added line from every branch is
in the result. `services/narrivyd/src/daemon/policy.rs` merged on its own
(honest-tests and inbox-durable change different parts) and builds and tests
clean.

## findings-status

- Verdict: **OK.** Docs only (status table plus PROGRESS entry).
- Note: it is a snapshot of rc13. Later branches on this integration fix
  some rows it lists as open (F-H9, F-H13, F-M18, P-2, P-3); the table is
  not updated.

## advisories

- Verdict: **OK.** Docs only. No ignore was added to `deny.toml`.
- Owner must decide: the 8 open advisories (zip 8.6 via harper-core;
  bincode, paste; zvariant, zbus_names, zvariant_derive via zbus 5). The
  report recommends a time-boxed acceptance per ID in `deny.toml`.

## msrv-185

- Verdict: **OK.** Let-chains in `narrivy-asr-api` turned into nested `if`s
  (same behaviour). rusqlite 0.41 → 0.40 and sha2 0.11 → 0.10 (Cargo.lock
  drops the digest 0.11 stack). No source change for the downgrade.
- Checked: MSRV command from CI on 1.85.0 passes; clippy on 1.97.1 has no
  `collapsible_if` complaint.
- Owner must decide: `cargo deny` was not run on this branch or here, so
  rusqlite 0.40 / libsqlite3-sys 0.38 have no new advisory check. The pin is
  now held by a comment only; nothing fails if someone bumps it except the
  `portable-msrv` CI job.

## honest-tests

- Verdict: **OK, one fix.** The mic test now feeds every recorder state
  instead of reading the host (it was the one test failing in the cloud);
  the `disabled` verdict that the daemon never sends is gone from the UI;
  child-process tests in `process-linux` now have a real deadline; the
  History tests wait for the session to finish instead of racing the write.
  No assertion was weakened that I could find; timeouts only got longer and
  are hang caps, not pass conditions.
- Fixed (own commit): the new
  `the_packaged_daemon_refuses_to_start_without_the_installed_runtime` test
  used `Command::output()` with no time limit. If a regression made the
  packaged daemon start serving, the test (and CI) would hang instead of
  failing. It now uses `DaemonProcess::wait_for_exit(EXIT_DEADLINE)`.
- Note: the packaged SIGTERM/SIGINT test is `#[ignore]` and needs an
  installed package; it is not run by CI.

## inbox-durable

- Verdict: **OK in code, one problem left for the owner.** Inbox moved from
  `$XDG_RUNTIME_DIR/narrivy/inbox` to the 0700 state root
  (`$XDG_STATE_HOME/narrivy/inbox`), old files are moved over at start,
  retention is 24 h and at most 100 files; an accepted-but-unverified typed
  or pasted delivery no longer leaves a plaintext Inbox copy; the mock lane
  journals `Committed` before delivery and a new SIGKILL test proves the
  text is replayed. The only new log line prints a file count, no text.
- **Fixed (own commit):** three strict routed tests in
  `services/narrivyd/src/daemon/session.rs` that build only with
  `asr-transcribe-cpp` still expected an Inbox copy after an accepted
  submission. The branch never ran this feature lane, so
  `cargo test -p narrivyd --features asr-transcribe-resident` (a CI step)
  failed 3 tests in 5 targets. They now assert the new contract:
  `SubmittedUnverified`, journal `Delivered`, no Inbox file.
- **Problem, not fixed here:** two qualification scripts still read the
  Inbox from the runtime dir, so the next native run will fail with
  "expected one inbox artifact, observed 0":
  `scripts/qualify_candidate_known_phrase.py:1372` and
  `scripts/record_native_ui_dictation_evidence.py:835,838,928` (the fake in
  `tests/packaging/test_qualify_candidate_known_phrase.py:164` matches the
  old path, so tests stay green). The fix is one path each
  (`state_dir / "narrivy" / "inbox"`), but both files are hash-pinned:
  `qualify_candidate_known_phrase.py` by `IDENTITY_SHA256` /
  `QUALIFIER_SHA256` in three recorder scripts, and
  `record_native_ui_dictation_evidence.py` by about 15 runner pins in
  `spec/evidence-verification-profiles.json`. Changing them means a re-pin
  chain across evidence profiles, which `AGENTS.md` says is qualification
  work done on purpose, one requirement at a time. **Owner decides** whether
  to do that re-pin now or with the next qualification round.
- Owner must decide / know:
  - With History retention off, a typed-but-unverified dictation is now kept
    nowhere. That is what F-M4 asked for, but it is a product choice.
  - The 24 h / 100 files numbers.
  - The SIGKILL test covers the mock lane. F-M18 as written in the findings
    status asks for the strict lane; the strict lane already commits before
    delivery, but has no kill test.

## clean-punct

- Verdict: **OK.** Clean no longer writes `3. 14`, `1, 000`, `10: 30`,
  `example. Com`, or breaks links and `e.g.`. Old tests unchanged, new cases
  added.
- Small behaviour changes to know: a space is now added after `.` only
  between a lowercase and an uppercase letter (`end.next` stays as it is),
  and after `,;:!?` only between two letters (`items:5` stays). A text that
  starts with a number no longer capitalises the next word (`3 apples`).

## ci-leftovers

- Verdict: **OK, but it relaxes two gates — owner must decide.** CI now runs
  the whole Python suite (before, only a few modules), deletes the unused
  `usePerfMarks` hook, adds the missing Tauri commands to the governance
  list, and adds a 2 s capture-stall test.
- Gates moved to `continue-on-error`:
  - `test_all_legacy_entry_points_and_imports_cannot_write` (manifest
    drift). On `main` this test ran in a blocking step and already failed.
    Now it is non-blocking. `BUILD-PACK-MANIFEST.json` needs an
    owner-reviewed `scripts/build_manifest.py` run.
  - `LiveFrontendGuardTests` (hard-coded bundled model id in `Models.tsx`).
    Note `python3 scripts/verify_frontend_boundaries.py` is still a
    **blocking** step in the `rust-quality` job and still fails, so CI stays
    red there until the model id comes from the daemon.
  I did not revert this: these were already red, and the CI comment says why.
  But `AGENTS.md` says not to weaken gates to get green, so the owner should
  confirm or revert.

## evidence-hygiene

- Verdict: **OK.** Six old P10 records moved to `agent/history/`; SEC-004
  runner pin now equals the file on disk (I checked the hash); the SBOM now
  carries real licences from `cargo metadata --offline` and the npm lockfile;
  phase report counts updated; a new test keeps runner pins and P10 IDs
  honest. `BUILD-PACK-MANIFEST.json` was updated by that branch too.
- Owner must decide: re-pinning SEC-004 makes older SEC-004 records (made
  by the earlier recorder) fail; they need a fresh recording. B-LEGAL-016
  (model licence texts in the .deb) is new and open.

## settings-wired

- Verdict: **OK.** Pipeline Import/Export buttons are disabled with a
  "Planned" chip because the service never reads saved pipelines (F-H13);
  onboarding and models-hardware specs marked roadmap; a new test checks
  that visible controls are wired. `tsc` accepts the `title` prop.
- Note: the Tauri commands `pipelines_export` / `pipelines_import` and the
  bridge methods stay in place, unused by the UI.

## Fixes made on this branch

1. `7c6b98a` strict routed tests follow F-M4 (see inbox-durable).
2. `3b5708b` `services/narrivyd/tests/daemon_shutdown_signals.rs`: bound the
   packaged refusal test with a deadline (see honest-tests).

## Test results

Run in this cloud container (Ubuntu 24.04, Rust 1.97.1 from
`rust-toolchain.toml`, Tauri system libraries installed as in CI).

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo test --workspace --locked` | pass: 2224 passed, 0 failed, 6 ignored (run before the two fixes; they touch only feature-gated and packaged tests) |
| `cargo test -p narrivyd --features asr-transcribe-resident --locked` | before fix: 3 tests failed in 5 targets; after fix: 1389 passed, 0 failed, 2 ignored |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo clippy -p narrivyd --all-targets --features asr-transcribe-resident --locked -- -D warnings` | pass |
| MSRV: `cargo +1.85.0 check --locked -p narrivy-types … -p narrivy-core` (CI `portable-msrv`) | pass |
| `python3 -m pytest tests` | 6 failed, 317 passed (see below) |
| `npx tsc --noEmit` in `apps/narrivy-desktop` | pass |
| `npm test` (Vitest) in `apps/narrivy-desktop` | 221 passed, 55 files. One earlier run had one failure in `AssistantScreen.test.tsx` ("saves a stopped share raw…", `findByRole` timeout) while cargo was building in parallel; it passed 3 of 3 alone and in the full re-run. Timing flake, not from these branches (none touch that screen). |

Python failures, all also failing on `origin/main` (main has 8 failures; the
integration fixes 2 of them, `test_desktop_tauri` and the dpkg-failure
test, which passes here):

- `test_frontend_boundaries.py::LiveFrontendGuardTests` (3 tests):
  hard-coded bundled model id in `Models.tsx`.
- `test_recovery_validators.py::…test_all_legacy_entry_points_and_imports_cannot_write`:
  `BUILD-PACK-MANIFEST.json` drift.
- `test_legacy_deb_builder_quarantine.py::…` and
  `test_phase_report_generation.py::…test_reports_only_attest_to_immutable_release_artifacts`:
  need the gitignored release `.deb` files.

These are exactly the tests `cloud/ci-leftovers` deselects in CI. Not run
here: `cargo deny` (not installed), the Vulkan and OpenVINO worker steps,
and the native qualification scripts.
