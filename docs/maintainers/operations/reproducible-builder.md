# Reproducible Builder & Package Plan — Linux 1.0

> Authority: legacy planning Part J. Covers dependency/tool pinning, builder baselines, release manifest, and update trust. AppImage baseline demoted to follow-on (owner decision 2026-08-22: `.deb`-only 1.0 release).

## J1 — Dependency / tool pinning

- **Rust toolchain pinned:** `rust-toolchain.toml` `channel = "1.97.1"`; `Cargo.lock` is committed and **required** for product and release builds (`cargo --locked`).
- **cargo-deny pinned:** `0.20.2` via `cargo install cargo-deny --version 0.20.2 --locked` (CI and `scripts/bootstrap-linux-dev.sh`); `deny.toml` `allow-wildcard-paths = true` only for private path deps, external wildcards remain denied.
- **CI actions pinned:** all `uses: actions/...` pinned by immutable SHA (e.g., `actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1`), verified by `scripts/check_action_pins.py`.
- **System deps recorded:** `agent/RESOLVED_VERSIONS.json` records `rust_project_toolchain`, `portable_msrv`, `cargo_deny`, `node`, `tauri_major`, `openvino_strategy`, `vulkan_acceleration`, and builder OS/kernel at qualification time.
- **No curl-pipe-shell installers:** tools are installed via `cargo install --locked`, official packages (`apt`, `rustup`), or verified releases with checksum/signature where practical.
- **Volatile facts:** model/runtime/driver support rechecked against upstream (OpenVINO docs, ONNX Runtime EPs, Vulkan spec) before first integration and before release; source URL, retrieval date, exact version/revision recorded in `agent/RESEARCH_LOG.md`.

## J2 — CI / build environments

- Floating hosted labels (`ubuntu-latest`) may be used for broad compatibility signal only.
- **Release-producing builders have a defined immutable baseline** where technically possible:
  - `builder_identity.os`: e.g., `Ubuntu 22.04.5 LTS`
  - `builder_identity.image`: e.g., `ghcr.io/narrivy/builder:2026-08-21-1`
  - `builder_identity.image_digest`: `sha256:...`
  - `toolchain`: e.g., `rustc 1.97.1 + cargo 1.97.1 + Tauri 2.12.5 (GTK3 + WebKitGTK 4.1, gtk-layer-shell 0.8; ADR-0075)`. Tauri 2 on WebKitGTK 4.1 is the shipped desktop host (ADR-0075); the GTK4/WebKitGTK 6 shell of ADR-0064 was removed.
  - Recorded in `artifacts/release-evidence/linux-1.0/release-artifacts.json: builder_identity` and in `agent/RESOLVED_VERSIONS.json`.
- For CI hosted builders where a custom image is not yet available, record at least `os`, `image` (runner image version), `toolchain`, and `cargo_lock_hash` (sha256 of `Cargo.lock`).

## J3 — AppImage baseline (FOLLOW-ON — dropped from 1.0 by owner decision 2026-08-22; retained for future re-promotion)

- **Concrete baseline:** `Ubuntu 22.04` (glibc 2.35) builder — compatible with both Pop!_OS 24.04 and Ubuntu 24.04.2 targets. Do not say merely “sufficiently compatible”.
- The produced AppImage is **tested on clean target systems** (both lanes) via `narrivy-qualify` scenario `p10-appimage-baseline` (install/launch/dictate within AppImage).
- Builder baseline is recorded in `release-artifacts.json`; a future move to a newer baseline requires an ADR and re-qualification of the target matrix.

## J4 — Release manifest

`spec/schemas/release-artifacts.schema.json` (v3) requires:

- `.deb` paths/hashes/sizes plus sbom/provenance artifacts (single-package release per owner decision 2026-08-22)
- `version`, `build_identity` (commit-derived or `Cargo.toml` version), `git_commit`, `git_dirty`, `builder_identity`
- SBOM path (e.g., `artifacts/sbom/spdx.json`), inventory, `THIRD_PARTY_NOTICES.md`, provenance/attestation where available (e.g., SLSA, `provenance.json`), signing state
- Qualification evidence set (`qualification.json` bound to same `git_commit`)

`scripts/build_candidate_supply_chain.py` writes an unqualified candidate bundle
for one existing `.deb`: canonical SPDX-2.3, in-toto provenance whose subject is
that exact package digest, and schema-v3 `release-artifacts.json` using
repository-relative paths. Caller-supplied builder identity is recorded, not
proven. The bundle does not replace native qualification, signing,
immutable-builder proof, or release-gate evidence. `scripts/generate_sbom.sh` is
quarantined and must not be used for a current candidate.

See `scripts/release_gate.py` for verification (checksums, sizes, commit binding, SBOM/notices existence, provenance).

## J5 — Updates (1.0 policy decision)

For **Linux 1.0**:

- **No in-app auto-updater.** Package/manual update only (`apt upgrade` for the `.deb`). See `docs/maintainers/operations/update-policy.md`.
- Rationale: Part J5 requires either no updater or a signed update metadata/trust/rollback design before release. No update-signing infrastructure is qualified for 1.0, so shipping an unsigned auto-updater is prohibited.
- The desktop `About / Updates` screen shows installed version, available `.deb` version (if GH Releases check is policy-allowed via `Privacy`), and manual update instructions.

## How to cut a release

```bash
# 1. Check the pack and owner gates (release_gate.py has no --dry-run).
python3 scripts/validate_pack.py
python3 scripts/verify_owner_gates.py

# 2. Build the .deb from a clean commit. This also builds the unqualified
#    supply-chain bundle and writes artifacts/candidates/VERSION-build.json.
#    (There is no packaging/build-deb.sh; no AppImage in 1.0.)
scripts/build_release_candidate.sh 1.0.0~rc13

# 3. After native qualification, populate
#    artifacts/release-evidence/linux-1.0/ and run the release gate.
python3 scripts/release_gate.py --target linux-1.0
```

## Verification

- `scripts/validate_pack.py` checks builder identity fields when `release-artifacts.json` exists.
- `scripts/release_gate.py` verifies checksums, builder identity, SBOM/notices, and that `qualification.json` environments are native and bound to the same commit.
