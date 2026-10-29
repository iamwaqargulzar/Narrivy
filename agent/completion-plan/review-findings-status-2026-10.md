# Review findings: status on 2026-10-29

Every finding in `00-review-findings.md` was checked against the code at `9d5a085`
(`Narrivy 1.0.0-rc.13`). The history is squashed (two commits), so `git log -S`
finds no per-fix commits. The evidence below is file:line and test names from
today's tree. Commit hashes are quoted from `PROGRESS.md`, and those commits
exist only in the owner's local history. No product code was changed.

Status key: **Fixed** (code and test are there), **Fixed, native pending** (code
fixed, the real-machine check is not done yet), **Partly** (some of the finding
is still open), **Open**, **Obsolete** (the code it describes is gone, or the
item moved out of 1.0).

## Critical

| ID | Title | Status | Evidence | What is left |
|---|---|---|---|---|
| F-C1 | Shipped package never types | Fixed | `packaging/deb/narrivy-daemon.service:12,16` set `linux-mic-routed-inbox` / `routed-inbox`. All three unit copies are byte-identical (same md5). Native: dictation typed through the installed unit, rc4 smoke (PROGRESS 2026-10-21) | Nothing |
| F-C2 | Ladder continues after dispatch (double insertion) | Fixed | `adapters/platform-linux/src/delivery/routed.rs:648-667` breaks after `SubmittedUnverified` and after a dispatch failure. Tests `routed_ladder_stops_after_unverified_submission` (:1564), `routed_ladder_stops_after_dispatch_failure` (:1610) | Nothing |
| F-C3 | No insertion path on GNOME | Obsolete for 1.0 | Moved to side quest SQ1 (owner, 2026-09-28) | SQ1 only |
| F-C4 | `wtype` not a dependency | Fixed | `packaging/deb/control:12` and `scripts/build_deb_installable.py:103` have `Recommends: … wtype`. The `wtype_missing` text has a test in `ResolvedCardTruth` | Nothing |
| F-C5 | Bundled model can't be installed by a user | Fixed | `adapters/platform-linux/src/model_import.rs:26-32,269-286,645-703` add the packaged-source rule. Tests `packaged_source_owned_by_the_system_owner_imports`, `packaged_source_rejects_any_unsafe_component`. Native: installed from the app on rc4 | Nothing |
| F-C6 | Installed mode can't complete a dictation | Fixed | `services/narrivyd/src/daemon/session.rs:4156-4163`: only the sidecar branch needs `job_model_path`. Native: dictation through the installed unit, rc4 | Nothing |
| F-C7 | Audio requirements "proven" on probe-only code | **Open** | `audio_policy.rs` and `pipewire_monitor.rs` are still modules (`adapters/platform-linux/src/lib.rs:36,39`), used only by `examples/p4_*`. FUN-006, FUN-012, REL-005 and REL-009 are still `must`/`linux-1.0`/`blocked` in `spec/requirements-traceability.csv:7,13,81,85`, although D14 = C moves them to post-1.0 | Amend the traceability rows per D14. Restate FUN-011 (picker + no-mic error + replug). Measure PERF-006 on the `recovery.rs` capture thread. Retire or quarantine the probe code |
| F-C8 | `wtype` on COSMIC: conflicting reports | Fixed (settled natively) | Typing works on COSMIC (rc4 and round-2 smoke). The modifier problem since 15 Oct is handled by the opt-in `paste-keyboard` | Nothing |

## High

| ID | Title | Status | Evidence | What is left |
|---|---|---|---|---|
| F-H1 | Capture queue 640 ms | Partly | `adapters/platform-linux/src/recovery.rs:64-65`: 64 chunks = 5.1 s, documented at :55-63. No test with a stalled consumer was found | The 10 s bound and a stall test (2 s sleeping consumer). Low risk for 1.0: tiny.en is batch, not streaming |
| F-H2 | Mic loss discards the dictation | Fixed, native pending | `crates/narrivy-core/src/recovery.rs:263,573` (`SOURCE_ENDED_MIN_SAMPLES`). Tests `batch_source_ended_after_enough_audio_keeps_the_text_undelivered` (:2512), `controlled_streaming_source_ended_keeps_the_text_undelivered` (:4709), `a_dictation_whose_microphone_disappeared_is_kept_undelivered` (`retained_history.rs:804`) | Native USB unplug mid-dictation (smoke round 2, step D) |
| F-H3 | Stops after 1.5 s of silence | Fixed | `session.rs:4357` `endpointing: None` | Nothing |
| F-H4 | Hotkey silent when the daemon is down | Fixed | `apps/narrivy-cli/src/main.rs:256-257` (systemctl, gdbus) and `mod start_tests` (:1252). Native: the shortcut started a stopped daemon (round 2, step B) | Nothing |
| F-H5 | Unit can start before the graphical session | Fixed | `narrivy-daemon.service:3-4,51` (`After=`, `PartOf=`, `WantedBy=graphical-session.target`). Native: logout and login on rc4 | Nothing |
| F-H6 | Unit never run natively | Fixed | rc1 failure found and fixed (mount sandbox dropped). rc4 runs the packaged unit on `narrivytest` (PROGRESS 2026-10-20, 2026-10-21) | Repeat in the WP5 campaign on the frozen candidate |
| F-H7 | Daemon unreachable 10–40 s after start | Fixed | `adapters/platform-linux/src/compute_probe.rs:107-109` drains stdout on a thread, capped at `STDOUT_CAP`. Commit 4b2c651 (PROGRESS) | Compute discovery still runs before `serve` (`services/narrivyd/src/main.rs:759-770`); acceptable now that the probe is fast |
| F-H8 | Non-ASCII text fails in `wtype` | Fixed, native pending | `adapters/process-linux/src/lib.rs:1989` sets `LC_ALL=C.UTF-8`; test asserts it (:3635) | Native "café — it's done" (round 2, C3; the model wrote no é or dash in step A) |
| F-H9 | Clean mode corrupts ordinary text | **Partly** | ITN is gone (`services/narrivyd/src/correct.rs:14-19`). `fix_space_after_punct` (:25-35) still adds a space after `, . ; : ! ?` before any alphanumeric. Clean → `CorrectionStrategy::FullCorrect` → `correct::correct` (`daemon/policy.rs:143,187`), so `3.14`→`3. 14`, `example.com`→`Example. Com`, `10:30`→`10: 30`, `1,000`→`1, 000` | Only space after punctuation between letters, outside URL, number and abbreviation tokens (or drop the rule). Add the table tests |
| F-H10 | Release daemon tests don't compile / not in CI | Partly | CI runs `cargo test -p narrivyd --features asr-transcribe-resident` (`.github/workflows/ci.yml:105-107`). Python CI runs only `tests/packaging`, `tests/hardening` and five governance modules (ci.yml:38-50,85-86). `tests/governance/test_frontend_boundaries.py` (`LiveFrontendGuardTests`) is not run; the other two named modules no longer exist | Run the full `python3 -m pytest tests` in CI (after the manifest is regenerated, or with that one test deselected) |
| F-H11 | Overlay may take keyboard focus | Fixed | Layer-shell overlay with `KeyboardMode::None` (`apps/narrivy-desktop/src-tauri/src/linux_overlay.rs:378,779`). Native: typed text arrived once in Text Editor, terminal and Firefox | Nothing |
| F-H12 | UI ignores events after a daemon restart | Fixed | `src-tauri/src/session_events.rs:34-38,100-103` keeps `event_seq` rising across reconnects and sends `events_resubscribed`; `SessionContext.tsx:217` handles it | Nothing |
| F-H13 | ~16 Settings controls change nothing | Partly | The `GenericSection`/`FIELD_SPECS` rendering is gone. Settings rows carry Live/Planned status, and Planned rows are disabled (`app/screens/Settings.tsx:158-215`). Pipeline import/export still writes to `$XDG_RUNTIME_DIR/narrivy/pipelines` (`src-tauri/src/bridge.rs:1743-1752`, tmpfs). `ListPipelines` answers from `advertised_pipelines` (`session.rs:5114`), and no daemon code reads that directory beyond seeding it (`main.rs:498`) | Confirm import changes anything (or hide it). Add the "every rendered control is wired or read-only" test; none was found |
| F-H14 | Status pill never shows recording | Fixed (leftover) | `ui/primitives/StatusPill.tsx:25` accepts `listening` | `hooks/usePerfMarks.ts:9` still waits for `recording`, but nothing imports it: delete it. No fake-bridge/daemon state contract test |
| F-H15 | Uninstall/upgrade leave the old daemon | Fixed, native pending | `packaging/linux/deb/prerm` stops the unit per user manager; `postinst:18` try-restart; `postrm:23-24` names `~/.local/state/narrivy`; no `narrivy-migrate`. Test `test_maintainer_scripts_stop_and_reload_running_user_daemons`. rc4→rc5 upgrade done natively | Native remove and reinstall (round 2, C4–C6); `-M <uid>@` has not run as root under dpkg |
| F-H16 | Unit test can type into the focused window | Fixed | `cfg(test)` stand-ins for the clipboard and wtype rungs (commit 844d886, PROGRESS); `session.rs` `#[cfg(test)]` items at :851,:859 | A governance check that no test composes a `System*Rung` (suggested, not done) |
| F-H17 | History on by default + storage defects | Partly | One-statement cap, `secure_delete`, WAL truncate: `crates/narrivy-history/src/history.rs:169-182` (test reads the pragma, :576). Default 7 days (`retained_history.rs:59`) per D6/D11, with a Keep-for/Clear UI | Privacy zones: PRIV-003 is still `must`/`linux-1.0` (`requirements-traceability.csv:95`) while 1.0 has no way to create a zone. Restate it or move it post-1.0 |
| F-H18 | Crash dumps can hold audio/transcripts | Fixed | `LimitCORE=0` (unit :37); `forbid_core_dumps()` at `services/narrivyd/src/main.rs:383` and `engines/transcribe-cpp/src/bin/narrivy-transcribe-worker.rs:34`, plus both OpenVINO workers. Native: `/proc/<pid>/environ` refused on rc4 | `kill -SEGV` + `coredumpctl` check in the campaign |

## Medium

| ID | Title | Status | Evidence | What is left |
|---|---|---|---|---|
| F-M1 | VAD samples vs frames | Fixed | Commit 5f63f39 (PROGRESS): multi-channel audio refused | Nothing |
| F-M2 | Rejected VAD chunk half-updates state | Fixed | Commit 5f63f39: chunk size checked before any state change | Nothing |
| F-M3 | Recovered text on tmpfs, no UI | Fixed by decision (D6) | Inbox is still `runtime_dir.join("inbox")` (`main.rs:503`), pruned after 15 min (`session.rs:739`). History is the durable copy and shows undelivered entries with Copy | If the user sets History to keep 0, recovered text lives only on tmpfs. Accepted by D6; say so in the docs if not already said |
| F-M4 | Every dictation also saved as an Inbox file | Fixed | `InboxStore::prune_older_than`, test `only_old_recovered_text_files_are_removed`; disclosure in Settings and `docs/security/data-retention.md` (PROGRESS 2026-10-21) | Nothing |
| F-M5 | WebKitGTK 6.0 floor on Ubuntu | Obsolete | The Tauri app depends on `libwebkit2gtk-4.1-0 (>= 2.44)` (`packaging/deb/control:11`); Ubuntu work is SQ1 | SQ1 only |
| F-M6 | Package metadata | Fixed | `packaging/deb/control:6,13`; no `Build-Using`; D3 versions. Test `test_package_identity_is_d3` | `packaging/*/control` templates still say `Version: 1.0.0~rc1` and lack `mesa-vulkan-drivers` (the builder writes the real fields; cosmetic). The `narrivy.local` SPDX identifiers go with the supply-chain evidence (P-8) |
| F-M7 | Default model least accurate | **Open** | Unit still `NARRIVY_TRANSCRIBE_MODEL_ID=whisper-tiny.en-Q8_0` (:19). D4 = A (measure `base.en`); no measurement in `PROGRESS.md` or `agent/RESEARCH_LOG.md` | Measure quantized base.en on the reference CPU, decide, and record the licence |
| F-M8 | Failure invisible in the overlay | Fixed | `linux_overlay.rs:106` `ERROR_HOLD` 4 s; the following `Hidden` waits for `hold_until` (:547-553) | The overlay exists only while the desktop app runs (smoke round 2, step B: no overlay or sound from the shortcut alone). This is a new item, not part of F-M8 |
| F-M9 | Full UI queue stops events forever | Obsolete | The GTK shell has been removed. The Tauri forwarder has no bounded queue: it stops only when `app.emit` fails at teardown (`src-tauri/src/lib.rs:189-193`) | Nothing |
| F-M10 | Status polls stack up | Fixed | Single-flight poll, commit c373091 (PROGRESS) | Nothing |
| F-M11 | First run is a dead end | Fixed | Six sub-items fixed with tests (`BundledModelInstall`, `LiveStatusStrip`, `HomeEditStale`, `DictationLatchShared`, `PaletteOpensAndExecutes`; PROGRESS 2026-10-21) | Nothing |
| F-M12 | Identity mismatches | Partly | One version: `1.0.0-rc.13` in `services/narrivyd`, `apps/narrivy-cli`, `src-tauri/Cargo.toml` and `tauri.conf.json:4`, checked by the release script | App ID `app.narrivy.shell` (`tauri.conf.json:5`) vs `StartupWMClass=narrivy-desktop` (`packaging/deb/narrivy.desktop:9`): check the window's app ID live (native) |
| F-M13 | Hand-started daemon runs the mock | Fixed | The daemon refuses to start without `NARRIVY_ASR_ENGINE`. Test `a_daemon_without_an_engine_choice_refuses_to_start` (`services/narrivyd/tests/daemon_shutdown_signals.rs:427`) | `DEFAULT_PIPELINE_ID` is still `"mock-dictation"` (`policy.rs:33`), now harmless |
| F-M14 | Builder accepts any `narrivyd` | Fixed | `narrivyd --build-features` check; test `test_only_a_resident_daemon_is_packaged` (`tests/packaging/test_build_deb_installable.py:339`) | Nothing |
| F-M15 | Legacy router reachable with `platform` | Fixed | Backend removed from `narrivyd`; `router` is test-only (`adapters/platform-linux/src/delivery.rs:25-35`) | Nothing |
| F-M16 | Test gaps | Partly | Fixed: support-report/History race, the recursive no-OS-branching guard, the deterministic kill-mid-migration test | Builder `build()`/`_inspect` success path still untested (no such test in `test_build_deb_installable.py`). Qualify fakes still `return ok()` (`tests/packaging/test_qualify_candidate.py:49-113`). `daemon_shutdown_signals.rs:104` uses the mock engine. Frontend model tests mock the bridge (the native rc4 install covers the real path). Process-linux blocking tests not re-checked |
| F-M17 | Raw env values on stderr | Fixed | No `{other:?}`-style echoes in `daemon/policy.rs`; commit a01aeca | Nothing |
| F-M18 | No SIGKILL survival test | **Open** | No strict-lane kill test in `services/narrivyd/tests` (only `daemon_shutdown_signals.rs`, no kill-and-replay). PROGRESS 2026-10-21: a mock-lane test cannot prove replay (`delivery_intent: null`) | Strict-lane subprocess test with fixture audio and a fake engine: kill after `Committed`, restart, text replayed or kept |
| F-M19 | History has no clear / real retention | Fixed | `Command::SetHistoryRetention` / `ClearHistory` (`session.rs:5792,5824`); `HistorySettings` UI and test | Nothing |
| F-M20 | Onboarding/Models smaller than the spec | Partly | Model-install step and "Set a shortcut" step in `app/Onboarding.tsx:30-46,112-114` (tests in `OnboardingCompletes`) | Mark the rest of `docs/product/ux/onboarding.md` and the models screen spec as roadmap (WP4 T4.7); neither file says roadmap or 1.0 |

## Low

| ID | Title | Status | Evidence | What is left |
|---|---|---|---|---|
| F-L1 | Dead FFI stub with `unsafe` | Fixed | `engines/transcribe-cpp/transcribe-sys/` is gone | The manifest still has to be resealed (see the gate) |
| F-L2 | Legacy router unused | Fixed | Test-only (`delivery.rs:25,35`) | Nothing |
| F-L3 | Very large files | Open (post-1.0) | `session.rs` 11,135 lines, `resident.rs` 6,253, `core/recovery.rs` 5,895 | Post-1.0 only |
| Low list | Portal/EIS, sidecar, streaming-only, `TriggerRouter`, etc. | Not re-checked | All of these are on paths that 1.0 does not run, as the register says | Fix only if cheap |

## Process and release-pipeline findings (quick check)

| ID | Status | Evidence / what is left |
|---|---|---|
| P-1 | Not re-checked | `phase_gate.py` still requires a passing `agent/phase-reports/<P>.json` for HEAD; the gate shows "P11 report commit mismatch" |
| P-2 | **Open** | `agent/evidence/P10/` still has `p10-deb-build.json`, `p10-deb-install.json` and other `p10-*` files next to `VER-OPS-00x.json` |
| P-3 | **Open** | `spec/evidence-verification-profiles.json:2531` pins `74214c49…` for `scripts/record_controlled_integration_evidence.py`; the file is `068087f2…` |
| P-4 | Not re-checked | Recorder defects (WP4 T4.5). The gate's 140 unbound assertions are consistent with it |
| P-5 | Not re-checked | Gate trust in self-declared fields (WP6 T6.0) |
| P-6 | Fixed | The 12 `scripts/refresh_p*_evidence.py` are quarantined (exit 2) |
| P-7 | Partly | `agent/CODING_AGENT_UPDATE_PROMPT_2026-08-24.md` is gone; the other stale docs were not re-checked |
| P-8 | **Open** | `scripts/build_candidate_supply_chain.py:582-585` still writes `NOASSERTION` licences; `THIRD_PARTY_NOTICES.md:16,23` still says "not bundled" for parts that are |
| P-9 | Fixed | `agent/evidence/P0F/p0f-mic-pipewire.json:67` is redacted |
| P-10 | Not re-checked | Doc sweep (WP4 T4.7) |

## Open findings that block the Linux 1.0 release gate

`python3 scripts/release_gate.py --target linux-1.0` today: exit 1, 848 lines.
- `validate_pack.py` fails on the manifest: 129 unlisted files, about 183 changed.
- 54 requirements are `blocked`.
- 140 assertions have no measurement binding (14 requirements).
- 57 artifact hash mismatches (34 requirements).
- 39 rows have evidence from an older commit.
- 37 artifact paths are missing (no `artifacts/releases/narrivy-linux-amd64.deb`).
- There is no release-evidence directory, and the P11 report does not pass.

Most of this is the WP5/WP6 evidence campaign, not a code defect. The open
findings below each keep specific gate rows red. They are ranked by how much
of the gate they hold, then by how cheap they are to fix.

1. **F-C7**: 7 gated rows: FUN-006, FUN-007, FUN-011, FUN-012, REL-005, REL-009, PERF-006. The D14 traceability amendments are code/docs-only. Writing the rows is the agent's job under D14, but they change release scope, so get the owner's go-ahead. FUN-011 replug and PERF-006 need **hardware**.
2. **Manifest drift** (not an F-finding; it blocks gate step 1 and the F-L1 reseal): **code-only**, but `build_manifest.py` is a separately reviewed packaging step. Run it on the frozen candidate commit.
3. **F-H17 (privacy zones)**: PRIV-003 is a gated `must` that 1.0 cannot meet. Owner restatement or move post-1.0: **docs-only**.
4. **F-M18**: holds REL-006 and TEST-006 (both blocked). Strict-lane SIGKILL test: **code-only**.
5. **P-3**: SEC-004 evidence is always rejected until the runner is re-pinned: **code-only** (reviewed commit).
6. **P-8**: OPS-004 (SBOM licences, model licence and notices): **code-only**, plus D9.
7. **P-2**: duplicate P10 evidence IDs block the P10 phase gate: **code-only** (move to `agent/history/`).
8. **F-H9**: Clean is the default pipeline, and FUN-021's evidence must be re-recorded anyway. Shipping `3. 14` / `example. Com` would be a visible defect: **code-only**.
9. **F-M7**: D4 measurement before the default model is frozen for the campaign (PERF and FUN-003 known-phrase evidence depend on it): **hardware** (reference CPU).

Not gate rows, but finish before the campaign. All code-only:
- F-H10: full pytest in CI.
- F-M16: builder success-path test.
- F-H13: pipeline import/export check.
- F-M20: roadmap marking in the docs.
- F-H1: stall test.
- F-H14: delete `usePerfMarks.ts`.

**Native checks only (code done, needs the reference machine):**
- F-H2: USB unplug.
- F-H8: non-ASCII typing.
- F-H15: remove and reinstall.
- F-M12: live app ID.
- F-H18: SEGV/coredumpctl.
- F-H6/F-C5/F-C6: repeat on the frozen candidate in WP5.

## Status entries that look wrong (listed, not edited)

- `spec/requirements-traceability.csv:7,13,81,85`: FUN-006, FUN-012, REL-005 and REL-009 are still `must`/`linux-1.0`, while D14 = C (`OWNER-DECISIONS.md:220`, `02-product-fixes.md:375-387`) moves them post-1.0. FUN-007 and FUN-011 still carry the old wording, not the D14 restatement.
- `spec/requirements-traceability.csv:95`: PRIV-003 (privacy zones) is still a gated `must`, against F-H17's "don't claim privacy zones for 1.0". This one needs an owner decision, not just an edit.
- `agent/STATE.json` `last_gate_result` (line 9) describes the 2026-10-19 run (614 lines). Today's run is 848 lines: 57 hash mismatches in 34 requirements, 37 missing paths, 129 unlisted / about 183 changed manifest files. It is dated, so it is stale rather than false; refresh it at the next checkpoint.
