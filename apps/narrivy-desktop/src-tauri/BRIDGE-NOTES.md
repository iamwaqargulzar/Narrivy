# src-tauri bridge notes (P3 shell)

## Env hooks (all optional; unset means no behavior change)

| Variable | Effect |
| --- | --- |
| `NARRIVY_SOCKET` | Daemon socket path override (default: `$XDG_RUNTIME_DIR/narrivy/narrivyd.sock` via `narrivy_ipc::default_socket_path`). |
| `NARRIVY_READY_FILE` | Setup hook writes a JSON readiness marker: resolved window config flags, live window introspection, bridge status, daemon capability checks, theme state, overlay lesson echoes. Test lanes only. |
| `NARRIVY_ACCESSIBILITY_ZOOM` | With `NARRIVY_READY_FILE`, value `2` applies WebKit's 200% zoom for native accessibility qualification. Ignored in ordinary launches. |
| `NARRIVY_THEME_FILE` | Theme persistence path override (default `<app config dir>/theme.json`). Also read by narrivyd's `theme-store` capability check. |
| `NARRIVY_QUALIFY_THEME_APPLY` | One-shot `system|light|dark` apply at launch through the same `ThemeStore` the `theme_set` command uses. |
| `NARRIVY_QUALIFY_LOG` | JSONL observation file appended at each launch (`persisted_theme`, bridge status ok flag, daemon `theme-store` capability check result). |

## Protocol surface

All CONTRACT.md bridge commands map 1:1 onto first-class narrivy-ipc request
kinds. Protocol minor 4 distinguishes optional `GetHistory` /
`GetHistoryEntry{txid}` from `GetRecoveryEntry{txid}`. The former fail closed
while optional History is disabled; the latter retrieves only the exact
short-lived transaction carried by a terminal session event.

Privacy: no recovery listing is exposed. The desktop never chooses “newest”
recovery data and never treats temporary recovery records as retained History.

## Shell migration seam

`bridge.rs`, `session_events.rs`, and `theme.rs` contain no Tauri API use.
`tauri_commands.rs` is the complete macro-facing command wrapper, while
`lib.rs` owns only current-shell composition, windows, qualification hooks, and
the event sink. The event-forwarding worker retains an explicit shutdown
signal and join handle; app exit requests shutdown and `run()` joins it before
returning. Source-guard tests prevent framework coupling from returning to the
reusable modules.

ADR-0075 makes this Tauri 2 shell the desktop host on every OS and retires
the GTK4/WebKitGTK 6 shell of ADR-0064. The web view is locked down in
`tauri.conf.json` (deny-by-default CSP, frozen prototype, no global Tauri
object) and `capabilities/default.json` (event listen/unlisten only), under
the `tests/governance/test_tauri_lockdown.py` and `test_desktop_tauri.py`
governance tests. Every command runs off the UI thread (`#[tauri::command(async)]`).

## Degraded state

Every command returns a typed `BridgeError { code, message }` when the
daemon is absent or errors (`daemon_unavailable`, ...); the shell never
panics from a missing daemon and the frontend renders the degraded state
from the error class.
