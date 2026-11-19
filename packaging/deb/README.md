# Narrivy .deb Packaging — Single Linux 1.0 Artifact

Owner decision 2026-08-22: `.deb` is the **single** Linux 1.0 installation artifact (no AppImage for 1.0).
All paths below follow `packaging/deb/` as canonical per P10 spec; `packaging/linux/deb/` is kept in sync for compatibility.

> Recovery quarantine: `scripts/build_deb.sh`, the P10 qualification runner,
> and the P10 evidence refresh entry point intentionally exit 2 without
> changing files. The historical builder remains unsafe. The replacement
> `scripts/build_deb_installable.py` creates a retained, reproducible,
> exact-inspected candidate. Default mode labels it an installable unqualified
> candidate. `--clean-source --source-root` creates an exact unqualified release
> candidate from a proven clean tree; native qualification and external
> schema-v3 provenance remain required before clean install, upgrade, removal,
> and application acceptance pass. The disposable negative composer remains
> available as `scripts/build_deb_candidate.py`. See ADR-0062.

## Contents

- `control` — Debian control file for GTK4/WebKitGTK 6 and the pinned Rust 1.97.1 toolchain
- `narrivy-daemon.service` — systemd **user** service (`WantedBy=default.target`, not `multi-user.target`)
- `narrivy.desktop` — desktop entry (`Exec=narrivy-desktop`, `Icon=narrivy`)
- `postinst` / `prerm` / `postrm` — maintainer scripts (upgrade preserves XDG user data, purge explicit)
- `changelog` — Debian changelog (gzipped reproducibility via `gzip -n`)

Desktop entry and icons also live in `packaging/linux/desktop/` and `packaging/linux/icons/` for Tauri/bundle use.

## Build

`scripts/build_deb_installable.py` accepts explicit release binaries, the
resident worker, the registry-pinned offline model, package metadata, source
identity, and an unused absolute output path. Optional `--clean-source` with
`--source-root` selects clean-source mode. It rejects mutable or unsafe inputs,
creates a root-owned deterministic archive, inspects exact data/control
membership and digests, then publishes without overwriting an existing file.

Default output is an **installable unqualified candidate**. Clean mode creates
an exact **unqualified release candidate**; native qualification and external
schema-v3 provenance remain required. Neither output is a release artifact.
Do not use or distribute anything under `artifacts/candidates/` as a release.
B-PACKAGE-015 remains open until clean-machine install, upgrade, removal,
runtime, provenance, and acceptance evidence pass.

## Systemd user service

The daemon runs **per-user** via `systemd --user` (see ADR-0012). The package
never installs a privileged system daemon. On its first status request, the
desktop's off-UI command worker performs a bounded user-manager reload/start
when the canonical daemon is absent. Explicit qualification sockets never
trigger service management. Package maintainer scripts do not attempt to
control a user manager from the package manager's root context. Ordinary
removal preserves user data; `postrm purge` documents explicit removal.

## Global activation (user-managed shortcut)

Narrivy does **not** create or modify desktop/compositor shortcuts. On
compositions where the desktop exposes no XDG GlobalShortcuts portal listener
(for example the current COSMIC host, whose portal exposes RemoteDesktop v2 but
no `org.freedesktop.portal.GlobalShortcuts`), the installed client provides one
atomic activation command the user may bind in their own shortcut settings:

```text
/usr/bin/narrivy-cli toggle
```

`narrivy-cli toggle` sends exactly one atomic `ToggleSession` request to the
daemon: with no active session the daemon runs the same strict admission/start
path as `start`, and with a recording session it requests a graceful stop. The
command prints only typed, content-free identity (outcome, session/transaction,
mode) and never transcript or delivery-target content. Optional explicit mode:
`/usr/bin/narrivy-cli toggle --pipeline raw|clean`.

The daemon reports three separate, content-free facts over status:
`activation.atomic-command`, `activation.portal-global-shortcuts`, and
`activation.portal-listener`. The last stays unavailable until a real
daemon-owned portal listener is composed; the packaged command is a
user-managed fallback, so the desktop UI may show the exact command as a
suggestion but must never claim the shortcut is configured or working.

## SBOM

Historical SBOM/build-info files are quarantined and are not release
provenance. A replacement build must generate and bind a complete inventory to
the exact source, builder, binaries, runtime manifest, and resulting archive.

## Update policy

No in-app auto-updater for 1.0 (see `docs/maintainers/operations/update-policy.md`). Updates via `apt upgrade` from the apt repo or GitHub Releases `.deb`.
