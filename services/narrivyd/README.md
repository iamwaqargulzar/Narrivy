# narrivyd

The Narrivy per-user dictation daemon: composes capture, ASR, correction, delivery and recovery.

- **Role:** Service: composes the portable core with the host OS adapters (Linux, and Windows for PORT-007) and engines, and owns the daemon lifecycle.
- **Crate docs:** `cargo doc -p narrivyd --no-deps --open`
- **Tests:** `cargo test -p narrivyd --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.

## Per-OS composition

`src/daemon/` is shared. `src/daemon/host.rs` picks the adapters once per
target, and the daemon modules name them only through it:

| | Linux (`src/linux_main.rs`) | Windows (`src/windows_main.rs`) |
| --- | --- | --- |
| IPC | `ipc-unix` socket under `$XDG_RUNTIME_DIR` | `ipc-windows` pipe `\\.\pipe\narrivy-<SID>-narrivyd` |
| Same-user check | peer uid | peer token SID |
| Single instance | `flock` on `narrivyd.lock` | first pipe instance (bound before the journal is opened) |
| Journal | `journal-unix` | `journal-windows` |
| Helper processes | `process-linux` | `process-windows` (Job Objects) |
| State / Inbox | XDG state dir / `$XDG_RUNTIME_DIR/narrivy/inbox` | `%LOCALAPPDATA%\Narrivy\state` / `%LOCALAPPDATA%\Narrivy\run\inbox` |
| Started by | `systemctl --user` (`narrivy-daemon.service`) | `narrivy-cli start`/`toggle`: launches `narrivyd.exe` from the CLI's folder, detached, as the same user |
| Clean stop | SIGTERM / SIGINT | Ctrl+C / Ctrl+Break / console close; a detached daemon ends with the logon session (the journal covers the abrupt end) |

On Windows the daemon must run in the user's interactive session, never as
a Windows service: session 0 cannot reach the user's microphone prompt,
input, clipboard or global shortcuts.

Linux-only features (the RemoteDesktop and GlobalShortcuts portals, the
trusted `pw-record` and `wtype` helpers, PipeWire echo cancellation, desktop
shortcut files, the evdev hold key, the Secret Service, sysfs accelerator
counters) are composed on Windows through
`src/daemon/linux_features_unavailable.rs` and `src/daemon/portal_unavailable.rs`,
which keep the Linux types' shapes and answer every probe with the typed
"unavailable" outcome, so status and capability checks report them
truthfully.

The live microphone lane on Windows (`asr-transcribe-resident`, run with
`scripts/dev-daemon.ps1`) composes WASAPI capture (`NARRIVY_AUDIO_SOURCE=wasapi`)
and the Windows routed delivery: `SendInput` typing, or clipboard paste when
the user chose paste (D37), then exactly one Inbox write, as the Linux routed
lane does. The daemon registers the toggle shortcut itself with
`RegisterHotKey` (`NARRIVY_TOGGLE_SHORTCUT`, default `Ctrl+Alt+space`); a
press goes through the daemon's own pipe as `ToggleSession`. Crash reports
are turned off at start (`SEM_NOGPFAULTERRORBOX`; a machine-wide WER
LocalDumps policy is outside the process's control).

Not composed on Windows yet: installed-package (trust-anchored) engine
bindings (refused at startup; the engine runs from an explicit, digest-pinned
worker and model), microphone selection (the default capture device is
used), echo cancellation, the hold key, Credential Manager, and a
`activation.shortcuts` capability for the registered hotkey (status still
reports the manual route).
