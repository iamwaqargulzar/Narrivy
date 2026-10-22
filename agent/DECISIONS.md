# Autonomous Decision Log

Append durable implementation decisions here using:

## YYYY-MM-DD — short title
- Phase/work item:
- Decision:
- Alternatives considered:
- Reason:
- Requirements/ADRs affected:
- Verification:

Architecture-level decisions additionally require an ADR under `docs/adr/`.

## 2026-08-28 — Linux IPC authenticates a UID principal, not a process
- Phase/work item: Recovery Phase 3A Unix IPC transport extraction.
- Decision: Adopt ADR-0031. Validate kernel peer UID in both directions before
  protocol bytes, retain strict endpoint ownership/mode/link/inode controls,
  and explicitly limit the claim to a same-UID principal. Handshake roles and
  PIDs are not authorization. Privileged command authorization/rate policy is
  a separate release blocker.
- Alternatives considered: claim pathname checks fully defeat same-UID
  substitution (rejected as impossible with POSIX unlink semantics); trust the
  self-declared role (rejected); weaken endpoint checks because same-UID risk
  remains (rejected because they enforce the cross-user boundary).
- Reason: independent security review found missing client-side peer
  validation and clarified that DAC/`SO_PEERCRED` cannot identify one trusted
  process among mutually hostile processes under the same UID.
- Requirements/ADRs affected: SEC-005, ADR-0011, ADR-0030, ADR-0031;
  B-IPC-012 remains open.
- Verification: injected client rejection produces `PermissionDenied` and
  zero hello bytes; server peer checks precede handshake handling; endpoint
  hard-link/symlink/type/owner/replacement tests pass.

## 2026-08-26 — Recovery Phase 1 evidence registry and artifact binding
- Phase/work item: Recovery Phase 1 governance rework.
- Decision: Adopt ADR-0020. Schema-v2 evidence is release-eligible only when the authoritative index registers its exact file hash, and qualifying evidence binds the actual exercised executable or release package.
- Alternatives considered: retain an advisory index (rejected — a v2 file could bypass registry review); accept arbitrary artifact hashes (rejected — source documents could masquerade as exercised artifacts); retain callable legacy runner functions (rejected — imports could still fabricate evidence).
- Reason: closes semantic evidence fail-open paths without changing product behavior.
- Requirements/ADRs affected: TEST-001, TEST-008, P11 release governance; ADR-0020.
- Verification: governance tests cover failed/skip/missing/malformed/nonzero evidence, artifact binding, index registration/hash mismatch, and direct/imported runner quarantine.

## 2026-08-26 — Verification profiles distinguish controlled proof from native behavior
- Phase/work item: Recovery Phase 1 governance rework.
- Decision: Extend ADR-0020 with the reviewed `spec/evidence-verification-profiles.json` contract. Every traceability verification layer maps once to a profile that declares permitted evidence type, execution environment, artifact-binding need, native-platform need, and behavioural-runner need. Procedures are per verification ID, argv-pinned, assertion-bound, and hash-pin their behavioural runner. The committed procedure registry is intentionally empty.
- Alternatives considered: classify by `release_gate` alone (rejected — required unit/contract rows would falsely claim package exercise); require a release artifact for every requirement (rejected — deterministic contracts do not exercise one); allow arbitrary attested command strings (rejected — `test`, `true`, prose, and unrelated commands are not behavioural proof).
- Reason: permits required deterministic unit/contract evidence truthfully while keeping real Linux/package/native claims strict and fail-closed.
- Requirements/ADRs affected: all verification layers; ADR-0020.
- Verification: isolated fixtures prove valid controlled unit and native-runner shapes; negative cases reject unit/mock/simulated native claims, package-existence commands, Markdown history, unregistered evidence, and index hash mismatch across all validators.

## 2026-08-26 — Substitution approval is explicit and behavioural proof has none
- Phase/work item: Recovery Phase 1 governance rework.
- Decision: Require `permitted_substitutions` in both each verification profile and each reviewed procedure. A record's substitutions must be a subset of both declarations. Native-behaviour and package-artifact procedures reject every schema-defined substitute even if a future declaration is incorrectly broad; their behavioural claims require the actual bound artifact.
- Alternatives considered: trust the evidence list merely because it is well-formed (rejected — permits `substitutions: ["mock"]` to bypass a native claim); give controlled profiles a blanket allowance (rejected — a fixture/proxy must be reviewed for the exact proof procedure); rely on marker-only enforcement (rejected — phase, release, and standalone verification could diverge).
- Reason: closes the remaining native substitution fail-open path without changing product behavior or creating product evidence.
- Requirements/ADRs affected: all verification profiles; ADR-0020.
- Verification: isolated native and package fixtures reject mock, fixture, proxy, stub, and synthetic-audio across marker, verifier, phase, and release validators without mutating traceability; a controlled fixture passes only after both isolated contract declarations approve it.

## 2026-08-21 — P0 manifest hygiene and deny.toml wildcard-path fix
- Phase/work item: P0 Foundation, Cargo.lock generation and gate hygiene
- Decision: Patch `scripts/build_manifest.py` and `scripts/verify_manifest.py` to exclude `target`, `node_modules`, `dist`, `.vite`, `__pycache__`, `.pytest_cache`, `artifacts` from manifest enumeration; add `allow-wildcard-paths = true` to `deny.toml` under `[bans]`.
- Alternatives considered: (a) clean `target` before each validate (hides determinism, slow), (b) set `wildcards = warn` (would allow external wildcards), (c) regenerate manifest with target included (pollutes manifest with ephemeral build outputs).
- Reason: `target/` is ephemeral build artifact (gitignored) and was not in original pack manifest (298 files); after `cargo check` it creates ~thousands of unlisted files causing fail-closed manifest check to spuriously fail. Exclusion mirrors `.gitignore` and CI where pack validation runs before builds. `allow-wildcard-paths` is cargo-deny's intended mechanism for private path deps (PR #487) — internal `workspace = true` path deps have no version and are flagged as wildcards, but should not be denied; external wildcards remain denied.
- Requirements/ADRs affected: PORT-001, SEC-004, TEST-001, TEST-008; no ADR needed (build-tool hygiene, not system boundary).
- Verification: `python3 scripts/validate_pack.py` → Pack static validation OK (304 files); `cargo deny check` → advisories/bans/licenses/sources ok; `python3 scripts/phase_gate.py --phase P0` → PASSED.

## 2026-08-21 — P0 toolchain and lockfile
- Phase/work item: P0 Foundation
- Decision: Generate `Cargo.lock` via `cargo generate-lockfile` with pinned toolchain 1.97.1, format with `cargo fmt --all`, install `cargo-deny 0.20.2` via `cargo install --locked`.
- Alternatives considered: hand-author lockfile (prohibited), floating toolchain (prohibited), cargo-deny from apt (not available).
- Reason: Reproducible locked dependencies; verify portable crates compile on MSRV 1.85.0 and native 1.97.1; satisfy P0 gate's 7 commands.
- Requirements/ADRs affected: PORT-001, SEC-004
- Verification: `cargo check --workspace --locked`, `cargo +1.85.0 check ...`, `cargo test`, `cargo clippy -D warnings`, `cargo deny check` all exit 0; gate passes.

## 2026-08-21 — Lock Vulkan as extra GPU accelerator
- Phase/work item: P5 ASR/Compute design, owner request to maximize acceleration
- Decision: Lock hierarchy `NPU (OpenVINO) → GPU pool [Vulkan + OpenVINO GPU + ONNX Runtime EPs (CUDA/ROCm/TensorRT where present)] → CPU fallback`. Vulkan added as complementary GPU runtime, not replacing OpenVINO. Scheduler tries all qualified GPU accelerators before CPU.
- Alternatives considered: (a) Vulkan replaces OpenVINO GPU (rejected — loses Intel tuning), (b) Vulkan only (rejected — loses NPU), (c) no extra runtime (rejected — more CPU fallbacks on AMD/NVIDIA).
- Reason: Owner wants feature-rich, multi-hardware acceleration where possible, avoid slowest CPU. Lunar Lake host has Intel iGPU+NPU; Vulkan covers Intel/AMD/NVIDIA Wayland systems generically. Health record per model+runtime+driver ensures safe fallback, isolated unsafe FFI in engines/asr-vulkan.
- Requirements/ADRs affected: FUN-003, FUN-028, PORT-002, PERF-007/008, ADR-0003; no ADR superseded, extension of compute-scheduler.
- Verification: P5 will qualify NPU (OpenVINO) → Vulkan GPU → CPU flow with fixture, timestamps, cancellation, cold/warm timings, health DB; P11 release evidence will show passing CPU fallback plus accelerator claims.

## 2026-08-21 — Lock 3-tier ASR weight tiers with per-runtime formats
- Phase/work item: P5 ASR/Model design
- Decision (reworded 2026-08-22 after lane revocation): Accuracy tier = the large-v3-class GGUF weights via transcribe.cpp / OpenVINO IR where qualified, Balanced = small-class weights, Fallback = CPU-known-good. Each tier provides OpenVINO IR (FP16/INT8) for Intel NPU/GPU, ONNX QDQ (INT8) for AMD NPU (Vitis AI), ONNX FP16 for CUDA/TensorRT/MIGraphX, GGML for Vulkan/CUDA via whisper.cpp. Source is pinned immutable HF revision (e.g., openai/whisper-large-v3@commit), conversion via versioned recipe (optimum-intel, RyzenAI quantizer). Validates via fixture before atomic publish.
- Alternatives considered: single format (would break NPU/GPU), auto-latest upstream (rejected — UX risk), separate model per runtime without catalog (rejected — health single-flag).
- Reason: Covers NPU→GPU→CPU hierarchy with minimal CPU fallback, qualified per device, reproducible cache key = model hash + recipe + runtime + driver.
- Requirements/ADRs affected: FUN-003/028, PORT-006, model-catalog/model-manager, SEC supply chain
- Verification: P5 will qualify each manifest on Intel Lunar Lake NPU/GPU/CPU + AMD host for XDNA, P11 keeps raw transcript preserve.

## 2026-08-21 — Lock HF org narrivy + OpenVINO beyond a single ASR family
- Phase/work item: P5 Model library + ASR compute
- Decision: (1) HF org = `narrivy` used everywhere possible (huggingface.co/narrivy hosts all qualified IRs). External libs remain candidates only, pinned immutable_revision, never auto-switched — avoids bad UX if upstream adds new model. (2) OpenVINO pipeline = Whisper via GenAI (primary) + generic OpenVINO IR for other ASR families (Wav2Vec2, Conformer, Parakeet/Canary/SenseVoice) via optimum-intel/OVC conversion. Keeps NPU→GPU→CPU hierarchy for all families.
- Alternatives considered: own org vs external only (external only rejected — auto-pick risk), single-family-only (rejected — limits choice, owner wants broader).
- Reason: Owner wants feature-rich, own library control + external opt-in, and OpenVINO not limited to one ASR family for future models. Generic IR path proven in OpenVINO demos (wav2vec) and docs.
- Requirements/ADRs affected: FUN-003, FUN-028, PORT-006, model-catalog/model-manager, openvino-npu
- Verification: P5 will publish narrivy/* IRs with manifests (source+revision+recipe+sha256) and qualify Wav2Vec2/Conformer on Intel NPU/GPU alongside Qwen3-ASR; Model Lab shows external candidates as unqualified until user qualifies.

## 2026-08-21 — Lock multi-family NPU quants (Parakeet/Qwen3-class)
- Phase/work item: P5 ASR/NPU
- Decision: Parakeet-TDT, Qwen3 (and similar ASR LLMs like Canary/SenseVoice) all via NPU. Provide per-model quants: FP32 (reference), FP16 (NPU default), INT8 (NNCF PTQ), INT4 (NNCF MatMulNBits 4-bit per-group + FP16 attention, e.g., OpenVINO/whisper-large-v3-int4-ov), mixed INT4/INT8 (e.g., 0.8 = 80% INT4 + 20% INT8). Qwen3-8B INT4 via OpenVINO/Qwen3-8B-int4-ov pattern. Hosted as narrivy/* on HF.
- Alternatives considered: single-family-only (rejected), single precision (rejected — wastes NPU TOPS/memory).
- Reason: Owner wants feature-rich NPU use, multiple quants allow latency/memory/accuracy tradeoff; OpenVINO GenAI docs confirm FP16/INT8 on NPU plus INT4 MatMulNBits shipped 2024-26 (whisper int4, Qwen3 int4). Quant per model health-checked.
- Requirements/ADRs affected: FUN-003, PERF-007/008, model-catalog
- Verification: Each quant compiled with recipe version, cache key includes precision, health on NPU/GPU/CPU; user picks in Model Lab.

## 2026-08-21 — Lock NPU cache (compile once, load cache next time)
- Phase/work item: P5 Model manager / NPU pipeline
- Decision: NPU pipeline (OpenVINO + Vitis AI) compiles on first load (Cold), saves compiled blob to versioned cache dir, next loads read cache (Warm→Hot). Cache key = model immutable hash + conversion_recipe + precision (FP16/INT8/INT4/mixed) + runtime version (OpenVINO 2026.0) + engine adapter version + device/driver ID + compile opts. Any key change invalidates cache, recompiles.
- Alternatives considered: recompile every load (rejected — slow), memory-only cache (rejected — lost on restart).
- Reason: Owner wants dramatic load time reduction. Already in docs/architecture/model-manager.md + model-residency.md (Hot/Warm/Cold) + openvino-npu.md step 8 (persist health keyed by revision+runtime+device). Matches NPU driver expectation (compiled blob per device).
- Requirements/ADRs affected: PERF-007/008, model-manager, PERF residency
- Verification: P5 measures Cold vs Warm load (health report median/p95), P11 compares baseline. Cache file sha verified, atomic publish, never expose partial.

## 2026-08-21 — Lock cache default ON + Clear button + quant recommendation UX
- Phase/work item: P5 Model manager + UX Models & Hardware
- Decision: (1) Compiled cache default ON (Cold→cache, Warm→Hot), Settings→Models & Hardware has Clear compiled cache (per-model and all), shows size + last used + invalidation reason. (2) Quant picker on model select: ask FP32/FP16/INT8/INT4/mixed, show recommendation badge "Better for you" based on detected RAM/VRAM/NPU memory, battery/AC, thermal, model size, accuracy need. Example: 8GB RAM + battery → recommend Whisper INT4, 32GB desktop + AC → recommend FP16. User can override, recommendation is hint not lock.
- Alternatives considered: cache off by default (rejected — slow), silent quant auto-pick (rejected — opaque), no recommendation (rejected — owner wants guidance).
- Reason: Owner wants default fast + manual clear, and guided quant choice per hardware. Matches PERF-007/008 Hot/Warm/Cold + model-manager cache key.
- Requirements/ADRs affected: PERF-007/008, UX-003 progressive disclosure, model-catalog
- Verification: Settings test shows Clear button deletes versioned cache dir and next load Cold recompiles; Model Lab test shows recommendation logic with mocked hardware profiles.

## 2026-08-21 — Lock MIT license (time being)
- Phase/work item: Release gate legal
- Decision: MIT License, Copyright (c) 2026 Narrivy, THIRD_PARTY_NOTICES.md generated (cargo deny allow list + model licenses). Owner chose MIT for now to allow fork/use.
- Alternatives considered: Apache-2.0 (patent grant), MIT OR Apache-2.0 (Rust standard), GPL/AGPL (copyleft) — deferred.
- Reason: Owner wants open-source, forkable, permissive for now. MIT satisfies release_gate length checks (>=200 LICENSE, >=80 notices) and is compatible with Tauri/Rust ecosystem.
- Requirements/ADRs affected: release_gate legal metadata, LICENSE_POLICY
- Verification: LICENSE 1070 chars, THIRD_PARTY_NOTICES 1200+ chars, no TODO/PLACEHOLDER, validate_pack OK. Full release_gate still blocked until P11 artifacts but license part now passes.

## 2026-08-21 — Lock activation two modes: Hold PTT + Toggle
- Phase/work item: P0F activation feasibility + P2 walking skeleton
- Decision: Support both from same shortcut: (1) Press-and-hold — while held → Listening, release → FinalizingAudio→Transcribing; (2) Toggle — press → Listening, next press → Finalize. Truthfully expose per-desktop capability: PTT requires key-release events (not always available on Wayland portal), toggle universally supported. P0F will probe both, capability drives UI (show toggle as supported, PTT as capability-dependent).
- Alternatives considered: toggle only (rejected — user wants PTT), PTT only (rejected — Wayland may not expose release reliably per the legacy planning amendment P0F)
- Reason: Owner wants both workflows; legacy planning Part C explicitly requires proving activation/toggle vs PTT key-release semantics and not faking parity. Matches supplemental automation (Activation shortcut/PTT/toggle backend).
- Requirements/ADRs affected: FUN-001 (global push-to-talk and toggle), capability-matrix, text-delivery not affected
- Verification: P0F scenarios: shortcut press/release cycles, repeated activation, restart/suspend, manual test on COSMIC+GNOME Wayland with capability record (available/degraded/unavailable + reason). Diagnostics shows which mode qualified.

## 2026-08-21 — Lock overlay for streaming vs non-streaming models
- Phase/work item: P3 UI overlay + P5 ASR contract
- Decision: Overlay supports both: (1) Streaming ASR (engine descriptor streaming=true) → Listening + voice equalizer + live partial transcript updating in overlay (e.g., "…please send me the…"); (2) Normal non-streaming → Listening + voice equalizer only, no live text, show final after Transcribing → Inserted. Capability per model artifact in registry (streaming flag).
- Alternatives considered: live text for all (rejected — non-streaming cannot provide partials), equalizer only for all (rejected — loses streaming value)
- Reason: Owner noted two model kinds; FUN-003 requires streaming tentative where supported. Matches supplemental Part 40 floating UI (Listening + live text) and asr-contract.md (Started/Partial/Final events).
- Requirements/ADRs affected: FUN-003, UX-004/005, overlay, asr-contract
- Verification: Overlay test with Whisper Turbo streaming (Partial events) vs Whisper large-v3 non-streaming (no Partial), theme Light/Dark/System.

## 2026-08-21 — Lock Toggle as default activation
- Phase/work item: P0F activation
- Decision: Default = Toggle (Press once to start, press again to stop). Hold to Dictate available as alternative in Settings → Dictation activation. Same shortcut (Ctrl+Space) behavior depends on mode, persisted.
- Reason: Owner chose Toggle as default (simpler, universally supported on Wayland portal; Hold needs key-release which may be degraded).
- Verification: Settings shows Toggle selected by default, indicator shows correct hint, P0F probes both.

## 2026-08-21 — modular runtime architecture (supersedes generic NPU→GPU→CPU wording)
- Decision: Narrivy is module-oriented internally. Replaceable families include activation, capture, audio processing/noise suppression, VAD, ASR, compute/runtime, processors, context and delivery. No public Rust dylib ABI; future external extensions prefer data packs, WASM/WASI and versioned native sidecars.
- Compute Auto preference is explicitly **NPU → dGPU → iGPU → CPU**. The hierarchy is filtered by tuple qualification and may choose a lower tier when measured quality/stability/performance/policy makes it better. User manual selection overrides Auto; CPU remains installed as correctness fallback.
- Distribution: lean base package + optional verified runtime packs for relevant hardware. Vendor packs are not bundled indiscriminately.
- Cache: after benchmark, remove only unreferenced disposable losing conversion/compile artifacts; preserve immutable source model, selected optimized assets, shared/pinned assets and CPU fallback requirements.
- Authority: owner instruction integrated into the current implementation plan, ADR-0014 and ADR-0015.

## 2026-08-22 — v1.3 first-class Pipelines and lean-extensible product
- Accepted: Modules compose into versioned Pipeline graphs; triggers bind to Pipeline IDs and different Pipelines may use independent hold/toggle hotkeys.
- Accepted: normal installation/user experience remains lean; Advanced exposes module graph/provider/runtime choices.
- Accepted: deterministic text correction is guaranteed baseline; local/cloud LLM processing is optional, provider-separated, privacy-gated and semantically guarded.
- Accepted: Module/Runtime/Model/Pipeline resources have separate identities/dependencies; trusted registry metadata resolves upstream first and a Narrivy mirror where legal/useful.
- Accepted: settings/Pipeline import-export is versioned and secret-free by default; account sync is post-1.0 and local use never requires login.
- Accepted: whole-Pipeline qualification may split ASR/postprocessing across NPU/dGPU/iGPU/CPU instead of hard-coding all models to one device.
- Authority: ADR-0016 through ADR-0019 and `IMPLEMENTATION-PLAN-v1.3.md`.

## 2026-08-22 — Salvage policy: v0.4 archive is a salvage source for the v1.3 build
- Phase/work item: cross-phase (owner-approved)
- Decision: The v0.4 archive (`old/`, commit `5168619`) is Narrivy's own MIT code and may be salvaged into the v1.3 tree, but only behind v1.3 contracts (Pipeline IDs, Module/CapabilityRegistry, Engine-vs-Runtime taxonomy, resource identities) — never transplanted with its singleton structure. Old evidence never transfers to new gates; old results are expectations to re-prove. First landing: the six P0F probes, adapted (scenario hash from `spec/scenarios/*.json`, detected environment facts, truthful GNOME-lane failure instead of COSMIC-host simulation, wtype check added, shared `p0f_common.py`). Full mapping recorded in `archive/docs/management/salvage-inventory.md`; archive-verified volatile versions recorded in `agent/RESOLVED_VERSIONS.json`.
- Alternatives considered: (a) wholesale copy of old crates (rejected — reintroduces the architecture the reset removed), (b) zero reuse (rejected — discards proven platform code and hard-won host truth), (c) treat old/ as behavioral oracle only (rejected for code — that restriction applies to external products under CLEAN_ROOM_POLICY, not to Narrivy's own prior implementation).
- Reason: owner instruction 2026-08-22 ("do the salvaging, whatever can be salvaged; the project folder is the product"); maximizes reuse of proven IPC/delivery/audio/model-manager/packaging work while preserving v1.3 architecture.
- Requirements/ADRs affected: none weakened; supports P0F owner-gate checks P0F-001..P0F-009 tooling.
- Verification: `python3 scripts/validate_pack.py` after manifest reseal; probes emit schema-valid evidence on the COSMIC host; GNOME scenarios fail truthfully until a native GNOME lane exists (B-HW-003).


## 2026-08-22 — Salvage archive reorganized as code/; owner's Handy-fork NPU work admitted as code salvage
- Phase/work item: cross-phase (owner decisions)
- Decision: (1) The v0.4 archive was cleaned (17 GB of target/, node_modules, dist, stale release binaries removed; source/docs/tests/evidence and git history kept) and moved to `code/narrivy-v0.4/` — all prior references to `old/` in living documents now point there; dated log entries keep their historical wording. (2) The owner's Handy-Pop-OS fork owner-authored NPU additions (openvino-worker + Eddy, Qwen3-ASR conversion, model catalogue, runtime packaging, gate experiments) are code-salvage sources under the same adapt-don't-transplant rules as narrivy-v0.4. (3) `code/` is gitignored except `code/README.md` and excluded from the pack manifest (build/verify scripts updated).
- Alternatives considered: (a) separate salvage/ folder for the fork (rejected — owner wants one tidy archive), (b) keep full old/ contents (rejected — 17 GB of build junk), (c) drop old git history (kept for now — invisible to tidiness, preserves provenance; can be dropped later if the owner wants).
- Reason: owner instruction 2026-08-22 — clean the archive, rename to one `code/` folder, remove unneeded material before the build starts, and admit the owner-authored fork work as salvage ("this is my code and I worked on it, it's all public").
- Requirements/ADRs affected: CLEAN_ROOM_POLICY.md amended; no requirement weakened.
- Verification: `python3 scripts/validate_pack.py` green after reseal; `code/README.md` committed; grep shows no stale `old/` paths in living docs (dated history entries intentionally unchanged).

## 2026-08-22 — Archive histories wiped; Handy-fork mentions scoped to owner-authored work only
- Phase/work item: cross-phase (owner decision)
- Decision: (1) Git histories of both salvage archives deleted (`code/narrivy-v0.4/.git`, and the fork tree replaced by the owner-authored extraction `code/handy-npu/` — worker, manager, design/gate docs, experiments, cumulative patch; the public fork remains the authorship record). (2) Owner instruction: no mentions of the Handy fork's original author anywhere in this repository, because only owner-authored code from that project is used; scrubbed from CLEAN_ROOM_POLICY, DECISIONS, RESEARCH_LOG, salvage inventory, and code/README. (3) Scope explicitly limited to the Handy fork: attribution for every functional third-party module/engine/library actually used (whisper.cpp, OpenVINO, Eddy, Silero, RNNoise, PipeWire tooling, etc.) is unchanged and mandatory per their licenses and LICENSE_POLICY/THIRD_PARTY_NOTICES — those are dependencies we cannot re-create. (4) Product-name references that protect us (clean-room policy, competitive monitoring list) and the GitHub-org pointer needed to locate transcribe.cpp were retained.
- Alternatives considered: (a) keep histories (rejected by owner — fresh start, tidy archive), (b) strip license notices from retained third-party code (rejected — license obligations and our own supply-chain rules; solved instead by not retaining non-owner code), (c) remove product-name references in protective docs (rejected — they exist for our protection).
- Reason: owner instructions 2026-08-22 (wipe histories; no author mentions for Handy only; functional third-party attribution accepted as unavoidable).
- Requirements/ADRs affected: CLEAN_ROOM_POLICY.md amended (author mention removed, no-upstream-code-retained state recorded); no requirement weakened.
- Verification: repo-wide grep clean of author mentions; main-repo commit messages clean; `python3 scripts/validate_pack.py` green after reseal.

## 2026-08-22 — Model choice is the user's choice (owner principle)
- Phase/work item: product principle, applies P5/P7 (Models UI, quant picker, Auto selection)
- Decision: Cataloging, qualifying, and recommending engines/models/modules is the system's job; selecting the model actually used is the user's choice. Owner's stated preference (e.g., Qwen3-ASR daily driver) informs the shipped default recommendation and Auto weighting only — never a mandate. Every selection surface keeps: qualified-options-only menus, measured evidence per option, recommendation-as-hint, one-click override, manual-preferred/locked modes, visible Auto decisions with Reset-to-Auto.
- Reason: owner instruction 2026-08-22 ("finding the right model for me, or for anyone else, is the user's choice"); consistent with UX-003 progressive disclosure, compute-scheduler manual modes, and the 2026-08-21 quant-picker "hint not lock" decision.
- Verification: P5/P7 UI tests assert override paths (UI-PIPE-SWAP-MODULE, model-picker override, Reset to Auto); no surface may remove or hide user model selection.

## 2026-08-22 — Owner clears all pending decisions (pre-build question round)
- Phase/work item: cross-phase; removes all owner-approval blockers from the autonomous loop.
- Decisions (owner answers, verbatim intent):
  1. GNOME lane: DEFER to P11 release qualification; P0F completes on COSMIC-native evidence. Owner gate P0F amended (P0F-002 removed, baseline resealed) with consent recorded here.
  2. Real-speech evidence: owner directs that real human-speech SAMPLES may be sourced from the owner's public Handy-Pop-OS fork history (github.com/iamwaqargulzar/Handy-Pop-OS). Policy adopted: virtual-source playback of real-speech samples (owner's, provenance-verified per sample from owner-authored commits; sha256 recorded) + redistributable synthetic fixtures qualify as real-audio-path evidence for pipeline/E2E gates; one live human-speech session per lane is still recorded at P11. The agent re-clones the public fork to extract samples (local copy was cleaned); verify each sample's provenance before use.
  3. Git history: SQUASH the session commits into one checkpoint before the build starts (executed immediately after this entry).
  4. Portal permissions: pre-grant via the portal permission store where scriptable + short owner-present permission windows; truthful degradation if the owner is unavailable.
  5. OpenVINO: LOCK 2026.3 now (latest + owner-fork-verified on this hardware); LTS 2025.4.1 is the recorded bounded fallback.
  6. License: MIT is FINAL for 1.0; B-LEGAL-007 narrows to the pre-RC THIRD_PARTY_NOTICES review.
  7. Reservations about the v1.3 scaffold: NONE remaining (owner confirmed the session's work addressed them).
  8. HF org: owner is creating the `narrivy` org/repo now (guided); token arrives at the P5 owner window via secure input; local-cache fallback stands.
- Requirements/ADRs affected: P0F owner gate (amended with consent); no requirement weakened (GNOME evidence obligation moves to P11 where TEST-010/bounded-support already requires it).
- Verification: verify_owner_gates green after amendment; run_gate P0 green after squash; audit/register/handoff updated to reflect resolved decisions.

## 2026-08-22 — HF org + auth completed (B-AUTH-006 RESOLVED)
- Decision/Action: owner created the `narrivy` HF organization and ran `hf auth login` with org-scoped full-access credentials (verified logged in; org `narrivy` visible). No repos created yet; the agent publishes `narrivy/*` model mirrors at P5 per ADR-0017 (upstream-first resolution, immutable revisions, manifests + sha256). Credentials stay in the host CLI store only.
- Impact: the P5 publish path is fully unblocked with zero further owner action; the earlier "P5 token window" owner touchpoint is cancelled.
- Verification: `hf auth whoami` → Logged in, user iamwaqargulzar, orgs: narrivy (recorded 2026-08-22; no secrets captured).

## 2026-08-22 — GitHub repository registered; push is owner-gated
- Decision/Action: created the private, intentionally-empty repository
  https://github.com/iamwaqargulzar/narrivy (gh CLI; issues/wiki disabled) and
  added it as git remote `origin` locally. NOTHING is pushed. The coding agent
  must NOT push until the owner explicitly signals "push time"; that pushed
  state becomes the repository's first version / starting point. At push time
  the owner also chooses whether to squash the local history into one clean
  baseline commit (sensible for an early starting point) or push the full
  phase history (sensible at/after release qualification). Note: the first
  push triggers the pinned CI workflows on a private repo (Actions free-tier
  minutes apply); the current baseline is CI-green locally.
- Impact: repository name claimed; build process unchanged (local-first);
  publish step is a documented owner decision, not an autonomous action.
- Verification: `gh repo view` shows PRIVATE + empty; `git remote -v` shows origin; no push performed.

## 2026-08-22 — GitHub repo visibility flipped to public (still empty)
- Decision/Action: owner chose PUBLIC for https://github.com/iamwaqargulzar/narrivy while keeping it empty — the name is now publicly claimed; the first push remains owner-gated and becomes the repo's first version. Side benefit: public repos get GitHub Free Actions without private-minute limits, so the CI-minutes caveat is void.
- Verification: `gh repo view` → PUBLIC + isEmpty true; no push performed.

## 2026-08-22 — Single-artifact release: .deb only, no AppImage/portable package for 1.0 (owner scope decision)
- Phase/work item: P10 packaging / OPS-002
- Decision: Linux 1.0 ships exactly one installation artifact — the tested `.deb`. The AppImage (self-contained/portable package) is dropped from 1.0 scope and demoted to follow-on packaging alongside RPM/AUR/Nix (re-promotion requires an ADR). Implemented across: OPS-002 requirement + traceability row, P10 phase spec/work order/PHASES summary, release gate required artifact kinds (deb+sbom+provenance), release-artifacts schema (minItems 3, appimage kind removed) + templates, linux-qualification schema (package_type=deb) + template, owner gates P10/P11 (appimage scenario check removed, baseline resealed), p10-appimage-baseline scenario deleted, release-packaging/reproducible-builder/update-policy/release-gates/qualification/checklist/master-prompt/implementation-plan/packaging READMEs, ADR-0012 annotated (sidecar fallback remains valid for systemd-less environments), salvage inventory note.
- Alternatives considered: keep both artifacts (rejected by owner — wants only the normal installer), keep AppImage as optional untested extra (rejected — never ship untested artifacts; a re-promotion goes through ADR + full scenario work).
- Reason: owner instruction 2026-08-22 ("I just want it to be the normal installation file and not the portable version"); the 1.0 support lanes (Pop!_OS/Ubuntu 24.04) are .deb distros, so nothing supported is lost.
- Requirements/ADRs affected: OPS-002 text amended (owner-approved scope change); no gate weakened — release gate still fails closed without the .deb+sbom+provenance artifacts.
- Verification: verify_owner_gates green (P10/P11 re-baselined); validate_pack + traceability green; grep shows AppImage references only in historical records (dated evidence, ADR context, follow-on notes).

## 2026-08-22 — Stream-kind contract: punctuation is the Transcript→Text rendering stage
- Phase/work item: P1 pipeline contracts (found by p1-pipeline-tester observation a)
- Decision: `text.punctuation` slot I/O is (Transcript, Text) — the canonical transcript-rendering entry per the ADR-0016 reference default (ASR → punctuation → ITN → guard → delivery). `text.itn`, `text.semantic-guard`, `text.llm-postprocess` are (Text, Text). Without this, no valid end-to-end dictation Pipeline could exist (ASR outputs Transcript; delivery consumes Text) — a latent schema gap in the P1 stream-kind vocabulary.
- Alternatives considered: delivery accepting Transcript (rejected — delivery consumes final rendered text; guard must run before rendering completes), a new text.render slot (rejected — duplicates punctuation's role).
- Verification: narrivy-pipeline 52 tests green with updated spot-checks; full-chain ASR→punctuation→delivery now validates.


## 2026-08-22 — Local ASR lanes LOCKED (final, supersedes all same-day interim entries above it)
- Phase/work item: P2 walking skeleton + P5 ASR qualification
- Owner decision chain (three same-day refinements, consolidated): (1) adopt transcribe.cpp outright; (2) pause; (3) FINAL scope confirmed: transcribe.cpp is the reference ASR RUNTIME for CPU/iGPU/dGPU lanes; the NPU lane remains Qwen3-ASR 1.7B INT8 via OpenVINO (reference lane, owner daily-driver class). The formerly-planned dedicated engine/runtime entity for one well-known STT model family is REVOKED and erased from the repository as if never planned — owner explicitly authorized editing decision/research logs and any file for this revocation.
- What REMAINS: ALL model variants of that family are first-class CATALOG WEIGHTS, executed through transcribe.cpp on CPU/iGPU/dGPU (`gguf-whisper-models-transcribecpp` binding) — the family name survives ONLY in model-weights sense. Auto selection: NPU → Qwen3-ASR-openvino; dGPU/iGPU → transcribe.cpp (Vulkan); CPU → transcribe.cpp.
- Constraints carried forward: transcribe.cpp has no NPU path; young project (2026-06-30) maturity risk accepted at P5 gate; LICENSE VERIFIED MIT 2026-08-22 (see RESEARCH_LOG) — vendoring/binding permitted with notices; immutable revision pins only.
- Implementation impact: catalogs rewired (engines 27→26 rows, modules 66 bindings, the formerly-planned dedicated family runtime removed from runtimes / transcribe.cpp added); verify_architecture_map.py enforces BOTH presence (new lanes) and absence (revoked entity); engines/transcribe-cpp lands feature-gated; docs sweep executed repo-wide.
- Alternatives considered: keeping the old runtime as pinned fallback (owner: removed as planned default; fallback only if qualification shows transcribe.cpp instability); dropping the NPU lane (never asked, contradicts daily-driver oracle).
- Verification: validators green after surgery; docs-whisper-scrubber agent audit table; engines/transcribe-cpp builder report.

## 2026-08-22 — Owner gate P2-002 (GNOME-native trigger probe) deferred to P11; baseline resealed
- Phase/work item: P2 walking skeleton gate
- Decision: Executes the standing owner decision of 2026-08-22 ("GNOME lane: DEFER to P11 release qualification; P1-P10 proceed on COSMIC"; B-HW-003 RESOLVED-deferred, BLOCKERS.md). The P2 owner-gate minimum check P2-002 (p2-trigger-gnome, native_required) is removed from the P2 gate exactly as P0F-002 was; GNOME trigger evidence becomes a P11 requirement alongside the other dual-lane qualifications. COSMIC-native evidence completes P2.
- Authorization trail: owner decision recorded in DECISIONS 2026-08-22 question round (item 1) and reaffirmed in the final ASR-lane decision same day. Baseline resealed via verify_owner_gates mechanism (hash update in spec/gates/owner/BASELINE.json).
- Verification: verify_owner_gates green after reseal; p2-trigger-gnome continues to run in generators and records truthful off-lane failure (exit 3) for history.

## 2026-08-23 — Owner gate P3-005 (manual) completed via deterministic UI-audit evidence; baseline resealed
- Phase/work item: P3 desktop client gate
- Decision: The P3-005 manual minimum check ("Deferred features hidden/labeled") shipped without a verification_id, which the fail-closed gate cannot execute. Completed it by assigning verification_id VER-UX-DEFERRED-HONESTY backed by an automated audit: DeferredFeaturesAbsent vitest suite + zero rendered-source occurrences of Snippets/Integrations. Baseline resealed per the established owner-action mechanism.
- Verification: verify_owner_gates green after reseal.

## 2026-08-24 — P11 GNOME lane deferred (bounded-support) — owner gate P11-002 removed, baseline resealed
- Phase/work item: P11 qualification
- Decision: GNOME Wayland native E2E remains deferred per bounded-support.md; COSMIC Wayland is the qualified native lane for linux-1.0 release. P11-002 removed from owner gate (same pattern as P0F/P2). Evidence p11-gnome-native truthfully records lane mismatch.
## 2026-08-27 — Minimal Reis EI text sender approved only behind the uncomposed portal owner

- Phase/work item: Recovery Phase 2C non-release delivery reconstruction.
- Decision: exact-pin Reis 0.7.1 with optional features disabled and use only
  its low-level typed EI objects/wire codec. Narrivy owns a `Send`, actor-local,
  text-only handshake/event loop and rustix polling under one absolute
  deadline. The Reis convenience handshake, high-level converter, blocking
  iterator, Tokio wrapper, and `REIS_DEBUG` mode remain prohibited.
- Boundary: exact six-interface sender handshake; one printable ASCII payload
  of at most 254 bytes; one start/text/frame/stop transaction; complete flush
  means only `SubmittedUnverified`. Incomplete flush is conservatively
  `DispatchUncertain`; no target-retention or delivery claim is inferred.
- Scope: direct-zbus owns the sender after `ConnectToEIS`, but permission,
  recovery, IPC, CLI, and desktop composition remain disabled. No prompt or
  native input was run. All 98 requirements and the release remain blocked.
- Alternatives rejected: unsafe `Send` for Reis's non-`Send` converter; system
  libei 1.2 without text API; helper processes that weaken ownership/privacy;
  keycode synthesis before text capability is tested.
- Verification authority: ADR-0026 and
  `agent/baselines/recovery-phase-2c-real-sender.md`.

## 2026-08-27 — GTK3 parent export admitted only as a startup-disabled adapter

- Phase/work item: Recovery Phase 2C desktop-parent foundation.
- Decision: preserve the Tauri UI temporarily, but isolate GTK3 Wayland parent
  export in `adapters/desktop-parent-linux` with one reviewed FFI module,
  redacted opaque handles, GTK-main-thread enforcement, single-flight exact
  export/unexport ownership, bounded off-main callback waiting, and a
  shutdown-only orphan fail-safe. No desktop command, frontend action, daemon
  startup injection, pipeline, or capability is enabled by this decision.
- Dependency decision: do not suppress the GTK3/glib advisories. The adapter
  is controlled but not dependency-qualified. A separate ADR must choose a
  maintained desktop runtime, an upstream GTK4 migration when available, or a
  narrowly justified containment plan before release.
- Lifecycle decision: defer production Tauri permission ownership until exact
  cancel+revoke+original-request terminal ordering and restart behavior are
  implemented and tested. A parent handle must remain exported through that
  terminal point and must never enter TypeScript, logs, status, or disk.
- Alternatives rejected: empty/unparented portal request; ashpd raw-handle
  helper; permanent export; ignoring RustSec; enabling the daemon seam before
  the initiating-window owner exists.
- Verification authority: ADR-0028 and the independently reviewed controlled
  adapter tests. No native export, portal dialog, or delivery qualification is
  claimed.

## 2026-08-27 — Desktop portal-parent lifecycle remains a pure inert coordinator

- Phase/work item: Recovery Phase 2C desktop-parent foundation.
- Decision: model permission-parent ownership as a generic Linux-only desktop
  state machine before adding any executor or UI action. Native acceptance is
  available only through an exclusive mutable-borrow admission closure, which
  installs the ticket before returning the callback request. This removes the
  stale-acceptance race structurally.
- Cleanup decision: possible dispatch requires the original terminal plus
  cancel and unconditional revoke. Parent release precedes publication and
  replay. Local release uncertainty suppresses replay; remote uncertainty after
  confirmed local release remains non-admitting but permits bounded one-shot
  close/exit replay. Accepted callback failure is `ParentExportFailed`, never
  fabricated cancellation.
- Runtime decision: keep the coordinator compiled but unconstructed and keep
  the GTK adapter outside desktop dependencies. No command, event hook,
  frontend action, daemon injection, capability, or native claim is enabled.
- Verification authority: ADR-0029, 20 controlled FSM tests, and three
  independent read-only PASS reviews. Production executor, restart policy,
  advisory resolution, and native qualification require later work.

## 2026-08-28 — Bounded resident-ASR transfer without simulated streaming

- Phase/work item: Recovery Phase 4C resident-ASR transport optimization.
- Decision: retain the final-only resident worker and replace redundant
  utterance-sized byte vectors with fixed 16 KiB encode/read transfer buffers.
  The worker's final f32 input buffer and the parent's canonical `AudioChunk`
  remain the only payload-sized audio owners required by the current native
  API and orchestration contract.
- Streaming decision: upstream streaming is real but model-dependent. Narrivy
  will not expose or simulate partials for the final-only Whisper recovery
  model, and the catalog no longer labels the entire Parakeet family as
  streaming. A separate ADR is required before adopting a streaming model and
  changing worker/core/UI protocols.
- Alternatives rejected: one payload-sized serialized `Vec<u8>` on each side
  (avoidable peak memory); repeated final-window decode marketed as streaming
  (false capability and duplication risk); immediate model-family replacement
  without integrity, accuracy, latency, language, packaging, and lifecycle
  qualification.
- Verification authority: ADR-0033, the maximum-size controlled protocol test,
  and `agent/baselines/recovery-phase-4c-asr-transport.md`. No microphone,
  transcript, native delivery, requirement, evidence, or release claim.

## 2026-08-28 — Model streaming support is attested separately from product support

- Phase/work item: Recovery Phase 4D native model capability attestation.
- Decision: upgrade the private resident-worker handshake to protocol v2 and
  carry the loaded model's exact `supports_streaming` bit in a one-byte,
  known-bits-only payload. Preserve that attested value for the daemon lifetime
  and reject a changed value after worker restart. Status reports native model
  support separately from Narrivy's explicitly disabled/uncomposed stream path.
- Alternatives rejected: infer capability from filename/family (not
  authoritative); keep a global compile-time `false` (hides a capable loaded
  model); promote product streaming from the native bit (capture/core/worker/
  VAD/UI contracts do not yet exist); accept capability drift on restart
  (runtime behavior could change under an apparently stable configuration).
- Requirements/ADRs affected: ADR-0033 and ADR-0034. No release requirement is
  passed or changed.
- Verification: malformed/unknown readiness, supported/unsupported attestation,
  restart mismatch, status separation, real final-only Whisper handshake, and
  existing decode/cancellation tests.

## 2026-08-28 — Native streaming lifecycle belongs in the isolated worker

- Phase/work item: Recovery Phase 4E bounded resident streaming protocol.
- Decision: adopt ADR-0035. Protocol v3 adds model-gated begin/feed/finalize/
  reset operations under one exclusive worker stream identity. Feeds are
  synchronous and caller-paced; no audio or partial-result queue is introduced.
  Native snapshot revisions/finality/framing are validated before emitting
  non-empty changed partials. Final uses the native full snapshot.
- Cancellation decision: expose the scaffold ASR stream's existing shared
  cancellation token so a separate owner can cancel a synchronous native call.
  The parent observes it while waiting and kills/reaps the isolated worker;
  reset-on-Drop is bounded and uncertainty also terminates the worker.
- Product boundary: the engine/worker adapter is composed, but live PipeWire
  chunks, VAD/endpointer, daemon partial events, and desktop UX are not. Status
  remains `streaming=disabled:not_composed`; no requirement is passed.
- Alternatives rejected: buffer the whole utterance and call repeated final
  decode (simulated streaming); detach feed threads; queue unbounded audio;
  trust native revisions or UTF-8 blindly; persist/deliver partial text; enable
  the daemon merely because a model bit is true.
- Verification authority: ADR-0035 and
  `agent/baselines/recovery-phase-4e-resident-stream-protocol.md`.

## 2026-08-28 — Stream admission shares cancellation and invalidates uncertainty

- Phase/work item: Recovery Phase 4E independent-review rework.
- Decision: pass one caller-owned cancellation token into `open_stream` and
  retain it in the returned stream. Bound resident native begin separately at
  two seconds. Any begin error, cancellation, timeout, EOF, or malformed frame
  removes and terminates/reaps the worker before returning; a possibly active
  native stream is never retained without a parent owner.
- Deadline decision: checked `Instant` construction converts an
  unrepresentable caller timeout into `invalid_timeout`, never a panic.
- Privacy decision: claim only that Narrivy intentionally creates no payload
  files or ordinary-log payloads. OS crash-dump/swap containment remains an
  explicit release blocker.
- Verification authority: marker-synchronized begin/feed tests record the old
  child PID, cancel or fail the operation, require `/proc/<pid>` to disappear,
  and then prove a capability-consistent restart.

## 2026-08-28 — Moonshine Streaming Tiny is the Recovery 4F canary

- Phase/work item: Recovery Phase 4F streaming-model qualification canary.
- Decision: use the exact 50,462,816-byte Q8_0 Moonshine Streaming Tiny GGUF
  only as the smallest native protocol canary. It is not the default model or
  a release model-pack decision.
- Selection reason: exact transcribe.cpp 0.2.2 accepts default stream options;
  the model is MIT, 34M/50.5 MB, and has an 80-ms feed contract. Parakeet
  Unified/Nemotron remain later accuracy/multilingual comparators with much
  larger artifacts and family-specific latency controls.
- Integrity decision: acquire only from immutable revision
  `85ddff612fa3a2cf40b2f745abcfa90ef82f293b`, require SHA-256
  `930e4622ad3a24158b91406c30c977fa6a26b34cb32d6ac3e57cfb23383a869e`
  before load, and retain it only in the mode-0700/0600 user cache.
- Verification authority: ADR-0036 and
  `agent/baselines/recovery-phase-4f-streaming-canary.md`. The native fixture
  observation is non-release and transcript-free.

## 2026-08-28 — Live capture uses a separate fail-closed bounded transport

- Phase/work item: Recovery Phase 4G bounded live capture.
- Decision: preserve batch capture and add a separate portable live-capture
  contract. Linux emits contiguous 1,280-sample chunks into an eight-slot
  synchronous channel; saturation fails the entire stream rather than
  overwriting or silently shortening it.
- Control decision: bind the transaction `SessionControl` into the parser at
  startup. The parser observes Stop/Cancel even while the orchestration thread
  is inside synchronous native ASR. Stop retains already-accepted audio;
  Cancel discards it. The parent remains the sole child reaper.
- Ownership decision: End requires parser join and child reap. A downstream
  failure calls explicit abort with its existing absolute deadline. Child and
  parser cleanup are both attempted within that same deadline.
- Truth boundary: the parser is non-real-time; PipeWire real-time ownership is
  external `pw-record`. The legacy overwrite pre-roll ring is not lossless ASR
  transport, and its controlled structural/microbench tests are not native
  latency qualification.
- Verification authority: ADR-0037 and
  `agent/baselines/recovery-phase-4g-bounded-live-capture.md`. Daemon
  composition, microphone behavior, and release requirements remain blocked.

## 2026-08-28 — Streaming recovery reuses one downstream transaction

- Phase/work item: Recovery Phase 4H capture-to-resident-ASR composition.
- Decision: adopt ADR-0038. Portable core validates capture continuity and ASR
  event order, keeps partials volatile, and hands exactly one final transcript
  to the same policy/correction/journal/delivery tail as batch recovery.
- Selection decision: strict resident sessions select streaming only when the
  loaded model attests native support. Final-only resident models and the
  sidecar retain batch behavior. A selected stream failure never falls back to
  batch, mock ASR, or another delivery backend.
- Ownership decision: either-side failure signals ASR immediately, then
  attempts capture abort/reap/join and ASR finish within one absolute deadline.
  Cleanup failure takes precedence; lifecycle-observer rejection also cleans
  both owners before returning.
- Truth decision: status says `composed:controlled_unqualified`, never ready or
  verified. No microphone, latency, accuracy, evidence, requirement, package,
  commit, push, or release claim follows from controlled tests.

## 2026-08-28 — Endpointing is a graceful, first-origin-owned boundary

- Phase/work item: Recovery Phase 4I conservative VAD/endpointer.
- Decision: adopt ADR-0039. Keep `speech.vad` and `speech.endpoint` as
  independent portable contracts. Compose the dependency-free energy-VAD and
  sample-count endpointer only in the strict streaming recovery lane.
- Lifecycle decision: user Stop and automatic endpoint share an atomic
  first-writer graceful-finalization origin; Cancel remains independent.
  Endpointing observes but does not trim audio, and Linux returns the typed
  `EndpointDetected` capture end only when endpointing owns that origin.
- Truth decision: the defaults and all results are controlled candidates.
  Native acoustic quality, threshold calibration, word-edge preservation,
  latency, segmentation, and release requirements remain blocked.
- Verification authority: ADR-0039 and
  `agent/baselines/recovery-phase-4i-vad-endpoint.md`.

## 2026-08-29 — Partial hypotheses are volatile presentation, not transaction data

- Phase/work item: Recovery Phase 4J controlled partial-result path.
- Decision: adopt ADR-0040. Core validates raw-local policy, order, non-empty
  content and a 16-KiB bound before invoking an optional transaction-scoped
  observer. Only a final transcript can enter correction, journal or delivery.
- Backpressure decision: the daemon coalesces the newest queued partial per
  transaction. Lifecycle events evict partials first; an incoming partial is
  dropped when only lifecycle truth occupies the bounded queue.
- Privacy/authorization decision: same-UID authentication is necessary but not
  subscription. Only an explicitly accepted Subscribe receives events; stalled
  event writers are bounded and disconnected. Partial Debug/envelope output is
  redacted.
- Client decision: protocol minor 3 preserves global envelope order. The
  desktop requires global plus session/transaction/pipeline/ASR sequence
  identity, keeps tentative content only in memory, and clears it at terminal
  state.
- Truth decision: controlled tests do not pass FUN-003/PERF-005 or establish
  native model/UI latency, stability, accuracy, microphone, package or release
  behavior.

## 2026-08-29 — Daemon event dispatch is notification-driven and close-linearized

- Phase/work item: Recovery Phase 4K controlled event-pump latency/lifecycle.
- Dispatch decision: replace the fixed five-millisecond sleep with an owned
  condition-variable wake. A 250-ms timeout is defensive only, not the normal
  event schedule.
- Lifecycle decision: queue admission and terminal close share one mutex.
  Events accepted before close remain drainable; late publications are rejected,
  so no producer can enqueue behind an exited pump.
- Shutdown decision: stop/reap session workers and close IPC connections before
  closing the bus. Worker timeout or pump panic makes daemon exit fail closed.
- Metrics decision: distinguish queued eviction, incoming drop, coalescing and
  post-close rejection instead of calling all loss `dropped_oldest`.
- Truth decision: controlled scheduling tests remove one structural polling
  delay but do not qualify native audio/model/webview latency, power, packaging,
  or any release requirement. Verification authority is ADR-0041 and the
  Recovery 4K baseline.

## 2026-08-29 — Strict streaming capture precedes resident ASR admission

- Phase/work item: Recovery Phase 4L capture-first streaming admission.
- Ordering decision: after journal creation, raw-local audio policy approval,
  and streaming-capability validation, start capture and retain the first real
  chunk before publishing `Listening` and opening the ASR stream.
- Integrity decision: feed the retained sequence-zero chunk and later queued
  chunks through one exact-once continuity path. Keep the fixed bounded queue;
  overflow remains typed `capture_backpressure` rather than overwrite or skip,
  and a known producer error preempts even the retained chunk.
- Lifecycle decision: readiness or `Listening` rejection opens no ASR owner.
  Admission failure/cancellation aborts and reaps capture; cleanup failure keeps
  precedence under the existing deadline.
- Truth decision: transaction-scoped capture-first admission is not continuous
  pre-roll and controlled ordering does not qualify native clipping or latency.
  Verification authority is ADR-0042 and the Recovery 4L baseline.

## 2026-08-30 — Resident worker recovery precedes the next strict transaction

- Phase/work item: Recovery Phase 4M-A supervised resident readiness.
- Lifecycle decision: resident worker state is explicit (`Ready`, `Active`,
  `RestartRequired`, `Warming`, `FailedClosed`). ASR request admission never
  spawns a worker. One generation performs cancellable startup outside the
  owner lock; uncertain native state is reaped before restart, and capability
  drift fails closed.
- Admission decision: one RAII permit serializes strict sessions before journal
  or capture side effects. A resident strict lane also requires live `Ready`;
  restart schedules an engine-deduplicated task under the daemon's existing
  bounded supervisor and rejects the current request.
- Truth decision: status and pipeline enablement derive from live resident
  lifecycle plus attested model capability. No anonymous stream prewarm, queue
  growth, substitution, microphone, evidence, package, or release claim is
  introduced. Verification authority is ADR-0043 and the Recovery 4M-A
  baseline.

## 2026-08-30 — Recovery timings are ephemeral, monotonic, and fixed-shape

- Phase/work item: Recovery Phase 4M-B controlled timing truth.
- Measurement decision: adopt ADR-0044. Core owns ten fixed content-free stage
  boundaries and an optional infallible monotonic observer. A fixed atomic
  trace records first relative offsets without identity, payload, wall clock,
  arbitrary keys, heap samples, persistence, IPC, logs, or evidence.
- Production decision: the daemon installs no observer. Remove legacy timing
  maps, completed-record retention, JSON writes, and runtime timing-directory
  creation. Keep the optional protocol-minor-3 field absent for compatibility;
  any durable/client-visible scheme requires a new ADR and privacy review.
- Truth decision: completed boundaries are marked only after success. A failed
  correction cannot claim corrected durable commit, and a rejected partial
  cannot claim first-partial acceptance.
- Diagnostic decision: an ignored, explicit-path, hash-pinned paced WAV may
  exercise the real resident model and Inbox transaction in private temporary
  storage. It is a controlled substitution, not microphone, speech-onset,
  UI-repaint, percentile, PERF-005, or release evidence.

## 2026-08-31 — Strict system helpers use a narrow Linux process adapter

- Phase/work item: Recovery Phase 4P trusted strict system helpers.
- Boundary decision: adopt ADR-0048. `adapters/process-linux` owns only
  root-owned helper resolution, scrubbed role environments, one-shot launch,
  process groups, parent-death setup, and bounded cleanup. Platform capture and
  delivery retain product semantics; portable core/API crates remain unaware.
- Trust decision: strict `pw-record` and `wtype` are found only in reviewed
  `/usr/bin` or `/bin`, with launch-alias and canonical-target identity
  revalidated at spawn. This is not FD-bound exec, package provenance, or a
  hostile-code sandbox.
- Lifecycle decision: all three stdio streams are explicit or null; normal
  leader exit with a live same-group descendant is a typed failure after group
  cleanup. Cancellation, timeout, and drop use the caller's existing absolute
  deadline and cannot upgrade uncertain cleanup to success.
- Truth decision: capabilities distinguish trusted-helper readiness from
  microphone behavior and verified target retention. No native observation,
  evidence, requirement transition, packaging, or release claim follows.

## 2026-09-01 — Resident executable binding and shutdown are explicit

- Phase/work item: Recovery Phase 4Q-A controlled resident worker boundary.
- Trust decision: adopt ADR-0049. The controlled resident executable is bound
  once by externally supplied SHA-256 and size, copied to a sealed executable
  memfd, and reused for every recovery generation. This is not installed-package
  or release provenance.
- Lifecycle decision: recovery admission and permanent shutdown are serialized.
  Normal shutdown sends the private protocol operation, then boundedly
  terminates and reaps the process group if needed. Daemon exit aggregates
  supervisor, resident and event-pump results without erasing uncertainty.
- Truth decision: status, ListPipelines, StartSession and SelectPipeline consume
  the same stored resident snapshot. Available/enabled/composed claims require
  both live Ready and controlled digest binding; package binding remains
  unavailable.
- Scope decision: model descriptor/FD binding, packaging/SBOM provenance,
  cgroup qualification, native behavior and release evidence remain separate
  phases.

## 2026-09-01 — Resident model bytes are descriptor-bound

- Phase/work item: Recovery Phase 4Q-B controlled resident model boundary.
- Trust decision: adopt ADR-0050. The service supplies exact model digest and
  size; the Linux process adapter copies verified bytes once into a mode-0400
  sealed memfd and admits only that descriptor through the worker's otherwise
  close-on-exec boundary.
- Attestation decision: private protocol v4 requires the exact digest, size and
  inherited descriptor path. The immutable worker independently checks path
  form, seals, mode, size, digest and GGUF identity before native CPU load.
- Truth decision: daemon status, listing, selection, admission and recovery use
  the retained worker/model snapshot. Neither source pathname is reopened, and
  both bindings must be controlled before a strict lane can be enabled.
- Scope decision: model-manager installation identity, package/SBOM provenance,
  large-model memory budgeting, native behavior and release evidence remain
  blocked for 4Q-C or later.

## 2026-09-01 — Model presence is not installed-model truth

- Phase/work item: Recovery Phase 4Q-C1 verified installed-model observation.
- Trust decision: adopt ADR-0051. A derived model path can report only
  `PresentUnverified`; an `InstalledModelBinding` requires exact registry
  identity, size, digest, bounded sidecar metadata, and regular non-symlink
  leaves.
- Boundary decision: the portable result is an observation, not a retained
  descriptor. Linux runtime admission still requires the immutable descriptor
  snapshot and independent worker attestation from ADR-0050.
- Input decision: reject unsafe registry path components, unknown registry
  fields, malformed pins, unsupported sizes/sources, and unsafe cache roots
  before acquisition or path composition. The live version-1 registry remains
  a non-authoritative compatibility catalog.
- Truth decision: package/SBOM provenance, the normative model manifest,
  signatures/license approval, completed-directory atomic publication, native
  qualification, evidence, and release status remain blocked.

## 2026-09-01 — Installed runtime authority remains layered

- Phase/work item: Recovery Phase 4Q-C2 installed runtime manifest binding.
- Trust decision: adopt ADR-0052. The fixed root-owned manifest binds only the
  exact worker digest and size at the fixed installed path. It is neither
  package membership nor external release provenance.
- Type decision: the process and ASR-engine layers expose only generic
  `TrustAnchorDigestBound` state. Manifest and registry attribution occurs
  solely inside daemon composition after it consumes the Linux adapter's
  opaque runtime and model newtypes together.
- Configuration decision: installed composition requires transcribe.cpp plus
  one bounded registry model ID and rejects controlled worker/model path,
  digest, and size identity. It performs no network fetch or fallback.
- Truth decision: capabilities separately report worker manifest binding,
  registry-observed model binding, unavailable package bindings, unavailable
  release provenance, and unqualified behavior. All release requirements
  remain blocked.

## 2026-09-01 — Package input staging is not package provenance

- Phase/work item: Recovery Phase 4Q-C3 isolated installed-runtime staging.
- Scope decision: adopt ADR-0053. Stage only the fixed C2 worker and manifest
  leaves into a new descriptor-anchored package root; do not invoke or repair
  the legacy Debian builder in the same phase.
- Safety decision: output creation and cleanup retain stable parent/root
  descriptors, explicit creation ownership, nofollow identity checks, fixed
  non-recursive deletion, restrictive-umask repair, and leaf-to-parent fsync.
- Truth decision: successful builder-user staging is exact byte/layout proof
  only. It is not root ownership, archive/dpkg membership, installation,
  SBOM/signature/build provenance, behavioral qualification, or release proof.

## 2026-09-01 — Disposable archive composition is not installation

- Phase/work item: Recovery Phase 4Q-C4 disposable Debian candidate.
- Scope decision: adopt ADR-0054. Build only below a private temporary root,
  inspect the exact archive, report a content-free identity, and delete it
  before return. Never invoke or repair the legacy builder in this phase.
- Safety decision: require explicit release amd64 ELF inputs, the exact C3
  worker/manifest subtree, strict installed-mode service policy, exact
  root-owned archive membership, and two-build byte determinism. A preinstall
  script deliberately exits 1, and there is no enable/start action.
- Truth decision: the empty-cache daemon probe proves only fail-closed installed
  feature selection. No model, installation, lifecycle, provenance, native
  behavior, evidence, requirement, package retention, or release claim follows.

## 2026-09-02 — Offline model import uses bundled identity and complete-directory publication

- Phase/work item: Recovery Phase 4Q-C5A complete-directory offline import.
- Boundary decision: adopt ADR-0055. The public import surface accepts only an
  ID resolved by the registry compiled into the build. No CLI command, caller-
  parsed registry authority, network transport, or product policy is added.
- Boundary decision: portable code owns opaque bundled identity, canonical
  metadata/digest verification, and a distinct bundled installed observation;
  the Linux adapter alone owns cache/source traversal and mutation. Installed
  daemon startup and immutable Linux binding reject generic registry-derived
  observations.
- Transaction decision: on Linux, create and sync both fixed leaves in one
  descriptor-owned private staging directory, then publish the directory with
  no-replace rename and fsync its parent. Preserve divergent destinations and
  clean only descriptor-anchored owned leaves. The public operation exists only
  in the Linux adapter; no non-Linux weaker fallback exists.
- License decision: correct the bundled OpenAI Whisper-derived entries and the
  matching non-Distil transcribe catalog entries to Apache-2.0. Do not rewrite
  broader historical notices; redistribution and notice approval remain
  owner/legal blockers.
- Truth decision: controlled fixture tests establish only local transaction
  mechanics. No real model was imported and no package, installation, native
  behavior, provenance, evidence, requirement, or release claim follows.

## 2026-09-03 — Missing installed model is a typed degraded daemon state

- Phase/work item: Recovery Phase 4Q-C5B1 degraded installed-model lifecycle.
- Lifecycle decision: after the fixed-root installed runtime is verified, only
  an explicitly absent selected bundled model keeps the daemon reachable for
  truthful status and starts no resident worker or fallback. Existing but
  unverifiable, incomplete, corrupt, I/O-failed, or JSON-failed observations
  remain fatal.
- Fail-closed decision: invalid configuration, unknown bundled model identity,
  unsafe cache-root policy, invalid installed runtime, and resident startup
  failure remain fatal. Strict selection and session admission stay disabled
  while awaiting the absent model.
- Coherence decision: resident ownership is one mutex-protected lifecycle state
  (`Unconfigured`, `AwaitingInstalledModel`, `Ready`, `Closing`); initialization,
  status snapshots, and shutdown removal are atomic with respect to each other.
- Protocol decision: `StatusDto.installed_model` is an additive optional typed
  lifecycle (`awaiting_model` or `ready`) with lossless unknown-tag handling.
  Capability strings remain diagnostic detail.
- Scope decision: C5B1 adds no mutation command. Authenticated offline import,
  concurrency, post-import resident initialization, CLI/UI, and installed
  systemd lifecycle belong to later separately reviewed phases.
- Truth decision: controlled capability and admission tests are not package,
  installation, native behavior, evidence, requirement, or release proof.

## 2026-09-03 — Offline import cancellation is cooperative and commit-aware

- Phase/work item: Recovery Phase 4Q-C5B2A cooperative offline-model import.
- Control decision: the Linux import primitive accepts the shared session
  cancellation type and one absolute deadline. It checks both around bounded
  copy/hash work; the portable installed-binding verification consumes the same
  budget. Cancellation wins when cancellation and expiry coincide.
- Commit decision: descriptor-relative `RENAME_NOREPLACE` is the irreversible
  publication point. Before it, cancellation/timeout requires confirmed stage
  cleanup and parent sync. After it, the destination is never rolled back;
  cancellation/deadline may stop confirmation only and therefore yields a
  typed published-but-unconfirmed outcome rather than clean cancellation.
- Truth decision: cleanup failure, pre-publication durability/verification
  failure, and post-publication durability/verification failure are distinct
  typed outcomes. The deadline is explicitly cooperative and does not claim to
  preempt a blocking filesystem syscall.
- Scope decision: C5B2A adds no daemon IPC mutation, worker ownership,
  lifecycle activation, CLI/UI, package, evidence, or release claim. Those
  remain for separately reviewed work.

## 2026-09-03 — Immutable resident activation uses one bounded control chain

- Phase/work item: Recovery Phase 4Q-C5B2B0 bounded activation prerequisites.
- Control decision: adopt ADR-0058. Trust-anchored executable/model binding and
  production resident launch use one shared cancellation token and absolute
  deadline. Cancellation wins over simultaneous expiry.
- Performance decision: hash the executable once while constructing the sealed
  snapshot and once at the final later spawn boundary. Controlled command
  construction performs structural validation only; it does not add another
  full digest pass.
- Lifecycle decision: reserve termination time inside the caller's startup
  deadline. Transfer retained workers under a brief global lock, perform OS
  waits outside it, and restore every unresolved owner before releasing cleanup
  admission.
- Scope decision: this prerequisite adds no IPC command, provisioning state,
  model/cache mutation, CLI/UI, package, evidence, requirement, or release
  claim. C5B2B remains separately gated.

## 2026-09-03 — Offline provisioning wire authority stays with the daemon

- Phase/work item: Recovery Phase 4Q-C5B2B1 provisioning protocol.
- Protocol decision: adopt ADR-0059 and minor 5. The caller supplies only one
  bounded local source path; model identity, digest, destination, URL, policy,
  and deadline cannot cross this command boundary.
- Lifecycle decision: use a daemon-generated opaque operation ID and explicit
  state, publication, cleanup, failure, and revision fields. Only installed
  lifecycle `ready` means an active resident binding.
- Compatibility decision: add optional status and lossless wire enums, require
  minor negotiation, and defer events. Old status remains decodable.
- Safety decision: keep both commands explicitly unavailable until the daemon
  lifecycle owner and authorization/rate-limit policy are composed and tested.

## 2026-09-04 — Nested Cargo outputs are never repository inputs

- Phase/work item: dedicated repository-hygiene cleanup for B-HYGIENE-011.
- Boundary decision: replace the root-only `/target/` ignore with
  `**/target/`. Cargo output is ephemeral at every workspace depth; source,
  packaging, evidence, and manifest tooling may consume only explicit source or
  the canonical root build paths they produce in the current run.
- Cleanup decision: remove only the exact tracked
  `apps/narrivy-desktop/src-tauri/target/` tree. It contained 4,955 generated
  Cargo entries and no source; moving it to Trash keeps the local operation
  recoverable while the Git deletions remain reviewable.
- Verification decision: prove independence with a fresh isolated
  `CARGO_TARGET_DIR` all-feature locked desktop build and inspect the produced
  ELF, then discard that isolated output. Do not reseal the pack manifest or
  convert a clean rebuild into release evidence.

## 2026-09-04 — IPC command policy is daemon-global and role-blind

- Phase/work item: B-IPC-012 / SEC-005 follow-on to ADR-0031.
- Principal decision: kernel `SO_PEERCRED` UID is the only transport principal.
  Handshake role, build string, PID, executable path, caller-supplied source
  path, transaction ID and operation ID never establish application identity.
- Policy decision: ADR-0061 classifies every wire command exhaustively and
  applies a fixed-window budget keyed by exact command across all connections
  in one daemon generation. Reconnect and claimed-role changes cannot reset a
  budget; an explicit daemon restart creates a new in-memory policy.
- Intent/lifecycle decision: continue using daemon-owned session, exact
  transaction, attended portal, exact operation, awaiting-model, single-flight,
  cooldown and commit-aware state as the command constraints. These prevent
  stale/confused operations but are not credentials against a hostile same-UID
  process.
- UX/privacy decision: rate rejection is typed `busy` with content-free
  class-level detail. The policy stores only command IDs, counts and monotonic
  timestamps and cannot log transcript/path/operation/transaction payloads.
- Scope decision: hostile same-UID process isolation requires a separately
  protected broker/capability and remains outside Linux 1.0's process-identity
  claim. This change adds no release evidence or requirement PASS.

## 2026-09-04 — Desktop client behavior is independent of the shell framework

- Phase/work item: B-DESKTOP-008 / B-SUPPLY-010 maintained desktop-runtime
  migration prerequisite.
- Boundary decision: keep daemon IPC, response validation, recovery/History
  separation, capability checks, and pipeline import/export in the
  framework-neutral `bridge` module. Theme persistence and event
  subscription/reconnect/sequence handling are framework-neutral too. Tauri
  command macros and forwarding wrappers live only in `tauri_commands` while
  the current shell remains.
- Migration decision: a maintained replacement shell must reuse this tested
  client behavior rather than reimplementing daemon or pipeline policy. A
  source guard rejects Tauri API references in each reusable module.
- Lifecycle decision: the current shell retains one event-forwarder owner with
  an explicit shutdown signal and join handle. Exit requests shutdown, bounded
  event/reconnect waits observe it, and `run()` joins the worker and propagates
  a worker panic instead of detaching it.
- Scope decision: this extraction does not select GTK4/WebKitGTK 6, add native
  dependencies, remove Tauri/GTK3, solve Wayland parent export, or satisfy a
  release requirement. The 17 advisory findings remain unsuppressed.
- Verification decision: require the locked all-feature desktop tests and
  warning-denying Clippy, frontend tests and production build, then the full
  sequential workspace, boundary, traceability, governance, and packaging
  regressions before recording the seam as verified.

## 2026-09-04 — Legacy Debian and P10 evidence entry points are quarantined

- Phase/work item: B-PACKAGE-015 and B-RECOVERY-001 safety containment.
- Builder decision: adopt ADR-0062. The historical Debian builder exits 2
  before resolving inputs or touching the filesystem. Its unsafe body remains
  visible only for forensic comparison and has no environment override.
- Evidence decision: P10 now matches P9/P11 recovery quarantine. Its runner and
  refresh modules expose only a non-writing `main()` that exits 2; stale
  schema-v1 records and the retained historical `.deb` cannot be promoted.
- Verification decision: hash the protected artifact and build log before and
  after invoking the builder, and include both P10 modules in the existing
  import/CLI non-mutation governance test.
- Scope decision: this is containment, not a replacement builder. It creates
  no package, SBOM, evidence, requirement transition, manifest reseal, commit,
  push, or release claim.

## 2026-09-04 — Systemd cgroup is the outer helper-containment boundary

- Phase/work item: B-PROCESS-014 structural package prerequisite.
- Lifecycle decision: adopt ADR-0063. The canonical user service explicitly
  owns all descendants with `ExitType=cgroup` and
  `KillMode=control-group`; process-group/session escape therefore does not
  escape the service-manager cleanup target.
- Shutdown decision: keep daemon-owned graceful cleanup as the first layer,
  then bound the service stop to three seconds and require kill-on-timeout with
  final `SIGKILL`. Parent-death and process-group cleanup remain inner layers.
- Drift decision: recovery and both package service templates are exact byte
  copies. Structural tests reject removal of any containment directive.
- Truth decision: systemd unit verification is not an installed/native cleanup
  observation. Abrupt death, escaped descendants, logout, restart and packaged
  behavior remain unqualified and B-PROCESS-014 stays open.

## 2026-09-04 — GTK4/WebKitGTK 6 is the provisional desktop replacement

- Phase/work item: B-DESKTOP-008 / B-SUPPLY-010 maintained desktop-runtime
  migration.
- Runtime decision: adopt ADR-0064 additively. `narrivy-desktop-gtk` uses exact
  GTK4 0.11.4 and WebKit6 0.6.1, declares the desktop-only Rust 1.92 MSRV, and
  reuses the framework-neutral bridge, event and theme implementations. The
  exact portable package list keeps its independent Rust 1.85 contract.
- Web boundary decision: embed a bounded prebuilt React asset set, serve only
  the local `narrivy://app/` scheme, inject the entry bundle at document end,
  deny network by CSP and ephemeral proxy, reject external navigation, and
  disable unused media/capture/clipboard/window/database surfaces.
- Lifecycle decision: one bounded reply bridge owns a 64-item work queue and a
  256-item UI queue. Command and event workers run off the GTK thread, observe
  shutdown, and are joined. Command/argument and session-event contracts remain
  equal to the Tauri parity surface.
- Migration decision: retain Tauri and the GTK3 parent adapter only until GTK4
  overlay, xdg-foreign portal parenting, close/exit, clean-source packaging and
  native qualification parity pass. Do not suppress the retained 17 advisories
  or remove the daemon-owned portal coordinator early.
- Truth decision: the content-free COSMIC smoke proves a development process
  mounted React and completed `theme_get`; it is not package, release evidence,
  platform support, requirement PASS, or permission-parenting proof.

## 2026-09-05 — Absolute request-frame deadline prerequisite

Adopt ADR-0065: share one absolute deadline across each command's write/read
frame exchange and handshake frames. Recompute socket timeouts per I/O retry,
and discard uncertain transport state after request errors without replay.
Connect, active cancellation and event-wait deadline work remain before a
fully bounded portal worker can be claimed. No qualification gates weakened.

## 2026-09-11 — Compose permission lifecycle, keep delivery unqualified

- Adopt ADR-0066. Production daemon startup composes the reviewed portal
  permission service and direct driver after a read-only eligibility probe.
- The explicit permission action belongs in Text Delivery settings. It is
  capability-gated and must never run during onboarding or startup.
- GTK owns parent export/release and close replay; the daemon owns portal/EIS
  authority, revocation events, and shutdown. Raw identifiers remain Rust-only.
- Permission and transport do not promote sender or delivery capability. The
  current host lacks the portal keyboard device, so attended behavior remains a
  cross-host qualification blocker and no release evidence advances.

## 2026-09-11 — Retain an installable candidate without promoting release

- Adopt ADR-0067. Keep the unsafe historical builder quarantined and add a
  separate exact-inspected builder whose output is explicitly
  `installable-unqualified-candidate`.
- Bundle the exact registry-pinned Whisper model so first use can remain
  offline. Import into the user cache still requires an explicit Models action.
- Start the per-user daemon from the GTK command worker only after canonical
  status is unavailable. Bound both user-manager commands and readiness wait;
  never do this work on the GTK thread or for explicit qualification sockets.
- Package metadata targets GTK4/WebKitGTK 6. Root maintainer scripts do not
  attempt to operate a session user manager.
- A reproducible archive and extracted runtime smoke are prerequisites only.
  Clean installation and every P10/P11 evidence obligation remain open.

## 2026-09-14 — Persistent resident worker spawn ownership

- Phase/work item: P11 recovery, release task 011 activation acceptance repair
- Decision: Adopt ADR-0068. Keep `PR_SET_PDEATHSIG(SIGKILL)` and route every
  resident worker process creation through one persistent per-engine spawn
  owner thread whose lifetime is the engine. Bound the rendezvous by the
  caller's absolute deadline with cancellation polling, keep abandoned or
  cancelled children owned by the anchor until they are reaped, wait for owner
  quiescence before declaring shutdown complete, and join the owner on teardown
  with a bounded retirement latch.
- Alternatives considered: disable parent-death hardening for resident workers;
  keep spawning from bounded supervisor threads; keep the unjoined,
  unbounded-rendezvous anchor from the timed-out task 010 draft.
- Reason: Linux binds the parent-death signal to the spawning thread, so
  offline provisioning and recovery publication produced `ready` model
  lifecycle with a dead live worker (`restart-required`). Disabling hardening
  would remove a daemon-exit containment net; unbounded rendezvous and an
  unjoined owner would keep the ownership defect.
- Requirements/ADRs affected: ADR-0068; resident activation/readiness, bounded
  startup cancellation, and shutdown/reap contracts.
- Verification: focused engine regressions require a bounded real protocol
  operation after the requesting thread exits (initial and recovery generation)
  and fail with process state `Z` when temporary direct spawning is restored;
  bounded-rendezvous and abandoned-child ownership regressions pass; the
  hardened-child/kernel-contract regression in `narrivy-process-linux` passes
  and fails when hardening is removed. Candidate lifecycle re-qualification is
  reported separately in the P11 report.

## 2026-09-14 — One bounded daemon shutdown budget

- Phase/work item: P11 recovery, release task 003 daemon shutdown budget
- Decision: Adopt ADR-0069. Add a portable `narrivy-types::ShutdownBudget`
  (one total, one arm-at-most-once epoch, saturating remaining); arm it from the
  first `UnixIpcServer::request_shutdown`, including the signal bridge; cap IPC
  connection cleanup by the shared remaining time; add deadline-based
  `WorkerSupervisor::shutdown_all_until`, `WorkerSupervisor::cancel_all`,
  `ResidentWorkerEngine::shutdown_until` and
  `PortalPermissionService::begin_shutdown`/`shutdown_until` entry points; order
  daemon cleanup as portal admission closure plus cancellation, `cancel_all`,
  resident cleanup, portal join, supervised-session join, retries and bounded
  pump/signal joins, all on one 2500 ms total below the unchanged
  `TimeoutStopSec=3s`; bound every thread join and report an unfinished owner as
  failure while keeping its handle. In the resident spawn anchor, acquire the
  retained registry with a deadline-bounded `try_lock`, record the tightest
  caller deadline, hand unresolved children and unfinished owner threads to the
  process-wide fallback owner with a sticky unconfirmed count that only a
  confirmed reap clears, close admission before lock acquisition and re-check it
  after a recovery permit.
- Alternatives considered: enlarge `TimeoutStopSec`; detach cleanup threads and
  exit success; compute each owner's timeout from `remaining()` without a shared
  epoch; cancel without joining.
- Reason: independently bounded owners accumulated past the packaged stop
  budget, the resident anchor could detach an unreaped child or an unfinished
  owner thread behind the parent-death signal, and a session waiting on the
  resident owner delayed that owner's cancellation. Bounding the total and
  refusing to claim unconfirmed cleanup keeps shutdown truthful without
  enlarging the unit budget or weakening owner cancellation.
- Requirements/ADRs affected: ADR-0069; ADR-0068; daemon shutdown/restart,
  resident lifecycle and bounded cancellation contracts. The previous
  duration-based APIs remain as wrappers, so no external caller breaks.
- Verification: `narrivy-types` budget regressions; `narrivy-ipc-unix` shared
  budget bound plus unchanged idle endpoint tests; `narrivy-core` shared-deadline
  and `cancel_all` regressions; resident concurrent-reap, forced-reap-failure,
  exhausted-retirement fallback and exhausted-shutdown admission/ownership
  regressions; real `narrivyd` subprocess TERM/INT with an active supervised
  session plus the unchanged idle signal/restart cases; strict Clippy. These are
  mock-lane/controlled-fixture results, not native model or platform
  qualification.

## 2026-09-14 — Canonical built-in mode identities for the strict installed composition

- Phase/work item: P11 recovery, release task 006 real microphone Raw/Clean mode
  policy.
- Decision: Adopt ADR-0070. Make `raw` and `clean` the pipeline identities of
  the strict installed microphone/ASR composition. Resolve one `AdmittedPipeline`
  (canonical id + deterministic correction + lane) in `select_pipeline` before
  any side effect and consume it for strict single-session admission, journal
  admission, delayed `Listening`, session/partial events, batch orchestration and
  the owned worker; the worker composes one corrector from that admitted value
  and both `RecoverySlice` and `StreamingRecoverySlice` use it, so Raw is
  `CollapseOnly` and Clean is the existing deterministic `FullCorrect` over the
  same owned capture/ASR/privacy/finalization/delivery path. Treat
  `linux-mic-inbox`/`linux-mic-wtype-inbox` as composition configuration
  (`NARRIVY_RECOVERY_MODE`) only: they are no longer selectable or advertised
  pipeline ids, the mock lane stays non-strict, and `NARRIVY_DEFAULT_PIPELINE`
  must name `raw` or `clean` under a strict composition (hard startup error
  otherwise). Advertise exactly the two modes gated by static plus live resident
  readiness, publish `pipeline.clean`/`pipeline.raw` capability tokens, and
  migrate packaged units and qualification helpers to
  `NARRIVY_DEFAULT_PIPELINE=clean`.
- Alternatives considered: keeping the lane id as the pipeline identity with a
  new protocol mode field; advertising the lane id next to both modes; silently
  aliasing the lane id to Clean; leaving Raw as a UI-only presentation state.
- Reason: the strict composition previously accepted only the lane id and always
  constructed `DaemonCorrector(FullCorrect)`, so the real installed path had no
  Raw mode, the correction was resolved twice, and journal/partial/advertising
  identities disagreed with what a session actually executed. Lane ids are
  process configuration, not portable pipeline identity; the built-in P7 modes
  already own the canonical ids and shapes.
- Requirements/ADRs affected: ADR-0070; ADR-0021/ADR-0022 lane composition
  (unchanged); FUN-021, FUN-027; the raw/corrected/delivered variant contract
  (FUN-005) and the deterministic-processing contract (FUN-027). No requirement
  state changes.
- Verification: daemon selection/capability regressions for canonical modes,
  lane/mock/unknown rejection before side effects, and strict advertisement
  (default plus resident-feature builds); controlled strict-composition
  regressions assert exact `collapse_only` vs `correct::correct` outcomes for
  ASCII, Unicode and multiline input, batch/streaming variant parity, durable
  journal/recovery-result and partial-event identity, and that a second mode
  cannot bypass strict single-session admission. An expected-red run with a
  temporarily hardcoded `FullCorrect` composition fails the Raw case. Checker
  and packaging contract tests use the canonical ids. These are controlled
  fixtures, not native microphone/model/delivery or release qualification.

## 2026-09-15 — Stop-controlled bounded dictation capture with a durable capture end reason

- Phase/work item: P11 recovery, release task 007 usable short and long
  dictation capture.
- Decision: Adopt ADR-0071. Move the portable capture contract into
  `narrivy-types` (`CAPTURE_SAMPLE_RATE_HZ`, `MAX_CAPTURE_SECONDS = 300`,
  `MAX_CAPTURE_SAMPLES`, and the serializable `CaptureEnd` enum re-exported by
  `narrivy-platform-api`) and make the documented maximum the production default
  for `EngineConfig::pipewire_capture_seconds`. Reclassify
  `NARRIVY_CAPTURE_SECONDS` as a controlled developer probe that may only
  shorten one session within 1..30 s. Let the strict `PipeWireCapture` batch and
  producer-owned streaming sessions accept any window inside
  `[1, max_capture_seconds()]` and enforce `<= MAX_CAPTURE_SAMPLES` before any
  process side effect. Bind the resident protocol `MAX_SAMPLES`, parent batch
  validation, stream accumulation, worker decode/feed and portable core
  finalization to that one finite frame budget without changing the wire shape
  (compatibility is fail-closed peer capability, not truncation). Record
  `captured.end`/`summary.end` on the recovery journal entry before ASR,
  correction or delivery; expose it additively as the optional `capture_end`
  payload field of `GetRecoveryEntry` at protocol minor 6; and render the
  reported `capture.window_seconds`/`capture.max_seconds` limit plus the typed
  end notice in the desktop Dictation screen.
- Alternatives considered: raising only the adapter window and leaving the
  resident 30 s bound; daemon-owned segmentation of long captures (FUN-100
  scope); keeping the 3 s default with an explicit opt-in; unbounded capture
  with Stop as the only limit; reinterpreting `SourceEnded` as success.
- Reason: the shipped batch path could not record longer than the 3 s default
  and could not be configured past 30 s, while independent 30 s bounds in the
  resident protocol would have rejected any longer capture even if configured.
  The typed end reason existed but was discarded before durable finalization, so
  the user could not be told why recording ended and `SafetyLimit` unit tests
  could not prove it. A single documented finite bound preserves bounded
  memory/IPC while making ordinary long dictation work.
- Requirements/ADRs affected: ADR-0071; ADR-0021/ADR-0022 lane composition
  (unchanged); ADR-0070 mode identity (unchanged); FUN-021 dictation; FUN-100
  remains post-1.0 roadmap and is explicitly not satisfied by this bound.
- Verification: controlled-fixture and portable-slice tests only —
  platform adapter window validation plus paced 40 s batch Stop, explicit
  maximum-window termination with all 4,800,000 samples, and a >30 s streaming
  delivery with contiguous sequences; `narrivy-core` long-capture-to-ASR,
  over-bound fail-closed and durable end-reason regressions; `narrivy-types`
  bound/wire-tag tests; `narrivy-ipc` additive-field and pinned-minor tests;
  daemon policy default/override/capability tests; frontend capture-policy
  rendering tests. Native microphone, installed-model, delivery and release
  qualification remain open.


## 2026-09-17 — Reviewed task009 source composition (ADR-0072)

- Codex accepted the bounded routed component after independent review corrections: immutable admission-time unavailable target binding, removal of late PATH-resolved target/clipboard helpers, SHA-256 transaction replay binding with fail-closed capacity, and strict portable report validation rejecting Inbox as verified external commit.
- DeepSeek performed implementation and corrections at provider reasoning=max; Codex integrated against preserved file baselines, preserving task007 capture/source-end changes and the accepted frontend races. Isolated STATE and colliding ADR-0071 were not copied.
- The installed default remains linux-mic-inbox + inbox. The routed configuration is explicit; live target provider, contained clipboard mechanisms, target-application/native qualification and release evidence remain open. No requirement promotion or push.
- Independent combined checks: core111; platform270 pass/1 diagnostic ignored; resident daemon all targets; strict combined core/platform/resident Clippy; frontend78/30 plus build; packaging43 plus27 subtests. Default daemon full tests and strict Clippy recorded in artifacts/parallel-delivery-009/combined-default-daemon.txt.

## 2026-09-17 — Atomic installed toggle activation and truthful shortcut fallback (task015)

- Phase/work item: P11 release task 015, common atomic daemon/CLI activation
  seam plus truthful activation capability/probe contract.
- Decision: Add the additive `ToggleSession { pipeline_id?, activation }`
  request at protocol minor 7 (`TOGGLE_SESSION_PROTOCOL_MINOR = 7`) with the
  closed `ActivationSourceDto` (`installed_cli_command` | `desktop_client` |
  `portal_global_shortcuts`, unknown tags preserved) and the typed
  `ToggleResultDto` outcome (`started` | `finalization_requested` |
  `already_stopping`). `StartSession`/`StopSession` wire shapes and meanings are
  unchanged. The daemon decides toggle atomically under one new
  `DaemonState::toggle_admission` gate that also covers explicit `StartSession`:
  with no session it runs the existing strict readiness/admission/start path
  verbatim (shared `start_session` function, same single strict permit and the
  same `admit_session_request`); with a `Preparing`/`Listening` session it
  requests graceful finalization exactly once (first user reason wins, never
  `cancel_active_sessions`); in any other live state it returns the typed
  `already_stopping` outcome without opening a second session. `narrivy-cli
  toggle [--pipeline raw|clean]` sends exactly one such request (unsupported
  modes fail before connecting) and prints only typed content-free identity;
  the packaging-documented user-managed shortcut command is
  `/usr/bin/narrivy-cli toggle`. The GlobalShortcuts probe now uses the crate's
  direct zbus boundary against the canonical lowercase object path
  `/org/freedesktop/portal/desktop`, queries exactly
  `org.freedesktop.portal.GlobalShortcuts`/`version` inside one 750 ms timeout,
  maps missing-interface D-Bus errors (`InvalidArgs`,
  `UnknownInterface`/`UnknownProperty`) to `Absent`, and never resolves a PATH
  helper or infers from a desktop name; `probe_portal_interface_with` keeps the
  injected deterministic seam. Status advertises three separate content-free
  facts — `activation.atomic-command`,
  `activation.portal-global-shortcuts`, `activation.portal-listener` — the
  last `unavailable:not_composed` until a real owned portal listener exists.
  Frontend onboarding shows the exact command only when the daemon reports the
  atomic command available and no live listener, labelled as a user-managed
  suggestion that is not configured and cannot be confirmed until a later
  check observes it.
- Alternatives considered: status-then-start/stop in the CLI (rejected: not
  atomic and can lose the stop); a free-form activation policy string (rejected:
  unbounded caller policy on the wire); toggling through
  `cancel_active_sessions` (rejected: destroys accepted audio); inferring
  portal support from an OS/desktop name or from `busctl` output (rejected:
  PATH helper + wrong object-path casing made the old probe report a missing
  interface on every host); treating an interface probe as a live listener or
  showing the command as configured (rejected as untruthful).
- Reason: the current native COSMIC host exposes XDG RemoteDesktop v2 but no
  GlobalShortcuts interface (recorded measurement: `RemoteDesktop.version = 2`,
  GlobalShortcuts `Properties.Get` answers `InvalidArgs`, "No such interface"),
  while COSMIC Settings does support user-created command shortcuts. The
  previous probe could never report presence because it introspected
  `/org/freedesktop/portal/Desktop` through a PATH-resolved `busctl`, so the
  capability was silently unavailable everywhere. An installed atomic command
  plus truthful, separate capability facts gives users a working activation
  path without claiming a portal listener Narrivy does not own.
- Requirements/ADRs affected: FUN-001/FUN-031 activation capability exposure
  (still `blocked`: no live listener and no native shortcut qualification);
  ADR-0031/ADR-0061 command policy (toggle registered as state-changing with
  the start rate budget); ADR-0024/ADR-0027 portal composition unchanged;
  ADR-0069 shutdown budget unchanged and shutdown IPC admission remains
  dominant; ADR-0070 mode identity unchanged (toggle carries the same
  selectable `raw`/`clean` identity); ADR-0071 graceful finalization semantics
  reused unchanged; ADR-0072 admission-time target binding still explicitly
  unavailable. Protocol doc updated in `spec/protocols/ipc.md`.
- Verification: `narrivy-ipc` protocol tests (minor pin, closed activation
  source, typed outcomes, older Start/Stop compatibility); daemon default and
  resident suites including idle-toggle single admission, recording-toggle
  graceful stop without cancel, concurrent double toggle, late-toggle typed
  outcome, fail-closed unsupported/malformed mode, and separated capability
  facts; CLI tests against a real local transport counting exactly one
  `ToggleSession`; IPC unix shutdown-admission rejection of a late toggle;
  trigger tests for canonical path, absent/present/timeout classification and
  the no-helper structural regression; frontend activation truth tests plus
  frontend build and strict Clippy for the changed packages. These are
  component/contract checks only, not native qualification: the live portal
  listener, admission-time target provider, native shortcut activation and
  release qualification remain open, and no requirement was promoted.


## 2026-09-17 — Atomic toggle validates before mutation

- The additive ToggleSession path is accepted only after validation of the closed activation source and pure pipeline catalog selection before the admission lock, toggle decision, or session-control mutation. Unknown wire tags remain losslessly decodable for compatibility but fail execution with a fixed content-free BadRequest.
- Start-only readiness remains after the atomic decision: it gates idle starts but cannot prevent a valid toggle from gracefully finalizing already accepted audio. The same admitted pipeline value is consumed once by the start path.
- The installed fallback command is `/usr/bin/narrivy-cli toggle`; portal interface availability and listener readiness remain distinct facts. No live listener or native shortcut qualification is claimed.

## 2026-09-21 — Runtime refresh and clean-room use of external inference-server ideas

- Decision: refresh the locked resident ASR dependency pair to `transcribe-cpp`/`transcribe-cpp-sys` 0.2.3 and the optional OpenVINO runtime-pack baseline to 2026.4. Every release artifact and native qualification must be regenerated because the source/runtime identity changed.
- OpenVINO device truth: Qwen3-ASR CPU/GPU is supported by the 2026.4 release; NPU is candidate-only unless Narrivy qualifies the exact converted model, runtime, driver and hardware tuple. Generic NPU discovery never promotes the model.
- XeBoostLM and OpenArc are research references only. Their useful behavior is independently implemented through Narrivy's existing provider/runtime, health, planner, worker-lifecycle and telemetry contracts. No code or implementation structure is copied. Product UI need not display project names; repository attribution is mandatory if licensed source is later incorporated.
- Release scope: do not add a new server or Python runtime during P11. Preserve the lean resident transcribe.cpp CPU path and evaluate optional OpenVINO/provider additions after the exact package release gate is green.
## 2026-09-28 — Portable crates receive host and cache-protection facts by injection
- Phase/work item: Portable-crate boundary remediation for `narrivy-compute` and
  `narrivy-model-manager` (P5 compute planning, SEC-002/SEC-007 model cache).
- Decision: Adopt ADR-0074. `narrivy-compute` keeps *which* probe answers
  *which* question as request values and receives *how* to run one through an
  injected `HostProbe` with no default implementation.
  `narrivy-model-manager` keeps the owner-only obligation as contract wording
  and receives the platform's way of establishing it through an injected
  `CacheFilePolicy` with no default. Linux implementations live in the existing
  `adapters/platform-linux`; no new adapter family is created.
- Alternatives considered: delete the 0600/0700 model cache to make the crate
  portable (rejected — a private model cache is a privacy control, and the
  boundary rule is about moving the mechanism, not the guarantee); a permissive
  default policy or probe (rejected — a default that reports "nothing here" or
  "protection applied" converts an unprobed or unprotected host into a false
  claim); `adapters/process-linux` for the host probe (rejected — it is scoped
  to no model-selection policy and accepts only `/usr/bin` and `/bin`, which
  excludes a vendor pack installed under `/usr/local`); a `LIBRARY_LEAF_ALLOWLIST`
  entry for `narrivy-compute` (rejected — it is a P5 domain surface with an
  identified daemon consumer, and an allowlist entry would assert the opposite);
  leaving `qualify_decode` spawning processes (rejected — it is the same defect
  class the guard exists to catch, merely unflagged by the current patterns).
- Reason: the guards were reporting real defects, and the honest fix is a seam
  that keeps the decision in the portable crate and the mechanism in the
  platform, so each becomes testable without the other.
- Requirements/ADRs affected: ADR-0074; PORT-006 qualification and device
  selection; SEC-002/SEC-007 model cache protection; the portable-crate
  boundary enforced by `scripts/verify-boundaries.py` and
  `scripts/verify_dependency_boundaries.py`.
- Verification: `scripts/verify-boundaries.py` passes for all 12 protected
  packages; `cargo test` for `narrivy-compute` (64) and `narrivy-model-manager`
  (45) passes with no test removed from either crate; `cargo clippy
  --all-targets -D warnings` is clean for both; hermetic coverage asserts the
  exact program, arguments, environment and budget of every probe, the
  owner-only obligation at every created cache path, the typed refusal when a
  platform cannot protect a file, and the POSIX owner-only bits on disk.

## 2026-09-28 — Windows/macOS portability and AMD NPU are post-release side quests

- Decision (owner, explicit instruction 2026-09-28): Windows, macOS and AMD
  hardware work is removed from the Linux 1.0 release scope. It starts only
  after Linux 1.0 is fully built, tested and released.
- Changes:
  - `PORT-001` (portable crates compile on Windows/macOS) moves from P0
    release-gated to post-1.0 roadmap phase R13, where `PORT-007` already owns
    native Windows/macOS work. `spec/acceptance/P0.json` and the owner gate
    `spec/gates/owner/P0.json` drop it; `BASELINE.json` is updated by this owner
    action.
  - `PORT-004` keeps its Linux 1.0 obligation (transport hidden behind a
    portable abstraction; same-user Unix domain sockets on Linux). Its
    Windows named-pipe clause moves to `PORT-007`.
  - `B-HW-004` (AMD Ryzen 7 8700G XDNA NPU) becomes a post-release side quest.
    Linux 1.0 makes no AMD accelerator claim.
- Not changed: `PORT-002/003/005/006/008` stay release-gated. `PORT-006` is
  proven architecturally (replaceable compute modules, CPU path) and needs no
  AMD hardware. The CI Windows/macOS matrix job stays in `.github/workflows/ci.yml`
  as informational; it is not release evidence.
- Follow-up the same day (owner): the first release is qualified on **Pop!_OS
  COSMIC Wayland only**. Order afterwards: other Linux distributions (Ubuntu
  GNOME first), then Windows, then macOS, each after the previous release is
  properly tested. Changes: `BUILD_TARGET.json` primary platform is Pop!_OS
  only (schema `primary_platforms.minItems` 2 → 1); `scripts/release_gate.py`
  requires only the `cosmic-wayland` native lane and a summary naming Pop!_OS;
  `linux-qualification.schema.json` `environments.minItems` 2 → 1; TEST-010
  acceptance names Pop!_OS COSMIC plus CPU fallback; bounded-support,
  charter, P11 phase spec and blockers B-HW-003/B-HW-004 updated. The P11 owner
  gate already treated GNOME as non-blocking (amendment 2026-08-24); the
  release gate now agrees with it. This narrows supported-environment
  claims; it does not relax any per-requirement evidence rule.
- Plan: `agent/completion-plan/07-post-release-side-quests.md`.
- Verification: `python3 scripts/verify_traceability.py`,
  `python3 scripts/verify_phase_model.py`, `python3 scripts/verify_owner_gates.py`,
  `python3 scripts/validate_pack.py` and `python3 -m pytest -q tests/governance`.

## 2026-09-28 — Open-source readiness (owner decisions D15–D20)

- Decision (owner): adopt `agent/completion-plan/08-open-source-readiness.md`.
  - **D15:** do it. Phase A runs as WP1b and finishes before the WP5
    freeze; Phase B runs before the first public push; Phase C runs after
    the release.
  - **D16:** keep a trimmed `agent/` public, archiving historical material
    under `archive/`.
  - **D17:** contributions use DCO sign-off.
  - **D18:**
    - repository and homepage `https://github.com/iamwaqargulzar/narrivy`;
    - security reports through GitHub Private Vulnerability Reporting;
    - conduct reports to the maintainer through GitHub.
  - **D19:** plain markdown docs for 1.0.
  - **D20:** the first public push is a single squashed commit, made by the
    owner.
- Constraints: archive rather than delete. Gate-referenced paths change in
  the same commit as their references. No change to evidence rules, and no
  push by the agent.

## 2026-09-29 — Owner answers D3–D6, D8, D9, D11–D14 (completion plan)

Full text: `agent/completion-plan/OWNER-DECISIONS.md`.

- **D3:** Maintainer `Waqar Gulzar <iamwaqargulzar@gmail.com>`; Homepage
  `https://github.com/iamwaqargulzar/narrivy`.
- **D4:** measure quantized `base.en` and make it the default only if it
  meets latency targets; otherwise keep `whisper-tiny.en-Q8_0`.
- **D5:** no autostart by default and no global enable. The app offers an
  opt-in "Start Narrivy at login" (`systemctl --user enable`). The CLI
  starts the daemon on demand.
- **D6 and D11:** one durable History keeps every dictation, delivered or
  not. Undelivered entries are flagged, with Copy and Retry. The size limit
  is by entry count: default 10, range 0–1000; 0 keeps nothing. It replaces
  the tmpfs Inbox and the 7-day time retention.
- **D8:** remove the Tauri shell after moving its shared files into the GTK
  crate.
- **D9:** the owner personally reviews `THIRD_PARTY_NOTICES.md`; approval is
  recorded with the file and SBOM hashes.
- **D12:** an optional, context-aware "convert spoken numbers to digits"
  post-processing step. It has a global setting (off by default) and a
  per-model override, and models that already write digits (whisper) keep
  it off unless overridden.
- **D13:** aliases (FUN-041) ship in 1.0, with a Dictionary screen. Privacy
  zones (PRIV-003) move to post-1.0; the traceability amendment is to be
  written by the agent.
- **D14:** FUN-007 = leave. A microphone picker and a no-mic error ship in
  1.0, and FUN-011 is restated to match. FUN-006, FUN-012, REL-005 and
  REL-009 move to post-1.0; the traceability amendment is to be written by
  the agent.
- The scope reductions (PRIV-003 and the D14 audio requirements) are owner
  decisions. They are not to be applied silently: the agent updates the
  traceability rows, docs and procedures in a reviewed commit citing this
  entry.

## 2026-09-29 — Documentation restructure; owner-gate baseline refresh (path-only)

- Under owner decision D15 (open-source cleanup, Phase A), `docs/` is
  reorganised by audience: `product/`, `architecture/`, `platforms/`,
  `security/`, `development/`, `maintainers/`, `roadmap/` and `adr/`.
  Stale process and management material moves to `archive/docs/`. Every
  reference in live files is rewritten. `scripts/check_docs.py` verifies
  that relative links resolve. Historical evidence records keep their
  original paths.
- `spec/gates/owner/P0F.json` and `P1.json` list ADR and phase-doc artifact
  paths. Only those path strings changed (`docs/11-adrs/…` → `docs/adr/…`,
  `docs/09-phases/…` → `docs/maintainers/phases/…`). No check, command or
  requirement changed. Their `BASELINE.json` hashes are refreshed for this
  path-only change.
- The two pinned recorders that mention doc paths
  (`record_cpu_performance_evidence.py`,
  `record_native_accessibility_evidence.py`) are re-pinned in
  `spec/evidence-verification-profiles.json` after the same path-only edit.

## 2026-09-29 — D21: the locked speech-engine pipeline applies to Linux 1.0 (owner)

- The owner confirmed that the pipeline locked on 2026-08-21 (and reaffirmed
  2026-09-21) in `agent/RESOLVED_VERSIONS.json` applies to 1.0:
  - NPU (OpenVINO 2026.4);
  - then a GPU pool: Vulkan (transcribe.cpp), OpenVINO GPU, and
    CUDA/ROCm/TensorRT via ONNX Runtime where present;
  - then CPU (transcribe.cpp) as the mandatory last resort;
  - the three Whisper model tiers, delivered from the narrivy Hugging Face
    org with pinned, verified artifacts;
  - the NPU compiled cache, and quantization selection with a
    recommendation.
- The completion plan wrongly treated 1.0 as CPU-only and asked the owner
  again; this entry corrects that. The 2026-09-28 note "Linux 1.0 makes no
  accelerator claim" applies only to AMD hardware. Release claims cover
  tuples qualified on the reference Intel machine; other tuples are
  self-tested on users' machines.
- Plan: `agent/completion-plan/02-product-fixes.md` T2.15, T2.16 and T2.17.

## 2026-09-29 — D22: Tauri 2 desktop host on every OS; the daemon is kept (owner)

- Decision: follow Handy. Tauri 2 (2.11.5+) is the desktop host on Linux,
  Windows and macOS. It is a client of `narrivyd`; the daemon, portable
  core, delivery safety, History and helper isolation are unchanged.
- ADR-0075 supersedes ADR-0064. The GTK4 shell and
  `adapters/desktop-parent-linux` are retired after parity.
- Security: upgrade first, then each remaining advisory is listed by ID in
  `deny.toml` with its reason. The owner accepts this risk; it is
  re-evaluated at each Tauri release, and GTK4 / WebKitGTK 6 is adopted
  when Tauri ships it officially.
- Also adopted from Handy: the `gtk-layer-shell` overlay, the Linux typing
  and paste ladder, `handy-keys` evdev press-and-hold (`input` group), and
  `cpal`/`enigo` for Windows/macOS.
- Measured on 2026-09-29: the retained Tauri shell builds, with 9
  advisories (6 unmaintained, 3 DoS in `quick-xml`/`time`).

## 2026-09-30 — D23: Module Store, Refine screen, light noise suppression (owner)

- Store and Refine become sidebar entries. 1.0 ships a built-in light noise
  cleanup; hardware-specific modules come through the Store after 1.0.
- Refinement language model: local only first.
- 1.0 Store shows Narrivy Verified items only; everything is hosted in the
  Narrivy Hugging Face organisation (ADR-0017, T2.17 unchanged).
- Design: `docs/roadmap/design/module-store.md`.

## 2026-09-30 — D24: shortcut capture on COSMIC (owner)

- Narrivy may add and remove its own entries in the desktop's keyboard
  shortcuts; capture (click, press keys) is the first option and the
  copy-a-command route the second.
- A combination already in use is explained, with both choices offered:
  pick another, or replace after explicit confirmation.
- COSMIC first; other desktops after 1.0. Press-and-hold is unchanged (D2).
- Design: `docs/roadmap/design/shortcut-capture.md`.

## 2026-09-30 — D26: one OpenVINO worker, one shared language model, writing assistant (owner)

- With the OpenVINO add-on:
  - noise suppression, speech recognition and the language model run in
    one OpenVINO worker, each on its own self-tested device;
  - D25 is unchanged: Vulkan stays the default without the add-on.
- One language model is loaded and shared by dictation Refine and the
  writing assistant. It is never loaded twice, and dictation has priority.
- Writing assistant:
  - a Chrome and Brave extension using native messaging to a limited
    Narrivy host (no local web port);
  - a rule checker for underlines;
  - the shared model for fix, improve and rewrite;
  - the ADR-0018 guard, with preview and undo.
- Owner answers:
  - the writing assistant ships after the release (SQ-WRITE);
  - the language model also runs on Vulkan (default), CUDA (add-on) and
    macOS hardware acceleration, making it a global feature;
  - English is the default, and other languages are the user's choice
    (the list is not decided yet).
- Design: `docs/roadmap/design/writing-assistant.md`.

## 2026-10-01 — D27: the Store holds accelerators and modules; models stay on Models (owner)

- `ListStore` lists runtimes and modules only. Speech models, including
  the OpenVINO packs, are installed and removed on the Models screen.
- Accelerators: Vulkan shown as built in; OpenVINO installable; CUDA shown
  as coming later with whether an NVIDIA GPU is present.
- Refine (small language-model specialists) and noise suppression modules
  from `spec/catalogs/modules.json` are shown as coming later.
- Details: `agent/completion-plan/OWNER-DECISIONS.md` D27.

## 2026-10-19 — D36: Windows comes forward; macOS is set aside (owner)

- Windows work (SQ2) may start now, alongside the Linux release; it no
  longer waits for the Pop!_OS release to be tested. Amends D7.
- macOS (SQ3) is parked: no work for now.
- Linux 1.0 stays the release target; `BUILD_TARGET.json` is unchanged.
- Details: `agent/completion-plan/OWNER-DECISIONS.md` D36.
