# Packaging

| Folder | What it holds |
|---|---|
| `deb/` | Debian control files, maintainer scripts, the user service and desktop entry for the `.deb` |
| `linux/` | the Linux icons and a copy of the `.deb` files |
| `recovery/` | the service unit used by the package tests |
| `voice/` | the speech detector model the voice worker uses |
| `store/` | Store catalog additions |
| `licenses/` | licence texts shipped inside the packages |

- **Linux:** one `.deb` for Pop!_OS 24.04 (COSMIC, Wayland), built by
  `scripts/build_release_candidate.sh` from a clean commit. The daemon runs as
  a per-user systemd service.
- **Windows:** one per-machine `.msi` for 64-bit Windows 11, built by
  `scripts/build_windows_msi.ps1`.
- Neither package contains a speech model; first run downloads the one the
  user picks and checks it against its published SHA-256.
- Updates come from new packages: there is no in-app updater (see the
  [update policy](../docs/maintainers/operations/update-policy.md)).
- AppImage, RPM, AUR and Nix are possible later.

`scripts/qualify_candidate.py` and `scripts/qualify_candidate_upgrade.py` test
an exact `.deb` in a disposable container (install, upgrade, removal, purge).
