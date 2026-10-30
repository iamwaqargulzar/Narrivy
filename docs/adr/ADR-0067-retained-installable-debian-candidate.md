# ADR-0067: Retained installable Debian candidate

- Status: accepted for recovery; release qualification pending
- Status note (2026-10-30): the package now carries the Tauri 2 desktop
  app, not the GTK4 desktop named below (ADR-0075).
- Date: 2026-09-11
- Phase: P10/P11 recovery

## Context

ADR-0062 quarantined the historical Debian builder and permitted only an
exact-inspected archive that deliberately refused installation. That contained
the immediate risk but could not exercise the real installed application. The
maintained desktop now uses GTK4/WebKitGTK 6, the resident transcribe worker has
a fixed installed manifest, and the selected bundled model has a registry-pinned
size and SHA-256. A retained candidate is required before clean-machine
qualification can resume.

The prior package metadata still named the retired GTK3/Tauri runtime. It also
claimed that first launch prompted to enable the user service, while no such
flow existed, and attempted `systemctl --user` from root package-maintainer
scripts.

## Decision

Add `scripts/build_deb_installable.py` as a separate, fail-closed builder. It:

- accepts every binary, model, metadata, source identity, epoch, version, and
  output path explicitly;
- reuses the descriptor-anchored runtime stager and bounded ELF/input readers;
- requires the exact registered `whisper-tiny.en-Q8_0` size and SHA-256;
- packages the GTK4 desktop, daemon, CLI, resident worker, installed-runtime
  manifest, offline model, user service, desktop entry, icons, and notices;
- normalizes archive ownership, modes, and timestamps, with data-root mode
  `0755` so installation cannot narrow `/`;
- inspects exact control/data membership, modes, identities, sizes, and digests;
- publishes with create-exclusive semantics and never overwrites an existing
  destination; and
- labels build metadata and command output `installable-unqualified-candidate`.

The GTK command worker performs a bounded `systemctl --user daemon-reload` and
`start narrivy-daemon.service` only after the canonical status endpoint is
unavailable. This work stays off the GTK thread. An explicit `NARRIVY_SOCKET`
disables service management so tests and qualification harnesses retain process
ownership. Maintainer scripts do not control a user manager from root context.

## Consequences

An installable candidate can now be retained and tested without reviving the
quarantined builder. The package grows by one compressed offline model so first
use does not require network access; installation into the per-user cache still
requires the user's explicit Models-screen action.

This decision creates no release artifact and advances no requirement. Clean
install, real installed-mode model activation, dictation, upgrade, removal,
desktop lifecycle, SBOM/provenance, accessibility, and P11 evidence remain
mandatory. `artifacts/candidates/` must not be published as a release.
