# Session Handoff — 2026-08-22 (pre-P0F handoff to next coding agent)

Written at the end of the owner's pre-build session. Everything in this file is
also recorded in the durable artifacts it points to. A fresh agent should be
able to start cold from here.

## Start here (in order)

0. `docs/13-management/gaps-improvements-register-2026-08-22.md` — product review: fixed defects, P1 schema gaps, edge cases, feature menu `[MENU]`, consolidated open-owner-decisions (ask items 5-8 early).
0b. `docs/13-management/autonomous-build-review-2026-08-22.md` — build-machinery review: the evidence↔HEAD ritual (mandatory before every gate), P0 evidence refresh script, structural blockers (GNOME hardware, portal permissions, real-speech evidence policy decision).

1. `00-AUTONOMOUS-START.md` → `SOURCE_OF_TRUTH.md` → `BUILD_TARGET.json`
2. This file
3. `agent/STATE.json` (dynamic pointer) and `docs/13-management/salvage-inventory.md`
4. `docs/12-references/upstream-version-audit-2026-08-22.md` (per-node version/module map)
5. `agent/DECISIONS.md` + `agent/RESEARCH_LOG.md` — **2026-08-22 entries define every owner decision below**
6. Resume `agent/STATE.json.active_phase` = **P0F**

## What this session accomplished (commits, oldest → newest)

- `c9c8e2f` — v0.4 P0F probes salvaged/adapted (6 probes + `p0f_common.py`), run on this COSMIC host (6 pass + GNOME truthfully failing); salvage inventory + decision; manifest defect (stale `.gitignore` seal) fixed by reseal.
- `a8bd8da`, `1459db5`, `2ee0ae2` — upstream version audit (all nodes), post-processing LLM lane research, consolidated audit doc.
- `c708600` — transcribe.cpp scoped (streaming candidate #4-of… see below).
- `495eabd` — owner's Handy-Pop-OS NPU fork audited (verified NPU facts + recipes).
- `8efa44f`, `d114096` — salvage archive reorganized as `code/` (17 GB junk removed); histories wiped; owner-authored-only extraction at `code/handy-npu/`; clean-room policy amended.
- `e8aa89f` — owner's daily-driver reference recorded (their fork, Qwen3-ASR 1.7B INT8 on NPU).
- `3f4b971` — pipeline composition figured out: node multiplicity rules (chainable vs singleton), Pipeline Editor screen contract (3 levels), capability-name drift fixed, verifier hardened with catalog cross-check.
- `4bc619c` + final commits — owner's all-NPU experiment pipeline recorded; full per-slot candidate research sweep integrated (see module-candidate-catalog addendum).

## Owner decisions made this session (all binding, all recorded)

1. **Salvage approved** for `code/narrivy-v0.4/` (own code) — adapt behind v1.3 contracts, never transplant; old evidence never satisfies new gates.
2. **`code/handy-npu/`** — owner-authored NPU work from their public fork is code-salvage eligible (clean-room policy amended). Upstream fork code is NOT retained. **No mentions of the Handy original author anywhere — scope: Handy fork only; all functional third-party attribution unchanged and mandatory.** Do not reintroduce such mentions.
3. **Prefer latest stable upstream at lock time**; product pins are immutable revisions; never silently upgrade across a passing release candidate.
4. **Owner daily-driver oracle** = their fork running Qwen3-ASR 1.7B INT8 on NPU. Narrivy's default path must be at least as good; the fork can be tested side-by-side on this machine.
5. **Owner experiment (personal, non-default)**: all-NPU pipeline (NPU denoise + NPU ASR + NPU LLM correction). Doubles as the NPU co-residency qualification case. Needs a `qwen3-asr` speech.asr catalog binding when the fork work is salvaged (P5).
6. Owner uses voice-to-text for instructions — interpret messages for intent; confirm before destructive/ambiguous actions.
7. ~~Reservations about the v1.3 scaffold~~ RESOLVED 2026-08-22: owner confirmed none remaining.
8. 2026-08-22 owner question round resolved every pending decision (GNOME→P11 with gate amendment; real-speech samples from the owner's public fork history, provenance-verified; OpenVINO 2026.3 locked; MIT final; squash executed; portal pre-grant+windows plan; HF org in progress, token at P5 window). Full record: `agent/DECISIONS.md` final entry.

## Current state

- **GitHub remote registered (2026-08-22):** PUBLIC empty repo
  https://github.com/iamwaqargulzar/narrivy added as `origin` (owner chose
  public-while-empty; CI runs free on public repos). **Never push until the
  owner explicitly signals push time** — that push becomes the repo's first
  version; at that moment ask the owner whether to squash local history into
  one baseline commit or push the full phase history.
- Preflight green (`validate_pack` 422+ files, owner gates 13/13, traceability 136/98/38, pipeline contracts incl. new catalog cross-check).
- Scaffold code: 12 crates compile, 3 tests pass; `engines/whisper-*` are README-only placeholders.
- `tools/p0f-probes/` real and proven on this host (COSMIC). **Evidence under `agent/evidence/P0F/` is bound to an older commit — regenerate (one command each) after any commit before gate runs.** GNOME scenarios truthfully fail on COSMIC by design (lane mismatch, exit 3).
- Salvage archive: `code/` (gitignored except README, manifest-excluded): `narrivy-v0.4/` (cleaned snapshot, no .git), `handy-npu/` (owner files only, incl. `owner-npu-work.patch`).
- Version/module map + 4 pending lock-time decisions: `docs/12-references/upstream-version-audit-2026-08-22.md` (OpenVINO 2026.3-vs-LTS now owner-fork-verified; ASR tier candidates incl. Qwen3-ASR owner-preference; streaming candidates sherpa-onnx / transcribe.cpp / Parakeet-Eddy-NPU / whisper.cpp-simulated; small-LLM class for post-processing).
- Candidate research FULLY integrated into machine-readable catalogs: `spec/catalogs/engines.json` 18→27 families, `spec/catalogs/modules.json` 56→68 bindings (all candidate/roadmap status; architecture-map + human indexes updated; verifier green). Urdu/Hindi sister-language rule recorded (owner): Hindi support ≠ Urdu support — script divergence; Urdu lanes = whisper family / whisper-large-v3-urdu fine-tune / IndicConformer-STT-UR / Qwen3-ASR (likely; verify card + Urdu fixture at P5; owner's daily Urdu use is empirical evidence). Remaining unverified licenses: ten-vad, moonshine v2.

## What to do next (ordered)

1. **Run every gate through `python3 scripts/run_gate.py --phase <P>`** — it performs the evidence↔HEAD ritual automatically (regenerates phase evidence at current HEAD, reseals the manifest, then gates). Never call phase_gate.py directly without the ritual. The working tree intentionally holds evidence one refresh ahead of HEAD when gates are green. `tools/narrivy-qualify/narrivy-qualify.py run <scenario>` now dispatches to the real probes and fails closed (skip, exit 3) for scenarios without a runner — it can no longer fabricate pass evidence.
2. **Optional, needs owner confirmation**: squash main-repo history to one checkpoint commit before P0F starts (owner floated it; not yet confirmed — ask).
3. **P0F formalization** (active phase): commit any changes → re-run all 6 COSMIC probes (commands in `tools/p0f-probes/README.md`) → update `spec/acceptance/P0F.json` (`implemented: true`, real commands) → write schema-valid `agent/phase-reports/P0F.md` → update `docs/04-platforms/capability-matrix.md` from evidence only → run `python3 scripts/phase_gate.py --phase P0F`.
   - **Expected blocker**: owner gate P0F-002 requires passing native GNOME-lane evidence; no GNOME machine exists yet (B-HW-003 deferred). Gate will stay red on that check until the owner provides the lane or amends the gate. Record state truthfully; do NOT fake or simulate. Per autonomy policy, surface to owner; independent COSMIC-side work may proceed.
4. **P1 onward**: follow `docs/09-phases/phase-01-core-ipc.md` and `agent/PHASES.json`; land salvaged v0.4 material per `docs/13-management/salvage-inventory.md` phase map (each landing gets a DECISIONS.md entry).
5. At P5: resolve the 4 pending decisions in the audit doc; add `qwen3-asr` binding from `code/handy-npu/`; run the owner's all-NPU experiment as the co-residency case; re-verify all volatile versions.
6. Standing blockers (see `agent/BLOCKERS.md`): GNOME lane hardware (B-HW-003), AMD 8700G (B-HW-004), HF org token for model publishing (B-AUTH-006), portal permission clicks at P3+ (B-PERM-002). None block P0F/P1.

## Session norms that worked (keep them)

- Commit after each coherent unit; reseal manifest (`python3 scripts/build_manifest.py`) whenever tracked files change, then `validate_pack`.
- Evidence binds to HEAD: commit → run probes → gate (never the reverse).
- Research goes in RESEARCH_LOG with dates+sources; decisions in DECISIONS; volatile pins in RESOLVED_VERSIONS.
- Never weaken a gate; truthful failure beats simulated success.
