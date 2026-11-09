# System Architecture Overview

## Architectural style

Narrivy uses a **portable domain core + capability contracts + replaceable modules/adapters/engines + long-running local service + thin clients**.

```text
 ┌───────────────────── clients ─────────────────────┐
 │ narrivy-desktop (GTK4)  narrivy-cli   overlay     │
 └──────────────────────┬────────────────────────────┘
                        │ versioned local IPC
                 ┌──────▼──────┐
                 │  narrivyd   │ composition/lifecycle
                 └──────┬──────┘
                        │
   ┌────────────────────▼─────────────────────────────┐
   │ portable application/domain                     │
   │ Session │ Pipeline │ Policy │ Context │ Recovery│
   └─────────────┬─────────────────────┬──────────────┘
                 │ contracts           │ contracts
        ┌────────▼────────┐    ┌───────▼─────────────┐
        │ platform adapter│    │ ASR engine bindings │
        │ Linux / future  │    │ mock / transcribe.cpp│
        └─────────────────┘    └─────────────────────┘
```

## Process ownership

### `narrivyd`
Single owner of active session state, module/capability registry, capture coordination, model residency, pipeline/policy execution, recovery journal, capability health, and privacy-safe diagnostics.

### `narrivy-desktop`
Presentation/client. It may create an overlay window, but the overlay is never the owner of capture or session state. Restarting the desktop client must not destroy a recoverable daemon session.

### `narrivy-cli`
Diagnostic/control client using the same typed application protocol. It must not bypass privacy or domain policy.

### optional workers
Use separate workers later only when native runtime isolation measurably improves reliability/security. Do not proliferate processes by default.

## Implemented engine bindings

`engines/` holds four crates (checked at rc13):

- `asr-mock` — deterministic test engine. `narrivyd` always links it, and
  `EngineConfig::new()` with no environment selects it (with a sine fixture
  and a null sink). That is a test and development default only: the
  packaged unit and `scripts/dev-daemon.sh` set `NARRIVY_RECOVERY_MODE`,
  `NARRIVY_ASR_ENGINE=transcribe-cpp` and the resident runtime, so an
  installed Narrivy never uses the mock engine.
- `transcribe-cpp` — the GGUF ASR binding the installed daemon uses. It is
  optional, behind the `asr-transcribe-cpp` / `asr-transcribe-resident`
  features (`services/narrivyd/Cargo.toml`).
- `openvino` — the OpenVINO add-on engine (D25, ADR-0078). The daemon never
  loads OpenVINO; it starts `narrivy-openvino-worker`, which loads the
  runtime pack the user installs from the Store. Device claims still need
  qualification.
- `grammar-harper` — the Harper grammar checker used by the text stages.

## Portable crate boundary

Portable/domain crates cannot directly depend on a desktop UI framework (GTK/WebKit, Tauri), PipeWire/WirePlumber, Wayland/X11, D-Bus desktop implementations, Win32/Cocoa, OpenVINO/ONNX/CUDA, platform automation utilities, or model/runtime SDKs. CI verifies both source references and dependency metadata.

## Core execution path

1. trigger capability emits an activation event;
2. session engine creates a transaction and persists recoverable metadata;
3. capture adapter streams typed audio chunks;
4. ASR engine emits partial/final events for that transaction;
5. selected first-class Pipeline resolves/executes compatible audio→ASR→deterministic/optional generative stages;
6. final text is journaled before risky delivery;
7. delivery router attempts bounded compatible backends;
8. commit is recorded or text remains in Inbox/recovery state;
9. optional history is written according to retention/privacy policy;
10. session resources are cancelled/released and audio state restored.

## Event correctness

Every externally asynchronous result that can outlive a request carries session/transaction identity and ordering metadata. Clients/core ignore stale results from cancelled/replaced sessions. Cancellation is an explicit protocol/domain event, not “drop the UI spinner”.

## Capability-driven design

Capabilities describe granular behaviors and health, not merely OS/device names. UI/pipeline asks for capabilities and gets an availability/degradation reason. Platform identity can be diagnostic metadata but is not a general feature flag.

## Stability boundary

Phase-0 traits are scaffolding. P1–P5 may redesign them. Once contracts are marked stable, changes require compatibility/migration treatment described in `api-stability.md`.

## Modular composition

Activation, capture, audio preprocessing/noise suppression, VAD, ASR, compute runtime, processors, context and delivery are replaceable internal modules. Core depends on contracts and typed capability/health records, never concrete module implementations. Linux 1.0 may compile trusted modules in-process; future external native modules should prefer versioned sidecars rather than a public Rust dylib ABI. See `module-system.md`, `pipeline-profiles.md`, and `resource-registry.md`.
