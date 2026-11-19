# narrivy-ipc-windows

Authenticated per-user named-pipe transport for the Narrivy IPC protocol on
Windows: the counterpart of `narrivy-ipc-unix`.

- **Role:** Adapter crate in the `ipc-*` infrastructure family. It owns pipe
  names, security descriptors, peer identity, connection workers and their
  shutdown. `narrivy-ipc` still owns the envelope, framing and typed errors.
  It owns no product policy.
- **Crate docs:** `cargo doc -p narrivy-ipc-windows --no-deps --open --target x86_64-pc-windows-gnu`
- **Tests:** `cargo test -p narrivy-ipc-windows --locked` (on Linux this
  runs only the portable SID and pipe-name tests; on Windows it also runs
  the transport and end-to-end tests).
- **Cross-check from Linux:** `cargo clippy --locked -p narrivy-ipc-windows --target x86_64-pc-windows-gnu --all-targets -- -D warnings`

## Security model (ADR-0011 on Windows)

| Unix transport | This transport |
| --- | --- |
| socket path under `$XDG_RUNTIME_DIR` | `\\.\pipe\narrivy-<user SID>-narrivyd` |
| `0700` directory, `0600` socket | protected DACL: owner SID only (`O:<sid>D:P(A;;GA;;;<sid>)`), no SYSTEM, no inheritance |
| `flock` on `narrivyd.lock` | `FILE_FLAG_FIRST_PIPE_INSTANCE`: bind fails if any instance of the name exists, so a second daemon or a squatter is refused |
| stale-socket cleanup | none needed: pipe names vanish with their last handle |
| `SO_PEERCRED` uid | `GetNamedPipeClientProcessId` → process token user SID, checked before any frame is read |
| client checks the daemon's uid | `GetNamedPipeServerProcessId` → token SID, checked before the hello is sent |
| local only by construction | `PIPE_REJECT_REMOTE_CLIENTS`; only `\\.\pipe\` paths are accepted |
| — | clients open with `SECURITY_IDENTIFICATION`: the server can identify but never impersonate them |

Peer lookups fail closed: a process that cannot be opened or queried (for
example an elevated client seen from a non-elevated daemon) is rejected with
`PermissionDenied`.

All I/O is overlapped with bounded waits. Timeouts and shutdown cancel the
exact operation with `CancelIoEx` and wait for it to drain, so the kernel never
holds a pointer into freed memory. All `unsafe` lives in `src/sys.rs`, each
block with a `SAFETY:` comment.

## Wiring it in (not done yet)

The public API matches `narrivy-ipc-unix`, so wiring is a `cfg` switch. The
differences callers see:

| `narrivy-ipc-unix` | `narrivy-ipc-windows` |
| --- | --- |
| `UnixIpcServer` | `WindowsIpcServer` (also exported as `IpcServer`) |
| `PeerCredentials { uid, gid, pid }` | `PeerCredentials { sid: UserSid, pid }` (`Copy`, like Unix) |
| `PeerCredentials::validate(u32)` / `is_same_user(u32)` | `validate(&UserSid)` / `is_same_user(&UserSid)` |
| `current_uid() -> u32` | `current_user_sid() -> Result<UserSid, IpcError>` |
| `UnixIpcServer::expected_uid()` | `WindowsIpcServer::expected_sid()` |
| `default_socket_path()` | `default_socket_path()` (a `\\.\pipe\...` path) |
| `LOCK_FILE_NAME`, `EndpointPermissions`, `SocketPathValidation` | not applicable |

`IpcClient`, `SequencedEvent`, `IpcHandler`, `bind`,
`bind_with_shutdown_budget`, `serve`, `accept_once`, `publish_event`,
`request_shutdown`, `shutdown_and_join`, `shutdown_budget`,
`active_connection_count` and `socket_path` have the same signatures and
behaviour.

Steps:

1. In `services/narrivyd`, `apps/narrivy-cli`, `apps/narrivy-browser-host` and
   `apps/narrivy-desktop/src-tauri` `Cargo.toml`, move
   `narrivy-ipc-unix.workspace = true` under `[target.'cfg(unix)'.dependencies]`
   and add `narrivy-ipc-windows.workspace = true` under
   `[target.'cfg(windows)'.dependencies]`.
2. Add one transport alias module per crate, for example:

   ```rust
   #[cfg(unix)]
   pub(crate) use narrivy_ipc_unix as ipc_transport;
   #[cfg(windows)]
   pub(crate) use narrivy_ipc_windows as ipc_transport;
   ```

   and import `IpcClient`, `IpcHandler`, `PeerCredentials`,
   `default_socket_path` from `ipc_transport` instead of `narrivy_ipc_unix`.
   For the server type use `narrivy_ipc_unix::UnixIpcServer as IpcServer` on
   Unix and `narrivy_ipc_windows::IpcServer` on Windows.
3. Make the daemon's principal check transport-neutral. Today
   `services/narrivyd/src/daemon/ipc_policy.rs` stores `expected_uid: u32`
   and compares `credentials.uid`; `session.rs` builds it from `current_uid()`.
   Store the server's expected identity instead (`expected_uid()` on Unix,
   `expected_sid()` on Windows) and call
   `credentials.is_same_user(&expected)`; the policy tests that build
   `PeerCredentials { uid, .. }` need a `cfg(windows)` twin with a `sid`.
4. `NARRIVY_SOCKET` keeps working as the override: on Windows it must be a
   `\\.\pipe\...` path, otherwise `bind`/`connect` return `BadRequest`.
5. Unix-only startup steps around the socket (runtime-directory handling in
   `main.rs`) stay `cfg(unix)`; the pipe needs no directory.

## Not covered yet

- On Linux, CI proves only compile/clippy for `x86_64-pc-windows-gnu` and
  the portable tests; the transport tests need a Windows host.
- No test exercises a real second Windows account (cross-user rejection is
  covered by the DACL, first-instance and SID-mismatch unit paths).
