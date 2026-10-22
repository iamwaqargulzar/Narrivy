# WP1 — Truth and hygiene

**Goal:** every document an agent reads at startup matches the code at `HEAD`.
Remove leftovers that confuse the evidence tooling or the next agent.

**Why first:** agents trust `STATE.json`, `BLOCKERS.md` and the entry documents.
Today several of them contradict the code, so an agent following them will
redo finished work or run the wrong tools.

**Scope:** documentation, state files and dead code only. No product
behaviour changes in this WP.

---

## T1.1 — Make `agent/STATE.json` truthful

**Problem (verified):** `STATE.json` was last updated 2026-09-22. It says
"50/98 passed" and names the known-phrase acoustic miss as the current work
item. Commit `1ccafff` (2026-09-28) says it fixed that miss (VAD thresholds in
`NativeEnergyVad`). It also moved four plugin requirements to post-1.0. The
traceability CSV now shows 93 gated requirements (after the 2026-09-28 owner decision
moved PORT-001 to post-1.0), 39 passed and 54 blocked.
Even the 39 "passed" ones fail the release gate.

**Steps:**
1. Run `python3 scripts/release_gate.py --target linux-1.0 > <scratch>/gate.txt 2>&1`
   and count the errors by type. Do not paste the whole output into context.
2. Update `agent/STATE.json` with these fields: `current_work_item` (point at
   this plan and the task you are on), `last_gate_commit`, `last_gate_result`
   (real counts), `latest_verified_work`, `notes`, `updated_at`. Keep the
   schema; check it with `python3 scripts/validate_pack.py`.
3. Do **not** claim the acoustic fix is qualified. The code changed, but no
   native evidence has proven it yet. Say exactly that.

**Acceptance:** `validate_pack.py` passes. Every number in `STATE.json` is
reproducible from a command you ran.

---

## T1.2 — Re-check every open blocker in `agent/BLOCKERS.md`

**Problem (verified):**
- B-SUPPLY-010 and B-DESKTOP-008 say `cargo deny` reports 17 advisories from
  the Tauri/GTK3 graph. On 2026-09-28 `cargo deny check` passed with
  `ignore = []`, because `apps/narrivy-desktop/src-tauri` is excluded from the
  workspace (`Cargo.toml` `exclude`).
- B-RECOVERY-001 says "no clean schema-v2 evidence is registered". That is no
  longer true: v2 and v3 records exist.

**Steps:**
1. For each blocker marked OPEN, PARTIALLY RESOLVED or LOCALLY RESOLVED, run
   the check its "Unblock criteria" names. Record the command and result.
2. Update the status line and add a dated "Current verification (2026-MM-DD)"
   bullet. Do not delete history; append.
3. For B-DESKTOP-008, check each parity item it lists against the GTK shell
   (`apps/narrivy-desktop/src-gtk/src/`: `overlay.rs`, `portal_executor.rs`,
   `portal_transport.rs`, `shell.rs`):
   - overlay parity;
   - xdg-foreign parent export;
   - cancel → revoke → original terminal reply → unexport ordering;
   - window-close and exit behaviour.
   Point to the code and test that proves each one, or mark it still open.
   Anything still open becomes a task in WP2 (add it to `00-review-findings.md` first).

**Acceptance:** each blocker's status matches a check you ran in this session.

---

## T1.3 — Fix stale entry documents

**Problem (verified):**
- `GIVE-THIS-TO-CODING-AGENT.md` tells the agent to use
  `scripts/run_gate.py`. `AGENTS.md` says that script is a quarantined no-op
  (exits 2). It also describes the P0F-era workflow.
- `README.md` says "P0F — active next phase".
- `00-AUTONOMOUS-START.md` step 4 says "this v1.3 scaffold: P0F".

**Steps:**
1. In each of the three files, replace the stale instructions with the
   current ones: active phase P11, use `phase_gate.py` / `release_gate.py`, and
   read `agent/completion-plan/README.md` first.
2. Keep the owner decisions and hard rules that are still true, such as never
   pushing to `origin`.
3. Search for other references to the quarantined script:
   `rg -n "run_gate.py" --glob '!scripts/run_gate.py' --glob '!agent/history/**'`.
   Fix every live instruction that still points at it. Historical records may
   keep it.

4. **Second pass (P-7):** fix or retire the other files that still steer an
   agent the wrong way. In each, make the file current (Pop!_OS COSMIC only,
   P11, `phase_gate.py`/`release_gate.py`), or move it to `agent/history/`
   with a one-line pointer:
   - `agent/CODING_AGENT_UPDATE_PROMPT_2026-08-24.md` ("continue at P8",
     `run_gate.py`): move to history;
   - `agent/PHASES.json` summaries at lines 24, 44, 134 (GNOME);
   - `docs/maintainers/release/linux-1.0-qualification.md:15,32` (`gnome-wayland/`
     directory, "both lanes");
   - `archive/docs/autonomous-build/reference-environments.md:3,7` ("provision the
     missing environment yourself");
   - `docs/maintainers/qualification/reference-environments.md:26`;
   - `docs/maintainers/phases/prompts/p11.md:4,11`;
   - `archive/docs/CHECKLIST_MASTER.md:7,15,29`;
   - `docs/platforms/bounded-support.md:37`, `gnome.md`, `linux.md:7`
     and the GNOME column of `capability-matrix.md` (mark GNOME
     post-release/SQ1);
   - `spec/scenarios/p11-gnome-native.json` and `p2-trigger-gnome.json`
     (retire them so no runner can pick them up);
   - `archive/docs/management/autonomous-build-review-2026-08-22.md` (its
     `run_gate.py` and `refresh_p0_evidence.py` recipe breaks the evidence
     INDEX): mark it historical;
   - `agent/BLOCKERS.md`: fix the counts (93 release-gated requirements, not
     98). Say that the 39 rows marked `passed` hold evidence from older
     commits, which the gate rejects until WP5 re-records them. Rewrite the
     B-HW-003/004 bodies as post-release;
   - `agent/RELEASE-EVIDENCE-TEMPLATE.md` and `agent/PHASE-REPORT-TEMPLATE.md`:
     rewrite them from what `release_gate.py` and
     `spec/schemas/phase-report.schema.json` actually enforce, or delete them
     and point to WP6.
5. **Second pass (P-6):** quarantine the legacy evidence writers the same way
   as `run_gate.py`: exit 2 with a one-line message. Add a governance test
   asserting that each one exits 2.
   - `scripts/refresh_p0_evidence.py`, `refresh_p1_evidence.py`,
     `refresh_p3_evidence.py` and `refresh_p8_evidence.py` overwrite recorder
     output with a hard-coded "native COSMIC" environment. That is the bulk
     refresh AGENTS.md forbids. Check the other `refresh_p*` scripts and
     quarantine any that no registered procedure uses.
   - The legacy `tools/narrivy-qualify/*_runner.py` (Sol B26):
     `p5_compute_runner.py`, `p5_model_runner.py`, `p7_runner.py`,
     `p8_history_runner.py` and `p8_dict_runner.py` overwrite **currently
     qualified** evidence (for example `VER-TEST-005.json`, `VER-SEC-007.json`
     and six P8 records) with schema-v1 output. `p4_runner.py` can leave the
     real default sink ducked.

   None of the 86 release procedures uses any of these (checked 2026-09-28).
   Owner gates that still call them are fixed in WP4 T4.6 item 4.

**Acceptance:** that `rg` finds `run_gate.py` only in historical records,
quarantine notes, or tests that assert it is quarantined.
`rg -n "gnome-wayland|GNOME" docs agent spec --glob '!agent/history/**' --glob '!agent/completion-plan/**'`
shows GNOME only as post-release, SQ1 or historical.

---

## T1.4 — Migrate the desktop to Tauri 2 on every OS (owner D22, ADR-0075)

**Goal:** the Tauri app in `apps/narrivy-desktop/src-tauri` becomes the only
desktop host, at parity with the GTK4 shell. Then the GTK4 shell is
retired. The daemon is untouched.

Steps, one verified commit each:
1. **Upgrade:** Tauri and plugins to the current 2.x line (Handy: 2.11.5);
   `rust-version` to the Tauri requirement; the frontend
   `@tauri-apps/api` to match. Re-measure `cargo deny`.
2. **Workspace:** add `src-tauri` to the workspace members, remove it from
   `exclude`, and remove `src-gtk` and `desktop-parent-linux` only in step
   6. Add each remaining advisory to `deny.toml` `ignore` by ID, with a
   reason and a pointer to ADR-0075. `cargo deny check` passes with no
   other exception.
3. **Parity** with `src-gtk/src/shell.rs`, `overlay.rs` and
   `portal_executor.rs`:
   - every frontend command is dispatched (keep the governance command-list
     test);
   - session events use bounded queues and never stop on a full queue
     (F-M9);
   - on-demand `systemctl --user start` of the daemon;
   - theme;
   - the capability UI;
   - the portal-permission flow (dormant in 1.0) or its explicit removal;
   - window close and exit behaviour.
4. **Lockdown:** reproduce the GTK4 shell's WebKit restrictions in Tauri:
   - a CSP that allows no remote origins;
   - no remote URLs and no navigation;
   - minimal Tauri capabilities or permissions, with no fs, shell or http
     plugin exposure beyond what's needed;
   - no clipboard access from JavaScript unless a feature requires it.

   Add a governance test over `tauri.conf.json` and the capabilities.
5. **Overlay the Handy way** (T2.18 is re-scoped): a Tauri overlay window
   converted to a layer-shell surface with `gtk-layer-shell` (GTK3; in the
   Pop!_OS archives, no bundling), with no keyboard interactivity and the
   monitor probe from `docs/product/ux/overlay.md`.
6. **Retire** `apps/narrivy-desktop/src-gtk` and
   `adapters/desktop-parent-linux`. Update the package builder (binary
   name, Depends: `libwebkit2gtk-4.1-0`, `libgtk-3-0`,
   `libgtk-layer-shell0`, the tray library if used), the CI desktop job,
   `test_desktop_gtk_migration.py` (rewrite for Tauri), and the docs.
7. **Native check** (owner present): launch the installed app on COSMIC. The
   UI loads, dictation start and stop work, the overlay never takes focus,
   and the app closes cleanly.

**Acceptance:**
- `cargo deny check`, `cargo test --workspace --locked`, `npm test`,
  `pytest tests` and all the gate validators pass;
- the native check passes;
- `PROGRESS.md` logs each step.

## T1.5 — Packaging directories (low priority, optional)

`packaging/deb/` and `packaging/linux/deb/` are duplicates. This is
deliberate. `tests/packaging/test_build_deb_candidate.py` asserts that the
three systemd unit files are byte-identical (ADR-0063). **Do not delete
either copy** without an ADR. Only act if you have spare time and the owner
agrees. Otherwise skip this task.

---

## T1.6 — Disk space (ask first)

`target/` is about 110 GB and `artifacts/` about 2.8 GB. `target/` and most of
`artifacts/` (packages, candidates, run logs) are ignored by git.
`artifacts/release-evidence/`, `artifacts/sbom/` and
`artifacts/quarantine-2026-09-27/` are tracked. **Ask the owner** before running `cargo clean` or deleting old `.deb`
files. Never delete anything under `artifacts/releases/` that is referenced by
evidence, or `artifacts/quarantine-*`.

---

## WP1 exit check

```bash
python3 scripts/build_manifest.py
python3 scripts/validate_pack.py
python3 -m pytest -q tests/governance
```

All three must pass. Commit WP1 as one or more small commits.
