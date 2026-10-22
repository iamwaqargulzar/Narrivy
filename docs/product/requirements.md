# Product Requirements

---
> **Legacy planning amendment (2026-08-21):** Linux 1.0 scope clarified per Planning Correction Parts C–D and Supplemental Integration §2–§51. Requirements marked *(Post-1.0)* are **not** Linux 1.0 release blockers; they are tracked in `spec/requirements-traceability.csv` as `post-1.0` / `roadmap` (R12/R14). Audio policy (FUN-007) is reduced to `leave + at least one crash-safe duck/pause` for 1.0; global activation (FUN-001) exposes toggle as supported and PTT only where P0F proves key-release semantics. See `agent/PHASES.json` (now includes P0F) and `docs/maintainers/phases/phase-0F-feasibility.md`.



**Authority:** requirement IDs in this file are normative. Implementation status/evidence lives in `spec/requirements-traceability.csv`.

**Linux 1.0 scope:** requirements explicitly described as roadmap/1.x/later are traced but are not Linux 1.0 release blockers. The release target must not grow silently.

## Product goal

Narrivy must make voice input feel faster and more dependable than typing for suitable tasks, without requiring users to understand speech-model infrastructure.

## Functional requirements

### Core dictation
- **FUN-001** Global activation: toggle is the supported Linux 1.0 activation path; push-to-talk (hold) is exposed only where P0F proves key-release semantics on the desktop/session, with truthful capability reporting.
- **FUN-002** Capture microphone audio without blocking the UI/input event loop.
- **FUN-003** Stream tentative transcription where the selected engine supports it.
- **FUN-004** Deliver final text with capability-based backend selection and bounded fallback (semantic/direct, transactional clipboard, portal/libei, virtual keyboard, etc.); Unicode/multiline correctness is per-backend capability, Inbox is the final non-destructive sink, and broad synthetic-input privilege (uinput/input-group) is not the default onboarding path.
- **FUN-005** Preserve raw, corrected, and delivered transcript variants per committed session.
- **FUN-006** Optional in-memory audio pre-roll to prevent clipped first syllables.
- **FUN-007** Configurable audio policy for Linux 1.0: `leave` must work; at least one crash-safe useful policy such as `duck` or `pause` must be independently proven, with restoration/watchdog coverage. Other policies (`mute`/`smart`/additional) ship only when they meet the same safety standard, otherwise deferred post-1.0.
- **FUN-008** Recover unsent dictation after process failure where feasible.
- **FUN-009** Dictation Inbox fallback when no writable target is available.
- **FUN-010** User can cancel an active session without stale output.
- **FUN-011** Microphones support deterministic ranking, explicit user override, hotplug detection, and bounded fallback/recovery.
- **FUN-012** Playback streams that appear while duck/pause/mute policy is active inherit the current session policy without polling races.
- **FUN-013** Final text delivery applies deterministic target-aware cursor spacing/punctuation rules so repeated dictation does not create glued words or destructive blind whitespace changes.

### Intelligence and writing
- **FUN-020** Modes are declarative pipelines, not UI-only presets.
- **FUN-021** Built-in Raw and Clean modes in 1.0.
- **FUN-022** Message, Professional, Email, Developer, Prompt, Compose, Notes, Translate and Custom modes in 1.x.
- **FUN-023** *(Post-1.0 — deferred per the legacy planning amendment Part D)* App Profiles select a default mode/policy based on active application/context.
- **FUN-024** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Selected-text transformations support replace + reliable undo.
- **FUN-025** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Smart self-correction/backtracking: phrases such as “actually”, “scratch that”, “make that” can repair recent speech.
- **FUN-026** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Confidence-directed correction can repair low-confidence spans without reprocessing the entire transcript.
- **FUN-027** Deterministic text processing is preferred over LLM calls for simple formatting/rules.
- **FUN-028** Context-aware decoding/biasing is used when an ASR backend supports it.
- **FUN-029** Spoken language, written language/script, translation target, and locale/formatting convention are distinct capability-driven settings; unsupported combinations are surfaced rather than guessed.
- **FUN-030** Saved Pipelines are first-class versioned typed graphs; multiple Pipelines can coexist and compose independently replaceable capability modules with explicit bounded fallbacks.
- **FUN-031** Trigger bindings target a Pipeline ID; users can assign different Pipelines independent toggle/hold hotkeys, with hold exposed only where press/release capability is proven.
- **FUN-032** Text post-processing separates deterministic correction from optional local/remote LLM processing; raw text remains recoverable and generative output is semantically guarded before delivery.
- **FUN-033** The out-of-box Pipeline is lean and straightforward (light enhancement/VAD, local ASR with qualified general GPU/Vulkan path where useful plus CPU fallback, deterministic correction, reliable delivery) while Advanced users can replace compatible modules without changing core.

### Personalization
- **FUN-040** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Scoped dictionary: global, language, app, mode, project, team-ready.
- **FUN-041** Spoken aliases/pronunciations map to canonical forms.
- **FUN-042** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Correction Memory can propose learning repeated user corrections.
- **FUN-043** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Learning weights decay with staleness and scope.
- **FUN-044** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Snippets support static text and typed variables.
- **FUN-045** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Shareable Mode/Profile/Pack format is versioned.
- **FUN-046** Smart templates with typed fields and field navigation are a post-1.0 workflow capability.
- **FUN-047** Domain packs can bundle vocabulary, pronunciations, Modes, snippets/templates, and deterministic rules in a validated versioned format after 1.0.
- **FUN-048** Structured dictation into schema-aware forms/JSON/tables is a post-1.0 capability.
- **FUN-049** Patch-based streaming insertion may apply stable transcript deltas instead of replacing the entire tentative region after 1.0.

### Context
- **FUN-060** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Context Fabric supports providers for app, window, selection, surrounding text, clipboard, project/repository, personal vocabulary and future integrations.
- **FUN-061** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Every context provider declares sensitivity and permission requirements.
- **FUN-062** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Modes request only required context fields.
- **FUN-063** *(Post-1.0 — deferred per the legacy planning amendment Part D)* Project Context may index filenames, symbols, dependencies and repository terminology with user control.
- **FUN-064** Context must never be sent remotely unless policy explicitly permits it.

### Voice interaction roadmap
- **FUN-080** Command Mode supports a typed safe action API.
- **FUN-081** Hybrid Mode combines dictation and explicit command grammar.
- **FUN-082** Spell Mode supports literal character/phonetic entry.
- **FUN-083** Dynamic “What can I say?” exposes context-specific commands.
- **FUN-084** Voice Workflows are permissioned, typed multi-step actions.
- **FUN-085** Anchored Dictation can keep a target while user views another app where platform APIs permit.
- **FUN-086** Number/grid UI overlays are a later accessibility capability, not a 1.0 requirement.

### Meetings/media roadmap
- **FUN-100** Long-running meeting/session capture.
- **FUN-101** Microphone + optional system audio capture with explicit permissions.
- **FUN-102** Speaker diarization; optional speaker recognition later.
- **FUN-103** Export TXT/Markdown/JSON/SRT/VTT.
- **FUN-104** Optional local summary, decisions, tasks, questions and commitments.
- **FUN-105** Live captions and subtitle-file workflows reuse the same media pipeline.
- **FUN-106** An optional phone-as-microphone companion may pair over the local network using authenticated encrypted transport with explicit revoke/reset semantics after 1.0.

### Ecosystem roadmap
- **FUN-120** A public extension SDK is post-1.0 and must derive from stable internal contracts with explicit sandbox/permission/versioning policy.
- **FUN-121** Optional remote/local-network compute nodes are post-1.0 and require authenticated encrypted transport plus the same per-request privacy/policy checks as local provider selection.
- **FUN-122** Optional team/shared knowledge packs and policy hierarchy are post-1.0 and must not complicate or disable single-user offline Narrivy.
- **FUN-123** *(Post-1.0)* Optional account-backed encrypted settings/Pipeline sync must preserve full account-free local operation and sync portable intent rather than heavyweight binaries or host qualification caches.
- **FUN-124** *(Post-1.0)* Shadow Pipeline evaluation may compare an alternative graph against the same consented utterance without delivering its output, then recommend changes using measured quality/latency/resource evidence.

## UX requirements
- **UX-001** First successful dictation must be achievable without exposing runtime/backend jargon.
- **UX-002** Linux 1.0 default navigation: Home, Dictation, Models, Applications, History, Settings, Diagnostics. Modes/profiles/dictionary live within these surfaces; deferred Snippets/Integrations are not shown as working top-level placeholders.
- **UX-003** Advanced technical controls use progressive disclosure.
- **UX-004** Overlay never steals focus in normal dictation.
- **UX-005** Overlay reflects listening/processing/error only when useful; avoid flashing transient stages.
- **UX-006** Local/cloud processing state is visible.
- **UX-007** User-facing errors provide recovery action, not just technical detail.
- **UX-008** Search/command-palette access to settings/actions.
- **UX-009** Destructive AI transformations have one-step undo.
- **UX-010** Capability tests can be launched individually from Diagnostics.
- **UX-011** Settings and Pipelines support versioned full or partial import/export; secrets and machine-specific qualification caches are excluded by default and imports show required resource downloads before activation.

## Performance requirements
- **PERF-001** Measure hotkey→listening, speech→partial, release→raw, release→committed separately.
- **PERF-002** Target hotkey acknowledgement <30 ms on supported reference hardware.
- **PERF-003** Target capture-ready <50 ms for a warm running service.
- **PERF-004** Target visible listening feedback <100 ms.
- **PERF-005** Target first partial <250 ms when engine/platform supports streaming.
- **PERF-006** Keep audio capture real-time thread free of blocking I/O and model inference.
- **PERF-007** Model residency states: Hot, Warm, Cold.
- **PERF-008** Cache compiled/optimized artifacts with robust invalidation/ownership metadata and safely remove unreferenced benchmark-generated losing variants while retaining source, selected, shared/pinned and CPU-fallback assets.
- **PERF-009** Avoid unnecessary device transfers and duplicate audio conversion.
- **PERF-010** Pipeline can prepare context/model/output concurrently.

## Reliability requirements
- **REL-001** Session state machine is explicit and testable.
- **REL-002** Failure of overlay must not terminate audio/ASR service.
- **REL-003** Failure of one text-delivery backend triggers bounded fallback.
- **REL-004** Suspend/resume and device hotplug are supported lifecycle events.
- **REL-005** Audio duck/pause state is restored after clean exit and abnormal failure wherever technically possible.
- **REL-006** Active session journal allows recovery of unsent results.
- **REL-007** Every background worker has cancellation and shutdown semantics.
- **REL-008** Support report excludes transcript/audio/context content by default.
- **REL-009** Temporary audio duck/pause/mute mutations have crash-safe restoration metadata/watchdog behavior and are repaired after abrupt termination wherever the platform makes reliable restoration possible.

## Portability requirements
- **PORT-001** *(post-1.0, R13 — owner decision 2026-09-28)* Portable core crates compile on Linux, Windows and macOS CI targets.
- **PORT-002** OS-specific APIs exist only in platform adapters.
- **PORT-003** Desktop/frontend is a client of application APIs, not the owner of core domain behavior.
- **PORT-004** IPC transport is hidden from domain logic behind a portable abstraction; Linux uses same-user Unix domain sockets. (The Windows named-pipe transport is post-1.0 under PORT-007.)
- **PORT-005** Capabilities drive UI instead of OS-name branching.
- **PORT-006** Compute/runtime backends are replaceable modules behind capability contracts; include CPU plus qualified NPU/dGPU/iGPU paths (Vulkan/OpenVINO/vendor runtimes as applicable) without core coupling or a public Rust dylib ABI.
- **PORT-007** Post-1.0 native Windows and macOS proof clients must reuse the portable domain/protocol contracts rather than fork/rewrite product logic, including a Windows named-pipe IPC transport behind the same abstraction and compile proof of the portable crates on Windows/macOS (PORT-001).
- **PORT-008** Module Packs, Runtime Packs, Model Packs, and Pipeline Packs are distinct versioned/resource-addressed concepts with explicit dependencies, compatibility, reference counting, and no core coupling to download location.

## Privacy/security requirements
- **PRIV-001** Local processing is default.
- **PRIV-002** History retention is user-configurable and can be disabled globally or by Privacy Zone.
- **PRIV-003** Privacy Zones override Mode/Profile/plugin cloud permissions.
- **PRIV-004** Cloud-bound data is declared before use and visible in UI.
- **PRIV-005** Secrets are never stored in regular config files.
- **PRIV-006** Logs/diagnostics redact content by default.
- **PRIV-007** Remote transcript processing and remote surrounding-context processing are separate opt-in permissions resolved per Pipeline/Profile/Privacy Zone; a text-only provider never receives audio implicitly.
- **SEC-001** Plugin permissions are explicit and enforced.
- **SEC-002** Model/plugin manifests support checksum/signature verification.
- **SEC-003** Network access from sandboxed extensions is deny-by-default.
- **SEC-004** Supply-chain and dependency-license checks are release gates.
- **SEC-005** Local IPC accepts only the intended local user/session and applies least-privilege endpoint permissions plus peer validation where the OS supports it.
- **SEC-006** Imported model/plugin/pack archives are treated as untrusted and reject path traversal, unsafe links, and integrity mismatches before extraction/use.
- **SEC-007** Downloaded executable/modules/models resolve through trusted registry metadata with immutable revision/version and expected hashes/signatures where supported; arbitrary mutable URLs never gain executable trust merely by being referenced.

## Accessibility requirements
- **ACC-001** Full keyboard operation.
- **ACC-002** Screen-reader labels and semantic roles.
- **ACC-003** Reduced motion.
- **ACC-004** Scalable text/UI and high-DPI correctness.
- **ACC-005** High-contrast compatible palette and focus visibility.
- **ACC-006** Critical state is not conveyed only by color or animation.
- **ACC-007** Hands-free roadmap must not compromise normal privacy/reliability defaults.

## Testability requirements
- **TEST-001** Every release-gated requirement maps to a named automated or manual verification and evidence location.
- **TEST-002** Architecture-boundary tests inspect source and dependency metadata so forbidden framework/OS/runtime coupling cannot silently enter portable crates.
- **TEST-003** Core session, policy, pipeline, IPC protocol, and fallback contracts have deterministic unit/contract tests independent of real hardware.
- **TEST-004** Linux integration tests exercise global-trigger, capture lifecycle, audio restoration, text-delivery routing, and target-application compatibility on reference sessions.
- **TEST-005** ASR/model/device qualification records correctness, streaming/final behavior, timestamps where supported, warm/cold behavior, failure/fallback, and quality/performance baselines.
- **TEST-006** Failure-injection tests cover daemon/client loss, text-delivery failure, device loss, corrupt cache/model metadata, and interrupted sessions without losing recoverable text.
- **TEST-007** Accessibility and UI acceptance include keyboard-only, focus, semantics, scaling, contrast, reduced-motion, error/recovery, and representative assistive-technology checks.
- **TEST-008** Phase and release gates fail closed: missing tests/evidence/status cannot be interpreted as a pass.
- **TEST-009** The Integrated Dictation Beta gate requires a real Linux vertical slice: global trigger → microphone → real local ASR → processing → external-target delivery or Inbox fallback.
- **TEST-010** Linux 1.0 qualification requires release-artifact E2E evidence on the Pop!_OS COSMIC/Wayland reference environment plus CPU fallback. *(Owner decision 2026-09-28: Ubuntu/GNOME and other distributions follow after the Pop!_OS release is properly tested.)*

## Operations/release requirements
- **OPS-001** A non-destructive doctor reports host/toolchain/desktop/audio/portal/compute capabilities and actionable remediation without exposing user content.
- **OPS-002** Linux 1.0 produces a tested `.deb` as its installation artifact (owner decision 2026-08-22: single-artifact release — no AppImage/portable package for 1.0). AppImage/RPM/AUR/Nix are follow-on packaging unless promoted by an ADR.
- **OPS-003** Install, launch, upgrade, rollback/recovery, and uninstall preserve or remove user data according to documented policy and never silently destroy history/configuration.
- **OPS-004** Release artifacts have checksums, dependency/license inventory, SBOM, immutable build identity, and signing/provenance where release infrastructure permits.
- **OPS-005** Release notes list verified supported environments/capabilities and known limitations; untested compatibility is never presented as supported.
- **OPS-006** Application and configuration/model schema migrations are versioned, testable, backward-safe where promised, and recoverable from interrupted migration.
