# Dependency Rules

Allowed high-level dependency flow:

```text
apps/services
   ↓
application/core
   ↓
contracts/domain
   ↑
adapters/engines
```

Concrete adapters are assembled at the application composition root.

## Forbidden in portable crates
- desktop UI framework types (GTK/WebKit, Tauri);
- platform FFI;
- PipeWire/Wayland/X11/D-Bus/IBus/Fcitx types;
- Win32/UIAutomation/WASAPI types;
- Cocoa/Accessibility/CoreAudio types;
- OpenVINO/ONNX/CUDA/DirectML/CoreML runtime types;
- frontend JSON shape assumptions.

## Architecture checks

`scripts/verify-boundaries.py` scans production Rust sources in portable crates
for concrete operating-system, native-runtime, UI-framework, and FFI coupling.
It ignores comments and literals, but fails closed for missing or unreadable
protected sources.

`scripts/verify_dependency_boundaries.py` checks direct normal and build
dependencies declared by protected crates, including renamed and
target-conditional declarations. Development-only dependencies and the
target-specific transitive internals of reviewed portable abstractions are not
production architecture edges.

Both validators have isolated positive and negative fixtures and run directly
in CI. The Linux, Windows, and macOS native-runner compile matrix remains a
separate portability gate; a validator pass is not a cross-platform runtime or
compile claim.
