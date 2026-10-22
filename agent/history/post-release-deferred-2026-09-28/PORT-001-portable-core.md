# PORT-001 Evidence — Portable Core Crates Compile on Linux, Windows and macOS CI Targets

## Verification: VER-PORT-001 (ci/architecture)
**Requirement:** Portable core crates compile on Linux, Windows and macOS CI targets from Phase 0.

## CI Configuration
- Toolchain pinned in `rust-toolchain.toml`: 1.97.1 (profile minimal, components rustfmt, clippy)
- Portable MSRV in `.github/workflows/ci.yml`: 1.85.0 (workspace.rust-version)
- CI jobs:
  - `portable-native` matrix: ubuntu-latest, windows-latest, macos-latest — runs `cargo check --locked -p narrivy-types -p narrivy-platform-api -p narrivy-asr-api -p narrivy-policy -p narrivy-pipeline -p narrivy-ipc -p narrivy-diagnostics -p narrivy-core`
  - `portable-msrv` job: ubuntu-latest with toolchain 1.85.0 — same check
  - `rust-quality` job: fmt/check/test/clippy on ubuntu-latest with pinned toolchain

## Local Verification (2026-08-21, Host: Linux COSMIC Wayland, 1.97.1)
```
cargo check --workspace --locked  → OK
cargo +1.85.0 check --locked -p narrivy-types -p narrivy-platform-api -p narrivy-asr-api -p narrivy-policy -p narrivy-pipeline -p narrivy-ipc -p narrivy-diagnostics -p narrivy-core → OK (see logs)
cargo check --locked -p narrivy-types ... (portable) → OK
```

## Evidence Files
- `.github/workflows/ci.yml` — defines portable-native and portable-msrv jobs
- `rust-toolchain.toml` — pinned 1.97.1
- `Cargo.toml` — workspace.rust-version 1.85
- This file

## Gate Status
- `python3 scripts/verify_dependency_boundaries.py` — OK (8 portable/core packages)
- `python3 scripts/verify-boundaries.py` — OK (source boundaries)
