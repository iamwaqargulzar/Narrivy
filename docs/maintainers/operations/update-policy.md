# Update Policy — Linux 1.0 Decision

**Decision (2026-08-21, legacy planning Part J5):** **No in-app auto-updater for Linux 1.0.**

- `.deb` updates via `apt update && apt upgrade` (or Discover/GNOME Software) from the apt repo or GitHub Releases `.deb`.

## Rationale

Part J5 requires that for 1.0 the project explicitly choose either (a) no in-app updater with documented package/manual update behavior, or (b) a signed update metadata/trust/rollback design **before release**. Shipping an unsigned auto-updater late in P10 is prohibited. No signing credential or update-transport trust design is qualified for 1.0 (see `agent/BLOCKERS.md` B-SIGN-005 removed — signing optional). Therefore option (a) is chosen.

## What this means

- The desktop `About / Updates` screen shows:
  - installed version / `git_commit` / build ID,
  - available version from GitHub Releases **only if** the user has allowed network for update checks in `Settings → Privacy` (default: no automatic check; manual “Check for updates” button),
  - manual instructions (“Download .deb from releases, `sudo apt install ./narrivy_<ver>.deb`”).
- If in the future a signed updater is desired, it must have:
  - signed update metadata (minisign/ed25519 or gpg) with trust anchor in the package,
  - rollback on failed signature or failed post-update health check,
  - ADR and qualification (`p10-update-signed`) before it ships.

## Non-goals for 1.0

- No background download, no unattended install, no privilege escalation without explicit user action.
