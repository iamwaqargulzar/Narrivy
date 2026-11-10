# First-class Pipeline Profiles

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

## Principle

**Modules → Pipelines → Triggers/Profiles/Policy.**

A Module implements one capability. A Pipeline composes compatible Modules into a typed graph. Triggers and application/profile rules select a Pipeline. Policy may further restrict which graph nodes/providers are allowed at runtime.

## Default experience

The normal user should install Narrivy and use one lean built-in Pipeline without learning graph terminology:

`hotkey → microphone → lightweight denoiser → VAD/endpointing → local ASR (qualified Vulkan GPU path when useful, CPU fallback) → deterministic text corrector → recovery journal → safe delivery`

Vendor-specific NPU stacks, large models, LLMs, advanced denoisers, diarization, translation, speaker extraction, etc. are optional resources.

## Built-in recipes

The build should reserve at least these recipe identities even if optional dependencies are acquired later:

- **Fast Dictation** — lean local deterministic path; default.
- **Smart Local** — Fast Dictation + optional local LLM faithful cleanup/guard.
- **Cloud Polish** — local ASR + explicitly authorized remote post-processing.
- **Custom** — user-edited graph behind Advanced UI.

Post-1.0 recipes can include Coding, Translate, Meeting, Target Speaker, or accessibility workflows without changing core execution semantics.

## Trigger binding

A Pipeline may have zero or more bindings. Examples:

- `Ctrl+Space` toggle → Fast Dictation;
- `Ctrl+Shift+Space` hold → Smart Local where key-release semantics are proven;
- CLI/hardware/app-rule trigger → any configured Pipeline.

Trigger code emits a typed `{pipeline_id, activation_kind, event}` intent. It never owns session state or processing logic.

## Graph validation

Every node declares capability, typed input/output, format constraints, statefulness, expected latency/lookahead, permissions, local/cloud boundary, compute/resource requirements, and failure policy. Audio nodes additionally declare sample rate/channel geometry/playback-reference requirements. Invalid graph compositions are rejected before activation.

## Runtime selection

A node can be `auto`, `manual-preferred`, or `manual-locked`. Auto resolves a qualified implementation and device. The optimizer may score an entire graph because two individually fast nodes can perform poorly if they contend for one accelerator.

## Fallback

Fallbacks are explicit and bounded. Examples:

- local LLM → deterministic cleanup;
- remote LLM unavailable → local deterministic cleanup;
- NPU ASR fails → qualified GPU → CPU;
- direct text delivery fails → transactional clipboard → Inbox.

No optional smart stage may make recoverable basic dictation unavailable.
