# TEST-002 Evidence — Architecture-boundary Tests Inspect Source and Dependency Metadata

## Verification: VER-TEST-002 (ci/architecture)
**Requirement:** Architecture-boundary tests inspect source and dependency metadata so forbidden framework/OS/runtime coupling cannot silently enter portable crates.

## Implementation
- Source boundary check: `scripts/verify-boundaries.py`
  - Scans portable crates: `crates/narrivy-types`, `narrivy-platform-api`, `narrivy-asr-api`, `narrivy-policy`, `narrivy-pipeline`, `narrivy-ipc`, `narrivy-diagnostics`, `narrivy-core`
  - Forbids tokens: tauri, pipewire, wayland, x11, openvino, onnxruntime, windows-sys, cocoa, objc, evdev, libei
  - Runs in `pack` job and as gate command `verify_dependency_boundaries`
- Dependency graph check: `scripts/verify_dependency_boundaries.py`
  - Uses `cargo metadata --locked` to walk transitive deps of 8 protected packages
  - Denies forbidden tokens in dependency names: tauri, pipewire, wayland, x11, openvino, onnx, cuda, cudarc, coreaudio, wasapi, windows-sys, cocoa, objc, libei, evdev, xkbcommon
  - Runs in CI `rust-quality` job before check/test/clippy

## Local Verification
```
python3 scripts/verify-boundaries.py → Source boundaries OK.
python3 scripts/verify_dependency_boundaries.py → Dependency boundaries OK for 8 portable/core packages.
```

## Evidence Files
- `scripts/verify-boundaries.py`
- `scripts/verify_dependency_boundaries.py`
- `docs/02-architecture/dependency-rules.md`
