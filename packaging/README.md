# Packaging Plan

See `docs/maintainers/operations/reproducible-builder.md` for builder baselines, SBOM, and release manifest, and `docs/maintainers/operations/update-policy.md` for the 1.0 update decision (no auto-updater).

- `.deb` — first-class, `packaging/deb/` control files, `daemon/narrivy-daemon.service` as user service, XDG paths, desktop file, icons.
- `AppImage` — follow-on after 1.0 only (owner decision 2026-08-22: `.deb`-single-artifact release; recipe retained for future re-promotion via ADR).
- Future RPM/AUR/Nix — post-1.0, unless ADRed as low-risk.

All packages are verified by `narrivy-qualify` scenarios `p10-deb-install`, `p10-upgrade-uninstall` on clean VMs plus native spot checks.

Recovery now has an exact-inspected retained candidate builder at
`scripts/build_deb_installable.py`. Default output remains an installable
unqualified candidate. `--clean-source --source-root` creates an exact
unqualified release candidate from a proven clean tree; native qualification
and external schema-v3 provenance remain required before those P10 scenarios
and P11 acceptance evidence can pass.
