# WP4 — Make every release requirement qualifiable

**Goal:** every one of the 93 release-gated requirements has a registered,
reviewed procedure whose evidence the release gate can accept. Evidence
itself is **not** recorded in this WP; that happens in WP5 on the frozen
commit. Here you only build and register the procedures.

**How procedures work (read this first):**

- Procedures are registered in `spec/evidence-verification-profiles.json` →
  `procedures[]`. Each has `id`, `verification_id` (for example
  `VER-FUN-004`), `profile_id` (one of `unit-contract-v1`,
  `controlled-integration-v1`, `native-behavior-v1`, `package-artifact-v1`),
  `command_argv`, an optional `runner {path, sha256, argv_index}` that pins the
  recorder script, and `required_assertions`.
- The release gate (`scripts/evidence_policy.py`) accepts evidence only if:
  - it is schema v2 or v3;
  - `git_commit` equals `HEAD`;
  - the pinned runner's SHA-256 still matches;
  - **every required assertion has a measurement binding**:
    `{"id": ..., "measurement": {"metric": "<dotted.path>", "comparator": "eq|le|ge|...", "expected": ...}}`,
    and the recorded metrics satisfy it.
- Look at a working, measured example before writing a new one:
  `VER-FUN-005` (unit-contract) and the procedures that use
  `scripts/record_cpu_performance_evidence.py` (native, measured). The
  measurement rules are tested in `tests/governance/test_evidence_measurements.py`.
- After changing any recorder script, update its `runner.sha256` in **every**
  procedure that uses it. Then run `python3 -m pytest -q tests/governance`.

Do all of WP4 before the freeze in WP5. Changing a recorder after the freeze
invalidates the campaign.

---

## T4.1 — Convert the 6 recorders that still use unmeasured assertions

These 27 procedures list `required_assertions` as plain strings, so the gate
reports "assertion declares no measurement binding" for their evidence even
when the product works:

| Recorder script | Procedures |
|---|---|
| `scripts/record_native_ui_dictation_evidence.py` | 15 (FUN-002, FUN-008, FUN-009, FUN-010, UX-001, UX-004, UX-005, UX-006, PERF-002, PERF-003, PERF-004, REL-002, REL-006, TEST-006, SEC-002) |
| `scripts/record_native_audio_policy_evidence.py` | 7 (FUN-006, FUN-007, FUN-011, FUN-012, REL-005, REL-009, PERF-006) |
| `scripts/record_native_recognition_evidence.py` | 2 (FUN-028, FUN-029) |
| `scripts/record_cosmic_vertical_slice_evidence.py` | 1 (TEST-009) |
| `scripts/record_package_lifecycle_evidence.py` | 1 (OPS-002) |
| `scripts/record_package_upgrade_evidence.py` | 1 (OPS-003) |

For each recorder:
1. Read its current assertion logic.
2. For each string assertion, decide the measurable fact behind it (for
   example `local-asr-known-phrase` → `metrics.known_phrase.matched eq 1`;
   `daemon-clean-shutdown` → `metrics.shutdown.duration_ms le 3000` and
   `metrics.shutdown.leftover_processes eq 0`).
3. Make the recorder emit those metrics in the evidence `metrics` object
   (schema v3; match the shape the CPU recorder emits).
4. Replace the string assertions in the profile JSON with measured bindings.
   Comparators must express the requirement's acceptance text. **Never
   loosen a threshold** to make a known run pass. Performance targets come
   from `docs/product/requirements.md` and
   `docs/maintainers/qualification/performance-benchmark-protocol.md`.
5. Update the `runner.sha256` pins and run the governance tests.
   After all six recorders are converted, tighten
   `spec/schemas/evidence-verification-profiles.schema.json:91` so a bare
   string is no longer a valid `required_assertions` item. The schema then
   rejects unmeasured assertions at commit time, instead of the gate
   rejecting them at release (Sol S17).
6. Dry-run the recorder once against the current candidate, into a scratch
   output path, not `agent/evidence/`. This proves it runs end to end and
   emits every metric. Record the result in `PROGRESS.md`. (Some need the
   owner present: microphone playback and permission prompts.)

Also check that these recorders still match the product after WP2. For
example, the UI dictation recorder must now expect text in the target app
(routed lane), not Inbox-only.

## T4.2 — Add procedures for the 10 requirements that have none

For each requirement below, write a recorder (or extend an existing one when
the evidence comes from the same run) and register a measured procedure.
Acceptance text is quoted from `spec/requirements-traceability.csv`.

| Req | Acceptance (short) | Suggested approach |
|---|---|---|
| **FUN-001** | Toggle activation reliable; PTT only where proven; capability truthful | Native: 50 toggle cycles via `narrivy-cli toggle` (the documented activation path), with the desktop shortcut bound. Metrics: `cycles`, `failures eq 0`, `orphan_sessions eq 0`; the capability report advertises toggle, and advertises hold only if T2.13 shipped it. If T2.13 shipped hold, add 50 hold cycles with a measured release-to-stop latency and `failures eq 0`. If T2.13 moved hold to a side quest, FUN-001/FUN-031 text must say so, backed by a recorded owner decision. Extend `record_native_ui_dictation_evidence.py` or add `record_native_activation_evidence.py`. |
| **FUN-013** | Deterministic target-aware spacing and punctuation, no blind whitespace changes | Mostly unit-contract: find the formatting code (`rg -n "spacing\|cursor" crates adapters/platform-linux/src/delivery`). Add or bind existing tests to `record_unit_contract_evidence.py`. If no target-aware logic exists, this is a **product gap**: tell the owner and either implement minimal "no leading space at line start; single space between dictations" logic, or ask for a scope decision. |
| **PERF-001** | Measure hotkey→listening, speech→partial, release→raw, release→committed separately | The daemon already records stage timings (`RecoveryTimingTrace`, `narrivy-cli` prints `StageTimingsUs`). Native recorder: N dictations; emit each stage's p50/p95 as metrics; assert that every stage was measured (`count ge N`). |
| **PERF-009** | Avoid unnecessary device transfers and duplicate audio conversion | Unit/controlled: prove capture f32 16 kHz mono flows to the worker without resampling or re-encoding. For example, a test that counts conversions on the path, or that the worker receives the capture's buffer format unchanged. Bind it to the unit-contract recorder. |
| **PERF-010** | Prepare context/model/output concurrently | The resident worker is warm before a session (model preloaded), and capture starts before ASR open (`core/recovery.rs:706`). Controlled test asserting the ordering and overlap using the timing trace; bind to the controlled-integration recorder. |
| **REL-004** | Suspend/resume and device hotplug are supported lifecycle events | Native, owner-present: (a) `systemctl suspend`, resume, then dictation works; (b) unplug/replug the USB mic, then the ranking/hotplug log updates and the next dictation works. With T2.7, unplugging mid-dictation saves text. |
| **PORT-006** | Compute backends are replaceable modules without core coupling or a public Rust dylib ABI | No accelerator hardware needed (AMD is a post-release side quest, see `07-post-release-side-quests.md`). Unit/architecture: the dependency-boundary scripts already enforce that the core has no engine dependency (`scripts/verify_dependency_boundaries.py`). Bind their exit codes, plus a test proving a second compute module can be registered, to the unit-contract recorder. |
| **TEST-004** | Linux integration tests exercise trigger, capture lifecycle, audio restoration, delivery routing, target-app compatibility on reference sessions | Composite: satisfied by a native run that executes FUN-001, the audio-policy recorder, and the target-app matrix (T4.3). The procedure's metrics reference those sub-results. |
| **TEST-010** | Release-artifact E2E on Pop!_OS COSMIC/Wayland, plus CPU fallback (acceptance amended 2026-09-28) | The final composite (see WP6 T6.2): requires the passed TEST-009-style COSMIC run and the CPU performance run with the same package hash. |
| **OPS-005** | Release notes list verified environments/capabilities and known limitations; nothing untested presented as supported | A generator script (`scripts/generate_release_notes.py`) that builds `summary.md` **only** from passed evidence (lanes, capabilities, target-app results), plus a check that every claim maps to a passed evidence record. See WP6. |

Remember:
- a new requirement mapping must keep `python3 scripts/verify_traceability.py`
  passing;
- `scripts/phase_gate.py --phase <P>` for the requirement's phase must
  recognise the new procedure;
- run `python3 -m pytest -q tests/governance` after each registration.

## T4.0 — Make the recorders able to run in one campaign (do this first)

**Problem (verified 2026-09-28):** the release gate needs evidence with
`git_commit == HEAD` and `git_dirty == false`
(`scripts/evidence_policy.py:451–454`). Several recorders refuse to start
unless `git status --porcelain` is **completely** empty, for example
`record_unit_contract_evidence.py:298`,
`record_controlled_integration_evidence.py:456` and
`record_native_audio_policy_evidence.py:51`. Every recorder then writes
tracked files (`agent/evidence/**`, the evidence `INDEX.json`, and the CSV via
`mark_requirement.py`).

So after the first recorder runs, the tree is dirty and the next clean-tree
recorder refuses. The only workaround is committing evidence between runs.
That changes `HEAD` and invalidates everything already recorded. This is why
every current "passed" record fails with "evidence commit does not match".
The campaign in WP5 is impossible until this is fixed.

**Fix:**
1. Add one shared helper, for example in `scripts/evidence_policy.py`:
   `source_tree_clean() -> bool`. It runs `git status --porcelain` and
   ignores only the evidence-output paths:
   - `agent/evidence/`
   - `spec/requirements-traceability.csv`
   - `BUILD-PACK-MANIFEST.json`
   - `agent/phase-reports/`
   - `artifacts/` (already ignored by git; keep it out regardless)

   Any other change (source, scripts, profiles, packaging) must still make it
   return false.
2. Replace every recorder's inline clean check with the helper. Find them
   with `rg -n '"status", "--porcelain"|status --porcelain' scripts/record_*.py`.
3. For the unit-contract and controlled recorders' **post-run** check
   ("suites changed the worktree"), compare the porcelain output **before and
   after** the suites, so that pre-existing evidence changes are tolerated but
   any change made by the suites is not.
4. Add governance tests:
   - a change under `agent/evidence/` alone counts as clean;
   - a change to any `.rs`, `scripts/*.py` or
     `spec/evidence-verification-profiles.json` counts as dirty;
   - an untracked file outside the allowed paths counts as dirty.
5. Record the decision in `agent/DECISIONS.md`: the evidence contract still
   binds every record to an unchanged source commit; only evidence outputs may
   differ.
6. Update every changed recorder's `runner.sha256` pin.

This does **not** weaken the gate. The source under test is still exactly
`HEAD`.

## T4.3 — Target-application matrix

`docs/maintainers/phases/phase-11-qualification.md` and
`spec/schemas/linux-qualification.schema.json` (field `target_matrix`) require a bounded target-app
matrix: GTK, Qt, Firefox, Chromium, Electron, terminal, LibreOffice and
XWayland.

- Read the schema's `target_matrix` definition and `scripts/release_gate.py` step 6 first.
- Write `scripts/record_target_app_matrix.py`. It launches each installed app,
  focuses a text field, runs one dictation of the canonical fixture (the same
  audio-playback approach TEST-009 uses), and reads the result back through
  AT-SPI or the app's own save file. It records per-app `inserted_once`,
  `unicode_ok` (use a short fixture with non-ASCII text if the model supports
  it; otherwise record the capability as not claimed), and `inbox_copy_written`.
- The gate (`scripts/release_gate.py`, `REQUIRED_TARGET_CLASSES`) needs a
  row **with evidence** for all 8 classes: `gtk`, `qt`, `firefox`,
  `chromium`, `electron`, `terminal`, `libreoffice`, `xwayland`. `result` may
  only be `supported`, `degraded` or `unsupported`; there is no "not tested"
  value. So every class must actually be run. `unsupported` is allowed, but
  must state the fallback in `delivery_outcome` (for example "text saved to
  Inbox; copy from Home").
- Ask the owner to install one representative app per class on each lane
  (a one-time step) on the Pop!_OS machine; every row uses `environment_kind: cosmic-wayland`. Suggested: gedit or GNOME Text Editor (gtk), KWrite or
  Qt Designer (qt), Firefox, Chromium, VS Code or another Electron app, the
  desktop's own terminal, LibreOffice Writer, and xterm (xwayland).

## T4.4 — Release-evidence generators (needed by WP6, written now)

WP6 runs after the freeze, when no script may change. Write, test and commit
these now:

- `scripts/generate_release_artifacts.py` → `release-artifacts.json` from the
  supply-chain bundle (WP6 T6.1).
- `scripts/generate_qualification.py` → `qualification.json` derived only from
  passed evidence (WP6 T6.2).
- `scripts/generate_release_notes.py` → `summary.md`, used by OPS-005 (WP6 T6.3).

Each needs unit tests with fixture evidence. Each must fail closed: exit
non-zero instead of writing a file when an input record is missing, blocked,
or bound to another commit or package hash. Validate the output against its
schema inside the script.

## T4.5 — Fix recorder defects found in the second pass (P-4)

Do these together with T4.1. Each is a recorder change, so re-pin the runner
hash afterwards.

1. `record_native_hardening_evidence.py:376`: emit `record["assertions"]`
   directly. It currently wraps each assertion dict as an ID, so the record
   is always schema-invalid.
2. Recorders that refuse to replace their own indexed v2 outputs:
   - `record_native_hardening_evidence.py:129-137`
   - `record_supply_chain_evidence.py:266-273`
   - `record_package_lifecycle_evidence.py:90`
   - the same pattern in the six T4.1 recorders

   Allow a guarded replacement: the existing record must be hash-matching,
   indexed and v1/v2. Replace the file and its INDEX entry in one transaction.
3. One shared native-lane validator in `scripts/`. It requires:
   - `/etc/os-release` Pop!_OS 24.04;
   - `XDG_CURRENT_DESKTOP` containing COSMIC;
   - a Wayland session with `WAYLAND_DISPLAY` set;
   - `systemd-detect-virt` returning `none`;
   - no container markers.

   Use it in every recorder that writes `native_or_vm: native`. Four
   currently hard-code it or trust environment strings:
   `record_cpu_performance_evidence.py:339-355`,
   `record_native_accessibility_evidence.py:675-740`,
   `record_native_audio_policy_evidence.py:80`,
   `record_native_hardening_evidence.py:150`. Also
   `qualify_candidate_known_phrase.py:773`.
4. Log privacy scans:
   - keep the actual transcript in memory only;
   - scan logs for normalized transcript n-grams and injected canaries;
   - an unreadable or missing log is a failure, not a clean result.

   Affected: `qualify_candidate_known_phrase.py:350,1091`,
   `record_native_recognition_evidence.py:246`,
   `record_native_ui_dictation_evidence.py:939`.
5. `record_cpu_performance_evidence.py:457`: inject one controlled worker
   failure and observe the fallback. Don't set `failed_decodes = 0` by
   construction.
6. "Test ran" proofs:
   - use structured results: `cargo test -- -Z unstable-options --format json`
     is unavailable on stable, so use `--nocapture` plus the `test <name> ...
     ok` lines, or nextest JUnit, and Vitest's JSON reporter;
   - require `ok`/`passed` for each named test. A skipped test is a failure.

   Affected: `record_controlled_integration_evidence.py:172`,
   `record_unit_contract_evidence.py:99`,
   `record_native_accessibility_evidence.py:266`.
7. `record_native_ui_dictation_evidence.py`:
   - PERF-002 compares `installed_toggle_ack_ms` (line 1034), not the
     visible-feedback time;
   - reject non-monotonic stage timestamps and negative deltas (line 646);
   - one outer `try/finally` owns the daemon and desktop processes (line 780).
8. `record_package_upgrade_evidence.py`:
   - start the daemon and check `narrivy-cli status` after install, upgrade,
     rollback and re-upgrade;
   - pin the previous package by hash.
9. `record_native_hardening_evidence.py`:
   - REL-008 runs the packaged `narrivy-cli support-report` against the
     packaged daemon, with canaries;
   - SEC-005 includes a live negative connection from a different UID. This
     needs `sudo -u nobody`, so it's an owner step.
10. `record_cosmic_vertical_slice_evidence.py:134,159`: scenario ID
    `p6-e2e-beta` and the hash of `spec/scenarios/p6-e2e-beta.json`.
11. `record_supply_chain_evidence.py:162-175`: reject `NOASSERTION` for
    shipped components, and check SBOM checksums against `Cargo.lock`. Needs
    T6.4's complete inventory (P-8).
12. Timeouts and process-group cleanup for every recorder that spawns suites
    or helpers:
    - `record_controlled_integration_evidence.py:464`;
    - `record_cpu_performance_evidence.py:420`, which must also stop the
      worker in `finally`;
    - `record_native_audio_policy_evidence.py:65`;
    - `qualify_installed_model.py:273`, which must also cancel the provision
      on timeout;
    - `record_native_accessibility_evidence.py:340`, which must pick the
      AT-SPI app by PID, never close the user's own Narrivy window.
13. `qualify_candidate_known_phrase.py` (Sol B25): playback of the known
    phrase starts as soon as the CLI accepts the session, not when capture
    is ready. That is a likely cause of the recorded known-phrase misses.
    Wait for the daemon's `listening` state (capture ready) before playing,
    and record the gap as a metric. Update
    `tests/packaging/test_qualify_candidate_known_phrase.py:197`, which models
    session start as instantly ready.
14. Test honesty in `tests/` (Sol B23/B25):
    - `test_phase_report_generation.py:97` must require every implemented
      phase's JSON report, including P11;
    - `:302` must stop blessing an arbitrary `--gate-command` (with P-1,
      remove the override or restrict it to tests);
    - `test_evidence_measurements.py` needs NaN/Infinity cases and a real
      commit check;
    - `test_build_candidate_supply_chain.py:326` must stop requiring
      `NOASSERTION` licences (P-8).
15. `qualify_candidate_known_phrase.py:251`: after the daemon leader exits,
    check that its process group is empty. A leftover `pw-record` would keep
    the microphone open.

**Check:** `python3 -m pytest -q tests`, one scratch dry run per changed
recorder, logged in `PROGRESS.md`.

## T4.6 — Unblock the gate plumbing (P-1, P-2, P-3)

1. **P-1, the phase-report cycle:**
   - split `scripts/phase_gate.py` into a qualification mode (commands,
     evidence, owner checks; no report check) and the full mode (adds report
     validation);
   - `generate_phase_report.py` runs the qualification mode;
   - WP6 then runs the full mode.

   Add a governance test that simulates a new commit: generate, then the
   full gate passes. Never use `--gate-command` to route around this.
2. **P-2, duplicate evidence IDs:**
   - write a one-off inventory that groups `agent/evidence/**/*.json` by
     `(phase, verification_id)`;
   - move every historical duplicate (for example
     `agent/evidence/P10/p10-deb-install.json` and `p10-deb-build.json`) to
     `agent/history/<date>/` with a README, as was done for PORT-001
     (P10 done 2026-10-29: `agent/history/superseded-p10-evidence-2026-10-29/`);
   - add a governance test: exactly one live record per `(phase,
     verification_id)`.
3. **P-3, SEC-004 runner pin:**
   - re-pin `controlled-sec-004-v1` in
     `spec/evidence-verification-profiles.json` to the current
     `scripts/record_controlled_integration_evidence.py` hash (after T4.5
     changes);
   - add a `doctor.py` or governance check that every pinned `runner.sha256`
     equals the file on disk, so pin drift fails at commit time instead of
     at the release gate.

4. **Owner gates and acceptance files that can never pass** (DeepSeek D01
   and Sol S16, confirmed):
   - `spec/gates/owner/P10.json:7`, `spec/gates/owner/P11.json:7`,
     `spec/acceptance/P10.json` and `spec/acceptance/P11.json` invoke
     quarantined stubs that always exit 2 (`tools/narrivy-qualify/p10_runner.py`,
     `p11_runner.py`, `scripts/build_deb.sh`, `scripts/generate_sbom.sh`).
     Replace them with the reviewed builders and recorders
     (`build_release_candidate.sh` from T3.1,
     `build_candidate_supply_chain.py`, and the WP4 recorders);
   - `spec/acceptance/P10.json:15` names
     `artifacts/releases/narrivy_0.1.0_amd64.deb`, while every procedure uses
     `artifacts/releases/narrivy-linux-amd64.deb`. Use one path;
   - retire, or mark non-release, the legacy scenarios whose runners pass on
     mock ASR, generated audio or a null sink: `p2-trigger-cosmic`,
     `p2-e2e-native-wayland`, `p2-inbox-fallback`, `p2-cpu-fallback`,
     `p10-deb-install`, `p0f-activation-cosmic`. No owner gate for a
     release-gated phase may reference them.

## T4.8 — Audit what each procedure actually proves (second pass, High)

Sol's pass over `agent/evidence/` found "passed" procedures whose proof tests
something other than the requirement. Measurement bindings don't catch this,
because the metric is measured, just of the wrong thing. Examples:
- **VER-FUN-031:** counts duplicate audio-capture nodes as toggle/hold
  isolation, and a remote-processing test as capability-gated hold.
- **VER-PRIV-004:** its cited test covers capture duration and end reasons,
  not cloud disclosure.
- **VER-PRIV-007:** checks only default denial. There is no context
  permission class.
- **VER-FUN-064 and VER-PRIV-003:** zone and context tests with no production
  caller.
- **VER-SEC-007:** registry "integrity" tests only reject unsafe path
  components.
- **VER-FUN-032:** doesn't show deterministic correction is separated from
  LLM output.
- **VER-FUN-033:** a navigation smoke test stands in for module substitution.
- **VER-TEST-005:** failure and fallback are never exercised (P-4 item 5).
- **VER-OPS-006:** an in-process SQL error on the unused core `Store`
  (P-4/B29).
- **VER-FUN-041:** dictionary unit tests only, with no production wiring
  (D13).

For **every** procedure in `spec/evidence-verification-profiles.json`, write
one line in `PROGRESS.md`: requirement text → what the bound test or metric
actually exercises → "matches" or "gap". For each gap, either rebind to a
test that exercises the product path, write that test, or (for features
absent in 1.0) take it to the owner through D13/D14. Requirements about
remote or cloud data (FUN-064, PRIV-004, PRIV-007) should be proven by
measuring that no network client or remote provider is composed in the
release build (for example a dependency-graph check plus a startup
capability assertion), not by tests of unused gates.

Also:
- **OPS-002/OPS-003:** the approved package procedures run in an Ubuntu
  24.04 container (`spec/evidence-verification-profiles.json:128` and the
  OPS-003 entry). Keep the container run as a reproducible package-artifact
  check, but label it as that. Add a native Pop!_OS lifecycle record (install,
  first run, upgrade, remove on the reference machine), so the release lane
  is proven on its actual OS.
- **OPS-005:** there is no schema-v2/v3 procedure yet. It's in T4.2's list;
  make sure T4.4's `generate_release_notes.py` feeds it.

## T4.7 — Documentation truth sweep (P-10, D-series findings)

Fix each document so it matches the code, or mark it as a future design note:
- `docs/roadmap/design/error-code-registry.md` (`NAR-*` codes don't exist);
- `docs/roadmap/design/observability.md` (counters don't exist);
- `archive/docs/doctor-plan.md` (fields `doctor.py` doesn't emit);
- `docs/maintainers/operations/reproducible-builder.md:64,67` (nonexistent
  `packaging/build-deb.sh`, and `release_gate.py --dry-run`);
- `docs/security/secrets.md` and PRIV-005: 1.0 stores no secrets,
  so say that;
- `docs/product/ux/screens/*.md` (36 test IDs that don't exist: replace them
  with real test names);
- `docs/product/ux/overlay.md:25` (cites a missing file);
- `spec/ui/design-tokens.json` (orphan: generate `tokens.css` from it, or
  delete it);
- `docs/architecture/pipeline-catalog.md:5` and
  `docs/product/ux/modes-profiles.md:5` (the default is `clean`, not
  `fast-dictation`);
- `docs/architecture/README.md:31` (`asr-mock` is `default-fallback`);
- `docs/architecture/audio-pipeline.md:28` (1500 ms, not 560);
- `docs/architecture/audio-architecture.md:41` (Linux has `leave` and
  `duck` only);
- `docs/maintainers/phases/phase-09-hardening.md:7` (SEC-001/003/006 are now R14);
- `docs/maintainers/phases/phase-10-packaging.md:21-29` (no AppImage);
- ADR-0002 and ADR-0028: superseded by ADR-0064, recording the GTK4 and
  `forbid(unsafe_code)` reality;
- ADR-0061: add `ToggleSession` to the matrix;
- ADR-0072:59: the wtype rung types into whatever is focused at delivery.
  Document that as the 1.0 behaviour;
- `archive/docs/management/{a11y-audit-P9,security-audit-P9,risk-register}.md`:
  mark them historical or correct them. The a11y "PASS" rests on a stub, and
  the advisory "pass" ran with a failed database fetch.

Later Sol results on docs, spec and evidence may add items here; the
reviewer appends them.

---

## WP4 exit check

```bash
python3 scripts/verify_traceability.py
python3 -m pytest -q tests/governance
python3 scripts/validate_pack.py
```

- Every release-gated requirement has a procedure whose assertions are all
  measured. Check with a one-off script over the profile JSON; the counts in
  T4.1 should all become "measured".
- Every new or changed recorder has had one successful scratch dry run,
  logged in `PROGRESS.md`.
