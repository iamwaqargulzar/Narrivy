# Narrivy v1.3 — Autonomous Implementation Plan

> **Current authority:** this file supersedes earlier build-pack implementation plans. Earlier implementation plans are superseded and intentionally omitted from this distribution.

## v1.3 architecture clarification

Before implementation, read `docs/00-ARCHITECTURE-MAP.md` and the machine-readable catalogs under `spec/catalogs/`. The coding agent must preserve the separation between Feature, Pipeline, Module, Engine, Runtime, Model and Provider. The Linux 1.0 OpenVINO lane is admitted per qualified model/runtime/driver/device tuple; OpenVINO 2026.4 officially supports Qwen3-ASR on CPU/GPU while NPU remains candidate-only. transcribe.cpp is the reference CPU/iGPU/dGPU ASR runtime (GGUF model variants incl. every Whisper model variant as weights).

> **Purpose:** This is the implementation plan for the future coding agent. The repository remains a scaffold until that agent is intentionally started. Follow `SOURCE_OF_TRUTH.md` and the phase gates; this plan refines execution order and architecture but does not waive any owner gate.

## 1. Product distinction

Narrivy must not become another generic “speech-to-text + clipboard paste” utility. Linux 1.0 differentiates on:

1. **Recoverable dictation:** final text is durably recoverable before risky delivery; failure ends in Inbox/recovery, not data loss.
2. **Replaceable modules:** major capability families are behind stable internal contracts rather than hardwired implementations.
3. **Qualified adaptive compute:** hardware is benchmarked/qualified, user-overridable, and safely falls back.
4. **Linux capability truth:** supported/degraded/unsupported states come from real probes and release evidence.
5. **Local-first privacy:** no network or content telemetry by default.

## 2. Architectural rule: modules compose into first-class Pipelines

Narrivy's main extension hierarchy is:

**Modules → Pipelines → Triggers/Profiles/Policy**

A Pipeline is a versioned typed graph and a stable persisted object. Multiple Pipelines can coexist, each with its own trigger bindings, module choices, privacy/provider policy, hardware preferences and bounded fallbacks. Hold/toggle hotkeys bind to a Pipeline ID. Do not create a singleton “dictation pipeline” in core.

The normal built-in Pipeline is deliberately lean:

`hotkey → mic → lightweight denoiser → VAD → local ASR (qualified Vulkan/general GPU path where useful, CPU fallback) → deterministic corrector → reliable delivery`

Reserve Smart Local and Cloud Polish recipes as optional dependency graphs; do not bundle their large models/providers into the base installer.

## 3. Architectural rule: module first, plugin later

Design the code so implementations can be replaced without changing domain logic. Initial Linux 1.0 implementations may be compiled-in crates; the contracts are the important part.

Required replaceable capability families:

- activation/global-trigger provider;
- microphone/audio-capture provider;
- audio preprocessing chain;
- VAD provider;
- noise suppression / denoise provider;
- ASR engine;
- compute/runtime provider;
- text normalization/processor stages;
- target identity/context provider;
- text-delivery backend;
- model/catalog source;
- secure-secret provider;
- diagnostics exporters that never bypass redaction.

Do **not** expose Rust trait-object/dylib ABI as a third-party contract. For external extensions, prefer:

- data-only packs for dictionaries/modes/models;
- WASM/WASI for portable untrusted processors where latency permits;
- versioned sidecar IPC for crash-prone or native high-performance extension workers;
- a narrow C ABI only if a measured requirement makes it necessary and an ADR defines ownership, versioning, panic/exception/allocator boundaries, and sandbox consequences.

## 4. Lean installation and runtime packs

### Base installation

The base `.deb`/AppImage contains only the application/service, discovery/diagnostic code, the reference CPU correctness path, and broadly useful baseline execution support that is legally redistributable and reasonably small. Large model weights and vendor-specific accelerator stacks are not bundled indiscriminately.

A fully offline setup may be offered as a separate optional offline bundle; do not inflate the normal installer solely to satisfy offline provisioning.

### Runtime packs

Optional runtime packs are immutable/versioned packages managed by Narrivy and described by a manifest. Examples include Intel OpenVINO/NPU, AMD XDNA/Ryzen AI, or future vendor runtimes. Packs are installed only after:

1. hardware/capability discovery;
2. compatibility check against OS/driver/application/runtime versions;
3. explicit network policy/user authorization for downloads;
4. hash/signature verification as supported;
5. staged install and self-test;
6. qualification evidence.

A runtime pack may be removed independently when no installed/selected model depends on it.

## 5. Compute selection contract

### Auto mode

Candidate exploration order is:

**NPU → dGPU → iGPU → CPU**

The hierarchy is a preference, not permission to choose a broken or slower path. A candidate is eligible only if the tuple is qualified. The planner scores eligible tuples using at least:

- model/quant compatibility and transcript quality;
- stability/self-test history;
- real-time factor and final latency;
- cold/warm load or compile latency;
- memory/VRAM/NPU memory;
- power/battery policy where measurable;
- current device load/thermal pressure where available;
- user profile and explicit preference.

### Manual mode

The user may choose an exact model/runtime/device path. Validate it before activation. Provide:

- **Manual preferred:** use requested path when healthy; safe fallback remains available.
- **Manual locked:** do not silently change the selected path; surface a clear error/recovery choice if it becomes unavailable. CPU remains installed even when not automatically selected.

Every automatic result is visible, e.g. `Auto → Intel NPU / OpenVINO / model X`.

## 6. Benchmark and cache hygiene

Benchmarks use project-owned/non-private fixtures and run only the bounded candidates necessary for selection. Track every generated artifact with ownership metadata.

After qualification/selection:

Keep:
- immutable original/source model artifact;
- selected converted/compiled artifact(s);
- CPU correctness fallback model/runtime assets;
- shared artifacts referenced by another qualified path;
- user-pinned artifacts;
- minimal benchmark summary/evidence.

Delete when safe:
- temporary staging files;
- failed/corrupt conversions;
- compiled/converted variants created solely for losing benchmark paths;
- stale cache entries invalidated by runtime/driver/model keys.

Never delete by broad directory heuristics. Cleanup must be reference-aware, transactional/restart-reconcilable, and observable in diagnostics. Offer a developer setting to retain benchmark artifacts and a user-facing `Clear unused optimized cache` action.

## 7. Text post-processing and provider contract

Keep deterministic text correction as the guaranteed local baseline. Optional LLM processors are separate nodes with Faithful/Polish/Rewrite semantics, semantic token guards, explicit local/cloud policy, and fallbacks to deterministic output. `text-postprocessor` and `inference-provider` are separate capability families.

For local accelerator users, whole-Pipeline qualification may compare OpenVINO ASR + OpenVINO postprocessor on NPU versus split-device plans. Do not assume co-residency is best merely because both models compile.

## 8. Resource distribution and settings portability

Separate Module Pack, Runtime Pack, Model Pack and Pipeline Pack identity. Resolve resources through trusted immutable metadata with upstream source first and Narrivy mirror where legal/useful. Model mirrors/conversions may live in the Narrivy Hugging Face organization; executable modules/runtime packs should use an artifact distribution channel appropriate for signed versioned binaries. Never execute an arbitrary mutable URL.

Implement versioned settings/Pipeline import/export. Default exports exclude secrets, history/audio, host qualification caches and compiled benchmark artifacts. Import previews missing dependencies before any downloads. Account-backed sync is explicitly post-1.0; local use never requires login.

## 9. Execution order

### Preflight — v1.3 scaffold integrity

Before product implementation, run:

- `python3 scripts/doctor.py --json`
- `python3 scripts/validate_pack.py`
- `python3 scripts/verify_owner_gates.py`
- `python3 scripts/verify_traceability.py`

Initialize/confirm Git identity and make a clean checkpoint. Do not modify owner-gate minima merely to make a phase pass.

### P0F — prove Linux feasibility

Implement disposable real probes before architecture hardens:

- global activation on native COSMIC and GNOME Wayland;
- target identity/context capability truth;
- PipeWire microphone lifecycle/hotplug/suspend;
- text delivery across bounded target classes and recovery fallback;
- Tauri/WebKitGTK build/launch boundary.

No pass-shaped stub evidence. A probe is `fail`/`skip` until the real scenario has run.

### P1 — stable portable contracts

Implement session state machine, typed errors/outcomes, cancellation, transaction IDs, recovery journal, IPC, migrations, privacy policy, first-class Pipeline persistence/validation, distinct resource identities, and the internal **ModuleRegistry/CapabilityRegistry**. Contracts should support replacement but remain `internal-evolving` until proven by P2–P5.

### P2 — smallest genuine walking slice

Build in increments, always keeping the last slice working:

1. trigger → mic → CPU ASR → Inbox;
2. Inbox/recovery → one proven direct-delivery target;
3. bounded fallback chain + Unicode/multiline;
4. second desktop lane.

Do not build polished UI before this path works with real audio and real ASR.

### P3 — thin desktop client

Tauri/React is a client of `narrivyd`, never the owner of active capture/ASR state. UI exposes actual capabilities and resolved Auto decisions. Linux 1.0 navigation is Home, Dictation, Models, Applications, History, Settings, Diagnostics. Deferred features are not shown as working placeholders.

### P4 — audio reliability and processing modules

Harden PipeWire capture, hotplug/restart, non-blocking real-time behavior, audio restoration, VAD and preprocessing contracts. Ship one conservative lean preprocessing chain first. Keep explicit capability slots for AEC, beamforming, dereverb, denoise, source separation, target-speaker extraction, audio tagging and separate endpointing; evaluate optional implementations later rather than hardwiring them. Noise suppression must be replaceable through the module interface; external third-party loading is not required for 1.0.

### P5 — production ASR + runtime-pack manager

Implement reference ASR, model manager, RuntimePackManager, candidate qualification, Auto/manual selection, cache ownership, cleanup, and CPU fallback. Add accelerator modules only when each tuple passes real qualification.

### P6–P9 — beta and hardening

Establish end-to-end performance baselines, preserve the real smoke path, add the lean Fast Dictation/Raw-Clean experience, saved Pipelines, independent hotkeys, deterministic correction, optional guarded LLM post-processing contract, settings/Pipeline import-export, privacy/history/dictionary essentials, then attack failure cases. Add plugin permission/sandbox tests even if the public plugin SDK is not shipped yet, so future extension boundaries are not unsafe by construction.

### P10–P11 — package what was qualified

Produce the `.deb` (single 1.0 installation artifact per owner decision 2026-08-22 — no AppImage) from reproducible builders. Release metadata includes checksums, SBOM, provenance, dependency/license inventory, signing status, bounded support, target-app matrix, performance evidence, CPU fallback and accelerator claims. Qualify release artifacts rather than source/dev builds.

## 10. Engineering practices

- Favor small vertical commits and test-driven fixes for regressions/state logic.
- Keep domain crates free of Tauri/Linux/vendor runtime types.
- Prefer structured typed outcomes over stringly success/failure.
- Make cancellation and stale-result rejection explicit at every async boundary.
- No blocking I/O/inference on capture/UI critical threads.
- Use staged downloads/conversions and atomic publish.
- Treat all imported models/modules/packs as untrusted archives.
- Pin/record volatile versions at implementation and release time; do not bake August 2026 assumptions into permanent logic.
- Failure paths must preserve recoverable text and never silently broaden permissions.
- A module crash/failure should degrade to another qualified module or a user-visible recovery state, not crash the daemon.

## 11. Stop conditions

The coding agent does not call the product complete because it compiles, launches, recognizes speech once, or shows a polished UI. Completion is the configured release gate with release-artifact qualification on the bounded Linux lanes.
