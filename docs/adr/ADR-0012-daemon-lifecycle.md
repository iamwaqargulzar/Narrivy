# ADR-0012 — Daemon Lifecycle

**Status:** Accepted (P1, 2026-08-21)  
**Context:** the legacy P1 planning amendment requires an explicit daemon/client lifecycle decision before broad implementation. The daemon owns session state, capture, model residency, pipeline, and recovery journal; clients are Tauri desktop, CLI, and overlay.

## Decision

> **2026-08-22 annotation:** AppImage was dropped from Linux 1.0 scope by owner
> decision (single-artifact `.deb` release). The sidecar fallback remains valid
> for systemd-less environments; historical AppImage references below retain
> their decision context.
>
> **2026-09-22 data-retention amendment:** package maintainer scripts never
> traverse user home directories. Both `apt remove` and `apt purge` preserve
> per-user XDG config/data/state/cache. User-data deletion is a separate action
> run as that user, with backup-first behavior required before the product may
> automate it.

- **Systemd user service is the primary lifecycle** on Linux (Pop!_OS/COSMIC and Ubuntu/GNOME Wayland). The `.deb` and AppImage both install/ship `narrivy-daemon.service` as `WantedBy=default.target` with `Restart=on-failure` and `RestartSec=2`. CLI `narrivy-cli daemon --start/--stop/--status` proxies to `systemctl --user`.
- **Sidecar fallback** for AppImage/portable or when systemd is unavailable (e.g., some containers): the desktop client spawns `narrivyd` as a tracked sidecar with `--sidecar` flag; the sidecar writes its PID to `$XDG_RUNTIME_DIR/narrivy/narrivyd.pid` and holds a lock file `$XDG_RUNTIME_DIR/narrivy/narrivyd.lock`.
- **Single-instance ownership:** Both transports use a Unix socket at `$XDG_RUNTIME_DIR/narrivy/narrivyd.sock` with `0700` dir, `0600` socket, and `flock` on the lock file. Second instance detects the live socket via connect+ping and exits with `E_ALREADY_RUNNING` and prints the owner PID.
- **Socket lifecycle:** Daemon creates `0700` directory, binds socket, `fsync`s directory, and registers cleanup on `SIGTERM`/`SIGINT`/`SIGQUIT`. Stale socket cleanup validates that the path is exactly `$XDG_RUNTIME_DIR/narrivy/narrivyd.sock`, is a socket, and `lstat` shows owner UID matches effective UID; it never follows symlinks and never removes another user's path.
- **Crash restart:** systemd handles restart; sidecar mode uses a parent watchdog that restarts once and then reports degraded state if crash loops. All crashes emit a sanitized diagnostic event (no transcript/audio) and preserve the recovery journal.
- **Graceful shutdown:** `SIGTERM` initiates `ShutdownRequested` → finish in-flight delivery → flush recovery journal → release audio policy → close socket → exit 0 within 2 s. `Shutdown` IPC command requires same-user peer credential and is rate-limited.
- **Version skew:** Handshake exchanges `protocol_major/minor`, `build_identity`, and `git_commit`; minor skew is tolerated if capability negotiation succeeds, major skew rejects with `INCOMPATIBLE_PROTOCOL` and instructs update. The desktop shows "Daemon out of date — restart required" and `doctor` reports skew.
- **Uninstall & data retention:** `apt remove` and `apt purge` stop/disable the
  user service and remove package-owned files, but retain
  `$XDG_CONFIG_HOME/narrivy`, `$XDG_DATA_HOME/narrivy`,
  `$XDG_STATE_HOME/narrivy`, and `$XDG_CACHE_HOME/narrivy`. Root maintainer
  scripts do not enumerate home directories. A user-session purge command may
  remove those paths only after explicit confirmation and a successful backup
  to `$XDG_DATA_HOME/narrivy/backup/pre-purge-<timestamp>.tar.gz`; until that
  flow is implemented and qualified, release notes provide manual removal
  instructions. `THIRD_PARTY_NOTICES` and SBOM remain in the package.

## Consequences

- Implementation must support both lifecycles behind a single IPC transport abstraction (already in `narrivy-ipc`).
- P6/P10/P11 qualification must test both systemd and sidecar restart/recovery, stale-socket, and skew cases.
- No broad privilege is required; service runs as the user.

## Alternatives considered

- Sidecar only (rejected — no restart after the desktop exits, no background hotkey when desktop closed).
- System-wide service (rejected — broader privilege, violates same-user IPC boundary ADR-0011).
- DBus activation only (rejected — not universally available, complicates AppImage).

## Verification

Recorded as of 2026-09-28. The named integration file
`tests/daemon_lifecycle.rs` was never created; the behaviours are covered at
these real locations:

- singleton lock: `adapters/ipc-unix/src/server.rs:1467` `ipc_server_second_instance_lock_rejected` and `adapters/platform-linux/src/instance.rs:321` `second_lock_attempt_fails_while_first_held`
- stale-socket cleanup: `adapters/ipc-unix/src/server.rs:1487` `ipc_server_stale_socket_rebound_after_drop`, `:1521` `ipc_server_refuses_non_stale_socket_paths`, `:1604` `ipc_runtime_directory_symlink_is_rejected_without_touching_target`
- SIGTERM graceful shutdown under one shared budget: `services/narrivyd/tests/daemon_shutdown_signals.rs` and `services/narrivyd/tests/daemon_shutdown_budget.rs` (ADR-0069)
- version skew handshake: `crates/narrivy-ipc/src/protocol.rs` `ipc_handshake_accepts_matching_versions`, `ipc_handshake_minor_skew_negotiates_explicitly`, `ipc_handshake_major_skew_rejected_incompatible`

Not covered: crash-kill (SIGKILL) restart with a preserved recovery journal has
no test at the paths named above. `tools/narrivy-qualify` still exists but
`tools/narrivy-qualify/p3_runner.py` documents a Tauri shell launch, so the
shell half of this verification line is stale.
