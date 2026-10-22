# ADR-0075: Tauri 2 desktop host on every OS, keeping the daemon architecture

Status: Accepted (owner decision D22, 2026-09-29). Supersedes ADR-0064 (the
GTK4/WebKitGTK 6 Linux shell) and the Tauri-removal intent of D8.

## Context

- ADR-0064 moved the Linux desktop from Tauri 2 (GTK3 / WebKitGTK 4.1) to a
  custom GTK4 / WebKitGTK 6 shell, because the Tauri graph carried 17
  unsuppressed RustSec findings. The GTK4 shell is Linux-only: WebKitGTK
  doesn't exist on Windows or macOS.
- The owner wants Narrivy to follow Handy (the closest open-source peer) for
  cross-platform capability, overlay and text handling. Handy 0.9.7 is one
  Tauri 2.11.5 app on Linux, Windows and macOS. Standard Tauri plugins
  provide the tray, autostart, single instance, global shortcuts (on
  Windows/macOS), clipboard, updater and macOS permissions, alongside `cpal`
  (audio), `enigo` (input), `handy-keys` (evdev shortcuts) and
  `gtk-layer-shell` (the Linux overlay).
- Measured on 2026-09-29, the retained Linux Tauri shell (older Tauri,
  Rust 1.85 pin) has 9 advisories: 6 unmaintained notices (`unic-*` via
  `urlpattern`, `proc-macro-error`) and 3 denial-of-service vulnerabilities
  (`quick-xml` via `plist`/`tauri-utils`, and `time`). The GTK3 soundness
  advisories behind ADR-0064 no longer appear. See
  `agent/completion-plan/09-cross-platform-architecture.md`.

## Decision

1. **The desktop host is Tauri 2 on every OS**, at the current release (Handy
   uses 2.11.5). The React UI is unchanged. The OS-neutral desktop code
   (`bridge.rs`, `session_events.rs`, `theme.rs`, `capability_ui.rs`) lives in
   the Tauri crate, or a small shared crate, and is not duplicated.
2. **The daemon architecture stays.** `narrivyd`, the portable core, local
   IPC, the recovery journal and History, the one-dispatch delivery rule, and
   helper isolation are unchanged. The Tauri app is a client of the daemon,
   exactly as the GTK shell was. It owns no domain policy (AGENTS.md
   architecture boundaries).
3. **Follow Handy where it is proven:**
   - the Linux overlay uses `gtk-layer-shell` (GTK3, in the Pop!_OS
     archives) with no keyboard interactivity, and the owner-fork monitor
     probe (`docs/product/ux/overlay.md`);
   - Tauri plugins for the tray, autostart (per D5, opt-in), single instance
     and clipboard;
   - `cpal` and `enigo`/paste for the Windows/macOS adapters;
   - `handy-keys` evdev for press-and-hold on Linux (the owner accepts the
     `input` group trade-off, D2/T2.13);
   - Handy's Linux typing-tool ladder as rungs of Narrivy's routed delivery.
4. **Security posture:**
   - upgrade Tauri and its dependencies to the newest compatible releases
     before accepting anything, and re-measure;
   - each remaining advisory is listed by ID in `deny.toml` `ignore`, with a
     one-line reason and a link to this ADR. The owner explicitly accepts
     that risk (D22);
   - nothing is ignored silently, and `cargo deny` stays a required check;
   - re-evaluate at every Tauri release. When an official Tauri release
     ships GTK4 / WebKitGTK 6 on Linux (wry PR #1767), adopt it and drop the
     ignores.
5. **The GTK4 shell (`apps/narrivy-desktop/src-gtk`) and
   `adapters/desktop-parent-linux` are retired** after the Tauri host reaches
   parity (completion plan WP1 T1.4). They remain in git history.

## Consequences

- Windows and macOS get a real desktop host path immediately; the per-OS
  adapter work is what remains (SQ2/SQ3).
- The Linux desktop depends on GTK3 / WebKitGTK 4.1
  (`libwebkit2gtk-4.1-0`, `libgtk-3-0`, `libgtk-layer-shell0`) instead of
  GTK4 / WebKitGTK 6.
- The locked-down WebKit settings of the GTK4 shell (a local-only asset
  scheme, no network or clipboard access from JavaScript, CSP) must be
  reproduced in Tauri's configuration (CSP, capabilities, no remote URLs) and
  kept under governance tests.
