# cloud/integration-2

Branch `cloud/integration-2`, made from `origin/cloud/integration-1`
(`36ee97d`, report `cloud-integration-1.md`). The six later `cloud/…`
branches (all cut from `main`, rc13) were merged in this order, each with
`--no-ff`: recorders, gate-trust, docs-sweep, fuzz-hardening, robustness,
a11y-i18n.

Conflicts:

- `agent/completion-plan/PROGRESS.md` at all six merges (each branch appends
  its entry at the end). Every entry is kept, in merge order. A script
  checked that every non-empty line each branch (and integration-1) added is
  in the result: 0 missing.
- `tests/packaging/test_qualify_candidate_upgrade.py` (recorders vs
  honest-tests F-M16). Kept the filesystem model from integration-1 and added
  what recorders needs: the daemon start script, `test -S` on the socket,
  `narrivy-cli status` (fails when the daemon is not running), `kill -TERM`,
  the pid/exit files, and `status_fails_on`. Removing the package while the
  daemon still runs now fails in the fake, as in the lifecycle fake.
- `services/narrivyd/src/daemon/policy.rs` (robustness vs honest-tests, the
  same mic test fixed twice). Kept the integration-1 version: it feeds every
  recorder state and also covers the host case robustness fixed.
- `services/narrivyd/tests/daemon_shutdown_signals.rs` (robustness vs
  honest-tests). Kept both: the packaged-shape module, then the new
  slow-discovery test at top level.
- `apps/narrivy-desktop/src/app/Onboarding.tsx` (a11y-i18n vs honest-tests).
  Kept a11y's outer live region and the honest-tests verdict (no `disabled`).
- `services/narrivyd/src/main.rs` (robustness vs inbox-durable) merged on its
  own; they change different parts. Builds and tests clean.

## recorders

- Verdict: **OK, one fix.** New `scripts/recorder_safety.py` (guarded
  replace, native-lane proof, log privacy scan, structured test results,
  bounded process groups) used by every recorder. I read it in full: the
  log scan keeps the transcript in memory and returns counts only; a missing
  log fails; a skipped or missing named test fails. The cross-UID probe
  reads the server's real "cross-user connection denied" frame (little-endian
  prefix, as in `narrivy-ipc` framing). No recorder prints transcript text.
- **Fixed (own commit `3d3a528`):** three runner pins in
  `spec/evidence-verification-profiles.json` did not match the files. The
  branch itself left `record_native_ui_dictation_evidence.py` (15 profiles)
  and `record_native_hardening_evidence.py` (3) pinned to an earlier edit,
  and it changed `record_controlled_integration_evidence.py` while
  evidence-hygiene had pinned the old file (SEC-004). The evidence-hygiene
  pin test failed on 19 profiles. The pins now equal the files on disk. This
  is the re-pin the branch says it made; older records made by those
  recorders now fail by hash, as recorders already expects.
- Owner must decide / know:
  - `record_package_upgrade_evidence.py` has `PREVIOUS_SHA256 = None`, so the
    upgrade recorder fails closed until the owner pins the previous package.
  - The cross-UID probe (SEC-005) needs `sudo -n` as `nobody` and briefly
    opens the runtime dir to 0711 and the socket to 0666 during the probe
    (modes are restored and re-checked).
  - OPS-004 rejects `NOASSERTION`; `build_candidate_supply_chain.py` still
    writes it for crates with no declared licence.
  - Not from this branch, seen while checking pins: `LIFECYCLE_SHA256` in
    `record_package_lifecycle_evidence.py` does not match
    `scripts/qualify_candidate.py` (also on `main`).
  - The inbox path problem from integration-1 is still open:
    `scripts/qualify_candidate_known_phrase.py:1398` still reads the Inbox
    from the runtime dir. Recorders changed and re-pinned this file anyway,
    so the one-line fix now costs one more re-pin of the same chain.

## gate-trust

- Verdict: **OK. Gates got stricter; I found no failure turned into a pass.**
  Checked one by one:
  - Release gate: tree cleanliness now comes from `git status` (only release
    outputs may differ); the three `qualification.json` booleans are derived
    from requirements the gate accepted and must agree; P11 JSON report is
    mandatory; `summary.md` must name the full commit; lane environment and
    p50/p95 must equal the evidence; package hash must be one the gate
    re-hashed; NaN/Infinity rejected; commit checks no longer skip when the
    commit is `unknown` (that case already errors).
  - Phase gate: Markdown report fallback removed; native evidence counts only
    after a full `verify_evidence(native_required=True)` (which still checks
    `result == "pass"` through `assess_evidence`); "passed" regression rows
    are re-verified; commands get a timeout (exit 124 = fail) and their
    process group is killed; timestamps are checked for every schema.
  - Blockers: I ran the old and new blocker logic over `agent/BLOCKERS.md`
    for every phase. The new logic reports every blocker the old one did and
    adds B-AUTH-006 (P5), B-LEGAL-007 (P0, P10, P11), B-LEGAL-016 (P10),
    B-PROCESS-014, B-PACKAGE-015 (P10, P11), B-RECOVERY-001 (P11). None lost.
- Owner must know: B-ARCH-009, B-STORAGE-013 and B-IPC-012 are open but name
  no phase in `Phase/Requirement`, so no phase gate reports them (the old
  logic did not either). B-LEGAL-007 now blocks P0 because its field reads
  "P0 → P10 pre-RC → P11"; edit the field if only P10/P11 is meant.
  `NARRIVY_GATE_COMMAND_TIMEOUT` can raise the 3600 s cap.

## docs-sweep

- Verdict: **OK, two fixes.** Docs brought to rc13 (no Dictation page, four
  engine crates, Tauri 2, default `clean`), control templates equal the
  builder, new guard test against live delivery rungs in tests, and
  `mock-dictation` removed as a pipeline id.
- No file overlap with settings-wired, but one doc overlap:
  - **Fixed (own commit `0d7fddb`):** `information-architecture.md` still
    said Pipelines has "versioned import/export"; settings-wired disabled
    those buttons as Planned (F-H13). It now says so. The same edit had also
    cut the About line in two and left "(sidebar footer), with the version
    and update state beside it" at the end of the new Dictation paragraph;
    put back on the About line.
- **Fixed (own commit `8c040b5`):** removing `mock-dictation` broke the
  SIGKILL replay test from inbox-durable
  (`services/narrivyd/tests/daemon_kill_recovery.rs`): the daemon answered
  `BadRequest: unknown pipeline "mock-dictation"`. The test now starts
  `clean` (same mock lane, same proof). Neither branch could see this alone.
- Note: the mock bridge (`apps/narrivy-desktop/src/lib/bridge.ts:1096`,
  `src/test/mockBridge.ts`) and some history fixtures still use the string
  `mock-dictation` as a sample id. They do not talk to the daemon, so
  nothing fails; left as is.

## fuzz-hardening

- Verdict: **OK.** Property tests (proptest, dev-only, no default features)
  at IPC framing/envelopes, the Unix server, pack manifests, local pack
  copies, model import and language tags. The six fixes are real and
  stricter: pack names checked as written (`a//b` gave `/b`), local pack
  source opened `O_NOFOLLOW | O_NONBLOCK` and must be a regular file, a whole
  frame must arrive within the idle timeout from its first byte, a
  header-only frame is a typed truncation, `AUTO` is automatic, the language
  list is capped while reading. No test weakened. `download.rs` was already
  Unix-only, so the `libc` flags do not affect other targets.
- Owner must know: `proptest` 1.x is a new dev dependency (Cargo.lock +34
  lines); `cargo deny` was not run (not installed here).
  `scripts/fuzz-untrusted-input.sh` is not in CI.

## robustness

- Verdict: **OK.** New `platform-linux::bounded_command` puts a deadline on
  `pactl`, `pw-dump`, `pw-play`, `pw-record`, the assistant command readers,
  the screen helper and the signal-listener join. Error type carries no
  helper output. F-H7: discovery stays before `serve`, with a reason and a
  real-binary test.
- Owner must decide / know:
  - Pipe-reader threads are left to finish on their own when a grandchild
    keeps a pipe open (documented in the module). They are bounded only by
    the grandchild.
  - `AGENTS.md` puts narrow reusable OS mechanics in `process-*` adapters;
    this runner lives in `platform-linux` next to `process-linux`'s own
    group-kill helpers. Not wrong for now, but a candidate to move.
  - Open item from its audit: a meeting recording is not stopped by daemon
    shutdown (`docs/maintainers/robustness-audit-2026-10.md`).

## a11y-i18n

- Verdict: **OK, one fix.** Dialog focus trap and restore, popover Escape,
  combobox palette, live regions, contrast fixes, zoom fixes, and strings
  moved to `en.json`.
- **Missing string keys: none.** A script read every static `t('…')` key in
  `apps/narrivy-desktop/src` (1085 keys, tests excluded) against `en.json`:
  0 missing. The one new dynamic key, `assistant.screen.title.${kind}`, has
  all four values (`record`, `share`, `sound`, `transcript`).
- **Fixed (own commit `e3bcad6`):** the branch darkened light `--live`
  (`#cc2d20`) and the light Gold accent (`#866523` / `#6e5219`) in
  `tokens.css`, but the native overlay keeps its own copy of the palette.
  `overlay_theme::tests::palette_matches_the_app_design_tokens` failed in
  `cargo test --workspace` (the branch ran only Vitest). The overlay now uses
  the same colours, so it gets the same contrast fix.
- Owner must know: items the branch left (composer focus has no ring, the
  dictation state may be announced twice, error toasts are polite, no Orca
  or WebKitGTK check) are in its PROGRESS entry.

## Fixes made on this branch

1. `3d3a528` re-pin three recorder runner hashes (recorders).
2. `0d7fddb` information-architecture: About line, import/export Planned
   (docs-sweep × settings-wired).
3. `8c040b5` kill-recovery test starts `clean` (docs-sweep × inbox-durable).
4. `e3bcad6` overlay palette follows the a11y token colours (a11y-i18n).

## Test results

Run in this cloud container (Ubuntu 24.04, Rust 1.97.1 from
`rust-toolchain.toml`, Tauri system libraries installed as in CI, frontend
`dist` built with `npm run build`). Python tests run with `/usr/bin/python3`
(3.13); the `/usr/local` Python here lacks `fcntl.F_ADD_SEALS`, which the
deb builder tests need.

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo test --workspace --locked` | before fix 4: 1 failed (`palette_matches_the_app_design_tokens`); after: pass, 2290 passed, 0 failed, 6 ignored |
| `cargo test -p narrivyd --features asr-transcribe-resident --locked` | pass: 1430 passed, 0 failed, 2 ignored |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo clippy -p narrivyd --all-targets --features asr-transcribe-resident --locked -- -D warnings` | pass |
| MSRV: `cargo +1.85.0 check --locked -p narrivy-types … -p narrivy-core` (CI `portable-msrv`) | pass |
| `python3 -m pytest tests` | 6 failed, 400 passed (before fix 1: 25 failed); the same 6 as on `cloud/integration-1`, see below |
| `npx tsc --noEmit` in `apps/narrivy-desktop` | pass |
| `npm test` (Vitest) in `apps/narrivy-desktop` | pass: 228 passed, 56 files. One earlier run, while cargo was compiling in parallel, failed the timing test `PerfDictationStartToListening` (PERF-004, <100 ms, took 515 ms); it passed 3 of 3 alone and in the full re-run. Timing flake, not from these branches. |

Python failures, all six also failing on `cloud/integration-1` (and `main`):

- `test_frontend_boundaries.py::LiveFrontendGuardTests` (3 tests):
  hard-coded bundled model id in `Models.tsx`.
- `test_recovery_validators.py::…test_all_legacy_entry_points_and_imports_cannot_write`:
  `BUILD-PACK-MANIFEST.json` drift.
- `test_legacy_deb_builder_quarantine.py::…` and
  `test_phase_report_generation.py::…test_reports_only_attest_to_immutable_release_artifacts`:
  need the gitignored release `.deb` files.

Not run here: `cargo deny` (not installed), the Vulkan and OpenVINO worker
steps, the native qualification scripts, `phase_gate.py` / `release_gate.py`
(known blocked; their failure counts are in gate-trust's PROGRESS entry).
