# ADR-0016 — First-class pipelines and trigger bindings

**Status:** Accepted  
**Date:** 2026-08-22

## Decision

Narrivy treats a **Pipeline** as a first-class, versioned user/configuration object. A Pipeline is a typed processing graph plus fallback, compute, privacy, and delivery policy. Individual Modules are composed into Pipelines; Modes are friendly built-in recipes/presets over Pipeline definitions rather than a separate execution architecture.

Triggers bind to **pipeline IDs**, not to a single global “start dictation” action. A user may bind multiple triggers to one Pipeline or give different Pipelines different triggers. Hold-to-talk and toggle are independent trigger semantics; each is exposed only where the platform capability proves the required press/release behavior.

## Required properties

- Multiple saved Pipelines may coexist.
- Pipeline graphs are validated before activation: node capability, typed I/O, audio format/channel constraints, permissions, local/cloud boundary, model/runtime dependencies, latency metadata, and fallback behavior.
- Pipeline changes occur between sessions unless a stage explicitly supports safe hot reconfiguration; never swap a speech-enhancement/ASR/text-transform implementation halfway through an utterance by default.
- A Pipeline may inherit from another versioned Pipeline recipe and override selected nodes/settings, but export resolves enough information to remain migratable and auditable.
- A failed optional stage follows the Pipeline's bounded fallback policy. Basic dictation must not become dependent on optional “smart” stages.
- The UI exposes a simple default experience first and puts graph editing behind Advanced controls.

## Consequences

This allows Fast Dictation, Smart Local, Cloud Polish, Coding, Translate, Meeting, or future pipelines to share the same core while using different graphs and hotkeys. It also prevents the shortcut layer, ASR engine, or UI from becoming the owner of pipeline semantics.
