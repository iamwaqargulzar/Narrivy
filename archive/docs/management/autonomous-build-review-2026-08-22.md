# Autonomous-Build Machinery Review — 2026-08-22

Owner-requested review of the **build pipeline itself** (phases, gates, state,
scripts, evidence, autonomy loop) — not the product. Companion to
`gaps-improvements-register-2026-08-22.md` (product review). Every gate was
dry-run for real; findings below are observed behavior, not inspection guesses.

## Defects found & fixed this pass

1. **P0 gate was RED on resume** (the loop's first step would have failed):
   all five `agent/evidence/P0/VER-*.json` records carried placeholder
   `git_commit: "0000000"` and stale pre-v1.3 facts (e.g., "126 requirements").
   The scaffold's "P0 passed" STATE claim did not survive its own gate.
   → Fixed by `scripts/refresh_p0_evidence.py`: re-runs each record's attested
   command and rebinds it to the real HEAD/outcome/timestamps. Permanent,
   re-runnable — run it before any P0 gate or release-gate attempt.
2. **VER-PORT-001's attested command was never runnable** — it contained a
   literal `...` ellipsis instead of the MSRV crate list. Repaired to the real
   CI command (workspace check + full MSRV portable check).
3. **VER-TEST-008 was doubly self-referential** (fixed-point defects): its
   command was `phase_gate --phase P0` (a record the gate itself reads →
   refresh loop could never converge), and its `artifact_hashes` hashed
   *itself* (any edit broke it). Now it asserts the actual TEST-008 semantics
   positively: an unimplemented phase gate (P0F) must exit non-zero; the
   self-hash pin is removed.
4. Result: **`phase_gate --phase P0` now passes** with live evidence
   (5/5 requirements, commit-bound, commands green including fmt/clippy/
   test/deny).

## The evidence↔HEAD ritual (critical for every phase)

Evidence binds to the git HEAD at generation time, and gates compare it to the
*current* HEAD — so **any commit invalidates all existing evidence**. The
working recipe, verified end-to-end:

1. Implement + commit everything except evidence and the phase report.
2. Regenerate/refresh evidence at that HEAD (P0: `refresh_p0_evidence.py`;
   P0F: re-run the probes; later phases: their generators).
3. Write the phase report with `git_end_commit` = HEAD (leave it uncommitted).
4. Run the phase gate → passes with `dirty=True` (explicitly allowed:
   "clean or documented dirty tree").
5. `advance_phase.py` re-runs the gate itself, then atomically updates
   STATE (including `last_gate_commit`/`last_evidence_digest`).
6. Commit evidence + report. From this moment the evidence is stale-bound
   again — which is fine, because the next gate run repeats step 2. At
   release time, ALL evidence refreshes at the final RC commit before
   `release_gate.py`.

Consequence to remember: **the working tree intentionally holds evidence one
refresh ahead of HEAD** whenever the gates are green. Do not "clean that up"
by deleting evidence.

## Verified-good machinery (no action needed)

- `phase_gate.py`: handles all owner-check kinds (scenario/evidence/artifact/
  command/schema/env/manual); verifies scenario hashes, native requirements,
  blocker annotations, regression prerequisites from the first working phase,
  smoke commands for working-app phases; fail-closed on unimplemented
  acceptance (P0F correctly exits 4 today).
- `advance_phase.py`: re-runs the gate, validates the phase report schema,
  atomic STATE writes (fsync+rename+flock), records gate commit + evidence
  digest.
- `release_gate.py`: correctly red today (planned requirements + missing
  release artifacts) — right failure mode.
- Blocker regex honors the non-blocking annotations (B-HW-003 deferred,
  B-PERM-002/B-AUTH-006/B-LEGAL-007 non-blocking) — P0F will not fail on
  blockers, only on real missing evidence.
- `tools/narrivy-qualify/narrivy-qualify.py` has `list/run/verify` matching
  the prompt references; CI now covers pack + owner gates + architecture map
  + pipeline contracts + rust quality + portable matrix + MSRV + supply chain.
- P0F probe evidence mechanics verified live (scenario-hash binding, truthful
  GNOME-lane failure).

## Structural blockers & issues for the autonomous build (owner-facing)

1. ~~GNOME lane hardware~~ **RESOLVED 2026-08-22**: deferred to P11 by owner
   decision; P0F owner gate amended (P0F-002 removed, baseline resealed,
   consent recorded in DECISIONS). P0F now completes on COSMIC-native evidence.
2. **Interactive portal permissions (B-PERM-002)** — plan CONFIRMED 2026-08-22:
   pre-grant via the portal permission store where scriptable + short
   owner-present permission windows; degrade truthfully if unavailable.
3. ~~Real-speech evidence policy~~ **RESOLVED 2026-08-22**: virtual-source
   playback of the owner's real-speech samples (sourced from the public
   Handy-Pop-OS fork history, provenance-verified per sample) plus synthetic
   fixtures = real-audio-path evidence; live human session per lane at P11.
4. **HF `narrivy` org token (B-AUTH-006)** — model publishing at P5; local
   cache fallback keeps the build autonomous until then.
5. **AMD 8700G hardware (B-HW-004)** — deferred, optional claims only.
6. **Legal review before RC (B-LEGAL-007)** — owner action, non-blocking
   until P10/P11.
7. **Coding-agent context limits** — mitigated: `archive/docs/autonomous-build/
   execution-loop.md` + `agent/SESSION-HANDOFF-2026-08-22.md` + durable
   decision/evidence records; fresh agents resume from STATE, not memory.
8. **ggml/Vulkan builds need the bindgen/clang environment** — the v0.4
   `.cargo/config.toml` env pattern (LIBCLANG_PATH etc.) is recorded in the salvage
   inventory; carry it into engines/transcribe-cpp native builds when the
   runtime feature lands (P2/P5).

## Standing recipe (put into practice by the next agent)

Before any gate: `python3 scripts/refresh_p0_evidence.py` (P0), re-run probes
(P0F), regenerate phase evidence (later phases), then `build_manifest.py`,
then the gate. See the ritual above for exact ordering.

## Second-look pass (same day) — additional findings & fixes

1. **`narrivy-qualify run` was a pass-fabricating stub** (most dangerous
   finding of the review): it executed no probe yet wrote
   `result: "pass", exit_code: 0` evidence — and the P0F work order tells the
   agent to use it. A well-intentioned agent would have produced fake
   gate-passing evidence. Rewired: it now dispatches to the real P0F probes
   and, for scenarios without a runner, writes truthful `skip` evidence and
   exits 3 (verified: real dispatch runs the probe; unimplemented exits 3).
2. **`scripts/run_gate.py` added** — the ritual in one command (generate phase
   evidence at current HEAD → reseal manifest → gate). This is now the only
   sanctioned way to run gates; wired into `00-AUTONOMOUS-START.md`,
   `AGENTS.md`, and the handoff.
3. Entry-point chain closed: `00-AUTONOMOUS-START.md` now points at the
   session handoff and both review documents before anything else.
4. End-to-end smoke: `run_gate --phase P0` → PASSED (5/5, commit-bound).
