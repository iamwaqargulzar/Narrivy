# ADR-0064: Provisional GTK4/WebKitGTK 6 desktop runtime

Status: **Superseded by [ADR-0075](ADR-0075-tauri-everywhere-with-daemon.md)** (owner decision D22,
2026-09-29). Previously: Accepted additive migration (2026-09-04); overlay,
portal-parent and replacement qualification pending.

## Context

The Linux Tauri 2/Wry shell and the isolated GTK3 desktop-parent adapter retain
17 unsuppressed RustSec findings. A compatible Tauri upgrade cannot remove the
GTK3 binding family or Tauri's `urlpattern 0.3` dependency. The existing React
client, daemon protocol, theme persistence and event subscription logic are
valuable and have already been separated from Tauri-specific wrappers.

Current maintained bindings are `gtk4 0.11.4`, `webkit6 0.6.1` and
`glib 0.22.9`. They require Rust 1.92. Narrivy's project toolchain is Rust
1.97.1, while the separately promised Rust 1.85 floor applies only to the
enumerated portable packages. The desktop can therefore state a narrower,
truthful Rust 1.92 MSRV without changing the portable contract.

GTK4 has no equivalent of the GTK3 `gdk_wayland_window_export_handle` helper.
Selecting the rendering shell does not settle xdg-foreign parent export,
overlay positioning, or portal-operation lifecycle ownership.

## Decision

Add `narrivy-desktop-gtk` as the provisional Linux desktop shell and retain the
Tauri shell temporarily as a parity oracle. The new crate:

- uses exact `gtk4 0.11.4` and `webkit6 0.6.1` dependencies and declares Rust
  1.92;
- reuses the framework-neutral `bridge.rs`, `session_events.rs`, and `theme.rs`
  implementations directly, without importing Tauri or duplicating daemon
  policy;
- embeds the already-built React distribution at compile time, rejects
  symlinks/non-files, and limits the input to 512 files, 8 MiB per file, and
  32 MiB in total;
- serves assets only from the local, secure, display-isolated
  `narrivy://app/` scheme;
- injects the embedded entry bundle as a document-end WebKit `UserScript` and
  removes its external module tag from the served HTML, because the target
  WebKit did not execute that module tag from the custom scheme;
- applies a deny-by-default CSP with `connect-src 'none'`, uses an ephemeral
  network session with an unusable loopback proxy, ignores navigation outside
  `narrivy://app/`, and disables browser media/capture/WebRTC/WebAudio/WebGL,
  JavaScript clipboard access, automatic windows, and HTML5 databases;
- exposes one reply-capable WebKit script-message handler with a 64 KiB
  request limit and the exact existing command and argument names; and
- runs daemon commands and event subscription outside the GTK thread using
  bounded queues and explicitly owned workers that observe shutdown and join.

The frontend selects the WebKit transport only when its native handler exists;
otherwise it retains the Tauri transport. Session events use an in-memory DOM
`CustomEvent` and are never persisted by the transport.

CI must build the frontend before compiling the GTK crate and install the
GTK4/WebKitGTK 6 development packages. Portable native/MSRV jobs continue to
name their exact package set and do not compile the desktop crate.

## Security and privacy limits

The WebKit controls are defense in depth, not an authorization boundary. The
daemon remains responsible for same-user IPC authentication, command policy,
privacy decisions, and lifecycle state. The bridge must not receive raw portal
handles in JavaScript, logs, readiness files, or persistent state.

`NARRIVY_READY_FILE` and `NARRIVY_GTK_SMOKE_EXIT` are content-free diagnostic
hooks. Their output can establish that a development process mounted the React
root and completed a bridge reply; it is not schema-v2 release evidence and
must not be promoted into P11 qualification.

## Migration and removal gate

Tauri and `desktop-parent-linux` may be removed only after the GTK shell has:

1. main-window and overlay behavior parity, including truthful Wayland
   capability limits and non-focus behavior;
2. a reviewed GTK4 xdg-foreign portal-parent implementation with deterministic
   cancel → revoke → original terminal reply → unexport ordering;
3. explicit close/exit/restart behavior and zero leaked workers/handles;
4. native COSMIC testing and the required bounded release-lane qualification;
5. packaging/build integration from clean source; and
6. a dependency graph for which `cargo deny check` passes with no advisory
   exceptions.

Until then this is an additive migration, not a production-runtime support
claim. No package, installed lifecycle, release evidence, requirement PASS, or
platform qualification follows from this decision.

## Consequences

- The maintained shell and React bridge are implemented and testable without
  waiting for upstream Tauri/Wry GTK4 migration.
- The desktop MSRV is honest and isolated from the portable Rust 1.85 promise.
- The old advisory-bearing graph remains visible and release-blocking while
  parity work continues; no RustSec ignore is added.
- Overlay and portal-parent work remain the next architectural risks and must
  be solved without weakening the daemon-owned portal coordinator.

## Recovery inspection amendment — 2026-09-05

The fresh source baseline already contains a GTK4 `desktop-parent-linux`
implementation using gdk4-wayland 0.11.4, wayland-client 0.31.15 and
wayland-protocols 0.32.13. Its presence supersedes statements above describing
that adapter itself as retained GTK3. Tauri still retains its GTK3 graph. The
adapter owns xdg-foreign v1/v2 export and main-thread release; the desktop
portal coordinator remains inert and is not yet connected to the GTK shell.

The inherited process environment selected GDK_BACKEND=x11. Earlier shell
smokes on the COSMIC host therefore do not establish native Wayland execution.
A fresh diagnostic with GDK_BACKEND=wayland exported and released a parent
handle successfully. The GTK shell now records its actual GDK display type; a
fresh launch reported GdkWaylandDisplay, mounted React, completed theme_get,
observed zero external requests/JavaScript errors and exited successfully.

These observations establish separate development prerequisites only, not
portal grant/cancellation ownership, verified text delivery or release support.
The removal/integration gates above remain in force.

## Portal composition amendment — 2026-09-11

ADR-0066 connects the GTK4 xdg-foreign adapter, the pure portal-parent
coordinator, deadline-bound authenticated IPC workers, and the daemon-owned
direct-zbus portal service. The user action lives in Text Delivery settings and
is capability-gated; onboarding does not request permission. Close and exit
defer until cancel/revoke and local parent release complete for operations this
desktop owns. Current-host eligibility is keyboard-unavailable, so attended
permission and delivery behavior remain unqualified and removal gates 4–6 stay
open.
