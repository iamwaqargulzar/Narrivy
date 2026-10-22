# ADR-0028 — GTK3 Wayland parent export is a narrow desktop adapter

**Status:** Accepted for a startup-disabled Recovery Phase 2C foundation
(2026-08-27)

## Context

The RemoteDesktop portal must parent its permission dialog to the visible
window which initiated the request. On Wayland this is not a raw surface
pointer: the client must asynchronously export its surface through
`xdg-foreign` and pass `wayland:<opaque handle>` to the portal. The handle must
remain exported until the permission request finishes.

Narrivy's desktop uses Tauri 2.11.5 on GTK3. Tauri can provide the invoking
`WebviewWindow`, schedule work on the application main thread, and expose its
GTK `ApplicationWindow`. GTK3's Rust bindings do not provide a safe wrapper for
`gdk_wayland_window_export_handle`, so a small FFI boundary is unavoidable.

The ashpd 0.12 raw-handle converter is not suitable for this ownership model.
Its locked implementation constructs Wayland objects from borrowed raw
pointers, starts an unowned standard thread, performs blocking Wayland
roundtrips without a Narrivy deadline, and unexports only when the returned
identifier is dropped. It also renders the handle from `Debug`. Those choices
are reasonable for its general API but conflict with Narrivy's bounded worker,
redaction, and explicit shutdown rules.

## Decision

- Add `adapters/desktop-parent-linux`, a Linux desktop integration adapter. It
  owns no portal, IPC, product policy, or UI behavior. Linking or constructing
  it cannot show a permission dialog.
- The public crate forbids unsafe code. The only exception is one private
  `native` module containing the GTK3 Wayland type check, export callback, and
  unexport call. Every unsafe block states its pointer, thread, allocation, and
  callback-lifetime invariants.
- Export is accepted only on GTK's initialized main thread from a visible,
  focused, realized Wayland `ApplicationWindow`. A process/thread-local
  registry permits exactly one active native export.
- GTK's asynchronous callback is returned through a bounded channel. Waiting
  on the GTK main thread is rejected, because that would starve the callback.
  Waiting borrows rather than consumes the request and uses the caller's
  absolute deadline.
- An accepted export returns an opaque ticket. Normal teardown must present the
  matching ticket on the GTK main thread and performs exactly one native
  unexport. A shutdown-only main-thread fail-safe can release the sole active
  export if its command owner disappeared.
- The parent value is validated as nonempty UTF-8, bounded to the existing
  4096-byte IPC limit, and free of control characters before the adapter adds
  the `wayland:` prefix. Its type has redacted `Debug`, no `Display`, and no
  serialization implementation. The only accessor is explicitly named for an
  immediate Rust-to-IPC borrow.
- Export requests and begin results are `must_use`. Callback cancellation,
  invalid data, rejection, focus/visibility failure, timeout, wrong-ticket
  release, and non-main-thread use are distinct content-free failures.
- Exact GTK dependencies are aligned with the versions already locked through
  the Tauri desktop build. No new runtime daemon or input privilege is added.
- This increment does not add the adapter as a desktop dependency, register a
  Tauri command, add a frontend action, inject the daemon portal service, or
  enable a pipeline. Those require a subsequent reviewed composition which
  owns window-close cancellation and keeps the export alive through the
  daemon's terminal permission result.

## Lifecycle consequence

The future desktop owner must schedule `begin_export` on Tauri's main thread,
await the callback on an owned non-UI worker, pass the secret directly into one
authenticated permission IPC request, and schedule ticket release on the main
thread in every terminal branch. Window close or application exit must be
prevented while the owner sends cancel followed by unconditional revoke and
reconciles the original permission request. Only then may it release the
parent and replay close/exit. The shutdown fail-safe is reserved for owner loss
or teardown and cannot substitute for confirmed normal cleanup. The parent must
never pass through TypeScript, status, events, ordinary logs, or disk.

Until that owner exists, the adapter is deliberately an isolated workspace
member and native permission remains unreachable from the shipped desktop and
daemon startup.

## Controlled verification

Unit tests call the callback boundary with isolated allocations and verify
valid prefixing, callback-before-destroy ownership, destroy-before-callback
cancellation, null/non-UTF-8/control/oversized rejection, secret and ticket
debug redaction, absolute-deadline timeout, single-flight admission,
wrong-ticket preservation, exact once-only release, second-release rejection,
and idempotent orphan shutdown cleanup. Focused tests and production Clippy run
without a compositor or portal.

The exact offline dependency audit remains red. It reports the GTK3 binding
family as unmaintained, `glib 0.18.5` as affected by RUSTSEC-2024-0429, and the
old macro dependency as unmaintained. Those versions already existed in the
workspace lock through Tauri before this adapter and this code does not use the
affected `VariantStrIter`, but that is not an exception or a release pass. The
desktop runtime dependency family needs a separate migration/containment
decision before release; this adapter remains controlled and unqualified.

These checks are not native parent export, portal, target-retention, or release
evidence. A compositor-backed export/unexport observation remains pending and
must be attended if it can open a permission prompt or affect another window.

## Primary sources reviewed

- [XDG Desktop Portal parent-window identifiers](https://flatpak.github.io/xdg-desktop-portal/docs/window-identifiers.html)
- [GTK 3.24 Wayland export implementation](https://github.com/GNOME/gtk/blob/gtk-3-24/gdk/wayland/gdkwindow-wayland.c)
- [GTK3 `gdk_wayland_window_export_handle` API](https://docs.gtk.org/gdk3-wayland/method.WaylandWindow.export_handle.html)
- [Tauri 2 `WebviewWindow` API](https://docs.rs/tauri/2.11.5/tauri/webview/struct.WebviewWindow.html)
- [ashpd 0.12 Wayland identifier implementation](https://docs.rs/ashpd/0.12.0/src/ashpd/window_identifier/wayland.rs.html)

## Alternatives rejected

1. Pass an empty parent. Rejected because permission UI would be unparented and
   the initiating-window authority would be false.
2. Send a raw pointer or exported handle through TypeScript. Rejected because
   it expands secret lifetime and lets presentation code forge platform state.
3. Use ashpd's raw-handle converter. Rejected for the unowned thread,
   unbounded roundtrips, pointer lifetime, and debug-disclosure mismatch above.
4. Put GTK/GDK FFI in the Tauri crate or general Linux platform adapter.
   Rejected because it would broaden unsafe/UI coupling beyond one reviewable
   desktop-specific boundary.
5. Keep a permanently exported main window. Rejected because authority must be
   scoped to one explicit permission operation and released afterward.
