# WP5 — Qualification campaign on one frozen commit

**Goal:** record current, valid evidence for every release-gated requirement
against one frozen source commit and one exact package, on the Pop!_OS
COSMIC reference lane.

**Hard precondition:** WP1–WP4 are complete, committed, and their exit checks
pass. In particular WP4 **T4.0** must be done. Without it, the second
clean-tree recorder refuses to run. From the freeze until WP6 finishes, **no tracked source, recorder,
profile or packaging file may change.** Evidence files are the only thing
written.

**Why this is strict:** `scripts/evidence_policy.py:453` rejects evidence whose
`git_commit` differs from `HEAD`. Recorder hashes and artifact hashes are
checked too. One edit resets the whole campaign.

---

## T5.1 — Freeze

1. Run the full WP2 exit check once more. Everything must be green.
2. Make sure `git status --porcelain` is empty. Record `FREEZE_COMMIT=$(git rev-parse HEAD)`
   in `PROGRESS.md` and `agent/STATE.json` (committing the STATE change is
   the last commit before the freeze; take the hash *after* it).
3. Build the package from `FREEZE_COMMIT` with
   `scripts/build_release_candidate.sh <version>` (WP3). The fixed path
   `artifacts/releases/narrivy-linux-amd64.deb` now holds it, and the previous
   candidate is at `narrivy-previous-amd64.deb`. Record the SHA-256.
4. Install that exact package on the COSMIC machine (owner approves the
   `sudo` step).

From here on, do not commit anything until T6.6.

## T5.2 — Run the recorders

Run each registered procedure **exactly as its `command_argv` says** in
`spec/evidence-verification-profiles.json`. Do not add or change arguments.
Evidence lands in `agent/evidence/<phase>/VER-*.json` as the procedure
defines.

Suggested order (cheap and automatic first, native and owner-present last):

1. `python3 scripts/record_unit_contract_evidence.py`
2. `python3 scripts/record_controlled_integration_evidence.py`
3. `python3 scripts/record_supply_chain_evidence.py` (needs the WP3 supply-chain
   bundle for the frozen package)
4. `scripts/record_package_lifecycle_evidence.py` and
   `record_package_upgrade_evidence.py` (Docker; check that Docker is
   available first)
5. `scripts/record_cpu_performance_evidence.py`
6. Native COSMIC, with the owner present for microphone and permission
   prompts:
   - `record_cosmic_vertical_slice_evidence.py`
   - `record_native_ui_dictation_evidence.py`
   - `record_native_audio_policy_evidence.py`
   - `record_native_recognition_evidence.py`
   - `record_native_accessibility_evidence.py`
   - `record_native_hardening_evidence.py`
   - the new WP4 recorders (activation, REL-004, target-app matrix)

After each recorder:
- check its exit code, and read only the failure excerpt;
- promote the requirement status with `scripts/mark_requirement.py` exactly as
  that script documents. Never edit the CSV `status` or `evidence` columns by
  hand;
- log `task, recorder, exit code, evidence path` in `PROGRESS.md`.

## T5.3 — When a recorder fails

- **Environment problem** (a missing app, the mic muted, a permission not
  granted): fix the environment and rerun the same recorder. Allowed; nothing
  tracked changed.
- **Product or recorder bug:** stop the campaign.
  1. Write the defect in `00-review-findings.md`.
  2. Fix it with a test (WP2 routine), and commit.
  3. Start WP5 again from T5.1 with a new freeze commit. All evidence from the
     aborted campaign is invalid. Delete it (`git checkout -- agent/evidence`
     plus removing new untracked evidence files) before re-running, so stale
     files cannot be promoted.
- To keep restarts cheap, run the recorders most likely to fail (native
  delivery, target matrix, accessibility) **as dry runs before the freeze**,
  as WP4 T4.1 step 6 requires.

## T5.4 — Check coverage

Run the gate without trying to make it pass yet. The gate's first step runs
`validate_pack.py`, which compares every tracked file with
`BUILD-PACK-MANIFEST.json`. The new evidence files would fail that, so reseal
the manifest first. Leave it **uncommitted**, like the evidence:

```bash
python3 scripts/build_manifest.py
python3 scripts/release_gate.py --target linux-1.0 > <scratch>/gate.txt 2>&1
```

Group the errors by type. At this stage, the only remaining failures should
be the WP6 items: missing `artifacts/release-evidence/linux-1.0/*` files, the
P11 report, TEST-010 and OPS-005. Anything else means a requirement is still
blocked. Go back to T5.2 or T5.3 for it.

---

## WP5 exit check

- every release-gated requirement except TEST-010 and OPS-005 is `passed` in
  the CSV, with evidence bound to `FREEZE_COMMIT`;
- `release_gate.py` shows no per-requirement errors other than those two;
- nothing tracked has changed since the freeze except evidence files and the
  CSV status/evidence columns written by `mark_requirement.py`.
