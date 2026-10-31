# P10 — Linux Packaging & Release Engineering

> **Scope note (2026-09-28, owner):** the first release is qualified on Pop!_OS
> COSMIC only. Mentions below of Ubuntu/GNOME as a release lane, or of AppImage,
> are superseded; see `docs/roadmap/README.md`. This phase spec is kept as the
> historical definition of the phase.

## Outcome
Produce installable, upgradeable, removable release candidates and reproducible supply-chain metadata with defined immutable builder baselines.

## Primary requirements
`OPS-002`–`OPS-005` with `SEC-004` revalidated.

## Mandatory artifacts
1. `.deb` first-class for Ubuntu/Pop!_OS — the single Linux 1.0 installation
   artifact (owner decision 2026-08-22: no AppImage/portable package for 1.0).
2. AppImage/RPM/AUR/Nix follow after 1.0 unless promoted by an ADR.
3. Flatpak only after sandbox/portal implications are proven.

## Package behavior
Desktop metadata/icons; daemon/autostart ownership (per ADR-0012); XDG config/data/cache; dependency declarations; upgrade migrations/interrupted-upgrade recovery; uninstall vs explicit purge; clean-VM install; upgrade from prior candidate; launch and real dictation from package — not source tree.

## Supply chain & reproducibility (Part J)
- Rust toolchain pinned (1.97.1), `Cargo.lock` required, tools pinned;
- CI/build environments have defined immutable baseline/container/image identity with digest recorded; floating labels may be used only for broad compatibility signal;
- Release manifest includes the .deb path/hash, source commit, build ID, builder identity, SBOM, third-party inventory/notices, provenance/attestation, signing state, qualification evidence set, release notes/support matrix derived from P11 evidence;
- Checksums verified; immutable CI actions; dependency/license inventory via `cargo-deny`.

## Update policy
For 1.0 either **no in-app updater** and document package/manual update behavior, or implement signed update metadata/trust/rollback design before release. Do not bolt on unsigned auto-updater late — see `docs/maintainers/operations/update-policy.md`.

## Exit evidence
Clean reference systems install `.deb` and AppImage, run real dictation, upgrade/uninstall per data policy. Schema-valid evidence. `python3 scripts/phase_gate.py --phase P10` exits 0.
