# Gaps, Issues, Improvements & Edge-Case Register — 2026-08-22

Full-project review (start → finish) performed at the owner's request before
the build handoff. Each item is actionable, scoped, and tagged:
`[FIXED-2026-08-22]` already done this pass · `[P1]`…`[P11]` landing phase ·
`[DECIDE]` needs an owner/architecture decision · `[MENU]` feature candidate —
owner's choice, never a default commitment.

## A. Defects & inconsistencies

1. `[FIXED]` **CI never ran `verify_architecture_map.py`** — catalog
   consistency (feature map ↔ pipelines ↔ runtimes) was unenforced in CI.
   Added it + `verify_pipeline_contracts.py` to the `pack` job.
2. `[FIXED]` **Singleton multiplicity unenforced** — chainable-vs-singleton
   rules existed only in prose; the pipeline verifier now rejects >1 instance
   of singleton capabilities (`audio.capture`, `speech.asr`, `speech.endpoint`,
   `context.provider`, `delivery.direct`, `quality.qualifier`).
3. `[FIXED]` **Duplicate enabled trigger bindings** within one pipeline now
   rejected by the verifier (cross-pipeline arbitration → P1 trigger router).
4. `[FIXED]` **Threat model gap**: prompt injection via *transcribed speech*
   into the optional LLM post-processor (spoken instructions redirecting the
   corrector) was absent — added with mitigations (semantic guard, edit
   budget, deterministic fallback, raw recoverability, ADR-0018 data classes).
5. `[FIXED]` `agent/STATE.json` placeholder timestamp; stale engine/module
   counts in `VALIDATION-REPORT-v1.3.md` (annotated as historical snapshot);
   `modes-profiles.md` now cross-links the pipeline-editor screen contract.
6. `[KNOWN]` Probe evidence under `agent/evidence/P0F/` is bound to an older
   commit — regenerate before any gate run (documented in handoff).
   v1.9.1 secondary) — both 2026-active; resolve exact tag at P5 pin time.
7. `[KNOWN]` Unverified licenses: ten-vad, moonshine v2 — check before any
   adoption decision.

## B. Schema / design gaps to resolve at P1 (with the pipeline registry)

1. `[P1][DECIDE]` **Pipeline instance versioning**: docs promise "versioned
   pipelines" and the editor does version-bump-on-save, but the schema has
   only `schema_version` — no per-pipeline `version`. Add `version` (int,
   monotonic) + migration semantics.
2. `[P1][DECIDE]` **Perf budget field**: the editor contract references a
   pipeline perf budget; schema lacks `perf_budget` (e.g., target p95 ms +
   memory ceiling). Add optional in schema v1.1.
3. `[P1]` **Concurrent-session rule for multi-pipeline triggers**: two
   hotkeys exist; what happens when pipeline B triggers while A is active?
   The state machine says one primary session but never resolves the conflict.
   Recommended 1.0 rule: reject B with immediate overlay feedback (predictable,
   bounded); queueing is post-1.0.
4. `[P1][DECIDE]` **`enabled` vs `optional` node semantics**: editor Level-1
   "on/off toggle" maps ambiguously to `optional` (skippable-on-failure).
   Recommend a distinct `enabled` flag (default true) for user disabling.
5. `[P1]` **Stream-kind vocabulary**: edge `stream` is free-form text; define
   the enum (audio/speech/transcript/text) + a per-capability I/O contract
   table so graph validation is mechanical.
6. `[P1]` **Trigger-binding grammar**: `binding` is an unnormalized string
   ("Ctrl+Space"); define the canonical keybinding grammar before compositor
   integration, and registry-level cross-pipeline conflict detection.

## C. Edge cases to encode in tests/docs (phase-tagged)

1. `[P2][policy]` **Sensitive/password targets**: Narrivy must never read
   password fields; dictation *into* them is user responsibility. Write the
   policy statement; suggest Privacy Zone recipe for password managers;
   heuristic warn = post-1.0 `[MENU]`.
2. `[P2]` **Long utterances**: v0.4 silently capped at 30 s. Define 1.0
   behavior explicitly (recommend auto-finalize + seamless continuation
   chunking over a hard silent cap) + UX for the boundary.
3. `[P5]` **First-run compile UX**: NPU cold compile measured at 144–170 s —
   onboarding must show real progress and set expectations ("one-time
   preparation"); post-driver-update cache invalidation must surface its
   reason in Settings, not look like a hang.
4. `[P5]` **Memory pressure**: 3-model residency ≈ 5–6 GiB; on system memory
   pressure unload LRU models (never the active session's ASR) instead of
   swap-thrashing. Add as scheduler requirement.
5. `[P4][MENU]` **Audio feedback while duck-policy active**: start/stop chime
   would itself be ducked — if chimes ship, they must respect the active
   audio policy and always have visual equivalents (ACC rules).
6. `[P3]` **RTL overlay partials**: live Urdu partial text rendering in the
   overlay needs the RTL acceptance pass before any streaming claim.
7. `[P2]` **Clipboard managers** record transactional copies — already noted
   in the delivery matrix; add an integration-test fixture with a manager
   running.
8. `[P9]` **Fast user switching / multi-seat**: per-user runtime dir isolates
   daemons; add lifecycle test.
9. `[P2]` **Trigger before model installed / mid-download**: typed error +
   recovery action (v0.4 pattern worked — keep it).
10. `[P7]` **Stale pipeline dependency** (referenced module/model removed):
    activation-time re-resolution + truthful degraded UI (editor contract
    covers design; needs the runtime rule).
11. `[P5/P11]` **Driver/runtime updates invalidate qualification tuples**:
    detect and prompt requalification; never silently run stale-qualified
    paths.

## D. Feature-addition candidates `[MENU]` (owner's choice; none are commitments)

1. **Local stats dashboard** on History (words/day, sessions, fallback rate,
   time saved estimate) — local-only, opt-in. Natural home: P7+.
2. **"Re-process with another pipeline"** from History (exists as a post-1.0
   journey; could be promoted early since pipelines make it natural).
3. **Model warm-up toggle** ("keep ready in background") with explicit battery
   tradeoff disclosure — P5/P6 settings candidate.
4. **Overlay drag-reposition + position memory** — small P3 addition.
5. **Pipeline quick-switcher** (tray/command-palette) beyond per-pipeline
   hotkeys — P7 candidate.
6. **Per-app default pipelines** — already FUN-023 post-1.0; flagging as a
   promotion candidate the owner may pull forward.
7. **Opt-in anonymized crash reports** — post-1.0, privacy-gated design only.
8. **Benchmark-result export** from Model Lab (adjacent to support bundle).
9. **Audio duck "smart" policy and mute** — deferred by FUN-007 scoping;
   promotion only after crash-safe standard is met.
10. **Acoustic/personalization adaptation** (beyond dictionary) — R12+ idea,
    not in any current requirement.

## E. Process/tooling improvements

1. `[FIXED]` CI coverage extended (architecture map + pipeline contracts).
2. `[P1]` Singleton/trigger checks now in the Python verifier must also live
   in the real Rust graph validator (verifier is a pack-level guard only).
3. `[P3+]` Consider a markdown link checker for the doc set as it grows
   (`scripts/check-markdown-links.md` notes the intent; tooling unbuilt).
4. Standing: reseal manifest after any tracked-file change; evidence binds to
   HEAD (commit → run → gate); historical reports get annotations, not edits.

## Consolidated open-owner decisions — status after the 2026-08-22 question round

1. ~~OpenVINO version~~ RESOLVED: refreshed to 2026.4 on 2026-09-21; Qwen3-ASR CPU/GPU supported upstream, NPU candidate-only pending exact qualification.
2. ASR default tier — P5, owner's call at qualification (daily-driver signal:
   Qwen3-ASR 1.7B INT8; selection surfaces always preserve user choice).
3. Streaming lane (sherpa-onnx / transcribe.cpp GGUF models /
   Parakeet-Eddy-NPU) — P5 qualification decides on evidence.
4. Small-LLM post-processing model — P5 qualification.
5. Concurrent-session rule (recommend reject-with-feedback) — P1 design;
   adopt recommendation unless owner vetoes at P1 review.
6. ~~History squash~~ RESOLVED: approved and executed 2026-08-22.
7. ~~GNOME lane timing~~ RESOLVED: deferred to P11; P0F owner gate amended
   with recorded consent; real-speech samples policy set (owner fork history).
8. ~~Owner reservations~~ RESOLVED: none remaining (owner confirmed).
9. HF org token — owner creating org now; token at P5 owner window (secure
   input); local-cache fallback keeps P5 autonomous.

## F. Open question for later (owner, 2026-08-22 — not a scope change)

**COSMIC Store submission.** Owner asked whether the project is documented for
submitting to the Pop!_OS COSMIC Store. Status: not documented — and it is
*deliberately out of 1.0 scope* by two standing owner decisions: (a)
single-artifact `.deb` release (no AppImage/portable package), and (b)
release-packaging rule "Flatpak only after sandbox/portal implications are
proven." COSMIC Store distribution is expected to mean a Flatpak/Flathub
submission (verify current store backend when this becomes active). The
critical unproven part for Narrivy specifically is **text delivery and global
shortcuts from inside a Flatpak sandbox** (wtype/uinput-style injection and
evdev are restricted; portal-first paths — GlobalShortcuts, PipeWire, Secret
Service — align well). Next agent: if the owner promotes this, treat it as a
post-1.0 packaging track requiring (1) sandbox permission/portal proof for the
full trigger→mic→ASR→delivery chain, (2) a Flatpak manifest + Flathub
submission per their current guidelines, (3) an ADR re-promoting the artifact
type. Until then: no action.
