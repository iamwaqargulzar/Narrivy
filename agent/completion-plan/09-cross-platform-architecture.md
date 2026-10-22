# Cross-platform architecture review (2026-09-29)

**Question from the owner:** before removing the old Tauri shell, which paths
would make Narrivy *better* across Linux, Windows and macOS?

**Short answer:** the choice of desktop shell (the window hosting the UI) is
the smallest part of cross-platform work. What decides portability is:
1. keeping the React UI and the shared desktop code independent of any
   shell;
2. a clean platform-adapter layer for the four OS-specific jobs: typing text
   into apps, global shortcuts, audio capture, and background-service
   lifecycle.

Narrivy's portable core (`crates/*`) already does (2) well. The weak spots
are the shell-shared code, which sits in a dead folder today, and
**Linux text insertion, which depends on `wtype`, with conflicting reports
about it on COSMIC** (finding F-C8 below).

Sources were checked on 2026-09-29 and are listed at the end. Re-check
volatile facts (the wry GTK4 PR, the CEF/Servo runtimes) before acting.

---

## 1. Desktop shell options (window that hosts the React UI)

| Option | Linux | Windows / macOS | Keeps the React UI? | Verdict |
|---|---|---|---|---|
| **A. Own GTK4 + WebKitGTK 6 shell** (today) | Excellent: maintained bindings, no advisories, locked down | **None.** WebKitGTK does not exist on Windows or macOS | Yes | Keep for Linux 1.0 |
| **B. Tauri 2** (system web views) | GTK3 + WebKitGTK 4.1 today, with the 17 RustSec advisories that caused ADR-0064. The GTK4/webkit6 port of wry ([PR #1767](https://github.com/tauri-apps/wry/pull/1767)) is an **open community PR** (July 2026, unmerged, no maintainer timeline) | Good: WebView2 / WKWebView; the Linux advisories don't apply | Yes | **Best candidate for Windows/macOS (SQ2/SQ3).** Revisit for Linux only after an official Tauri release ships GTK4/webkit6 |
| C. Tauri + CEF runtime (Chromium everywhere) | Consistent engine, no WebKitGTK | Same engine everywhere | Yes | Heavier (≈100+ MB per install), community runtime ([tauri-runtime-cef](https://github.com/SableClient/tauri-runtime-cef), on [cef-rs](https://github.com/tauri-apps/cef-rs)). Keep as a fallback if web-view differences hurt |
| D. Tauri + Servo/Verso | — | — | Yes | Experimental; [reported stalled](https://github.com/versotile-org/tauri-runtime-verso). Not viable |
| E. Electron | Mature | Mature | Yes | Large footprint and a Node runtime inside a privacy product. Not recommended |
| F. Native Rust UI (Slint, Dioxus-native, egui, iced) | Varies | Varies | **No: rewrite the ~9k-line UI** | Only if the web UI itself becomes the problem. Slint and Dioxus have the best accessibility |
| G. Qt 6 / QML ([CXX-Qt](https://github.com/KDAB/cxx-qt)) | Excellent Wayland support (layer-shell via LayerShellQt), strong accessibility | Excellent | **No: rewrite the UI in QML** | Strongest native option, but a rewrite plus a C++ toolchain and LGPL. Not now |

**Superseded 2026-09-29 by owner decision D22 (ADR-0075): Tauri 2 on every OS, like Handy, keeping the daemon. The measured analysis is below in §6.** Original recommendation: keep the React UI and use **thin per-OS hosts** over one
shared, OS-neutral `shell-core` crate:
- Linux: the GTK4 shell now;
- Windows and macOS: most likely Tauri 2 when SQ2/SQ3 start, decided then;
- Linux could move to Tauri later, once an official Tauri release has
  GTK4/webkit6 and clears the advisories.

This is "path 4", refined in §4.

## 2. The real cross-platform work: platform adapters

Each job below already has, or needs, a portable contract in
`crates/narrivy-platform-api` or the delivery port, with one adapter per OS.

| Job | Linux (1.0) | Windows (SQ2) | macOS (SQ3) | Notes |
|---|---|---|---|---|
| **Type text into the focused app** | Today: the `wtype` helper (**unverified on COSMIC, F-C8**). Recommended: a **native `zwp_virtual_keyboard_v1` client** in `adapters/platform-linux` that synthesises a keymap per text, as [cosmic-voice](https://github.com/techgeek1/cosmic-voice) does. It types arbitrary Unicode (this also fixes F-H8) with no external tool. Later: `zwp_input_method_v2`, and the portal/libei route for GNOME (SQ1) | `SendInput` with `KEYEVENTF_UNICODE` first; Text Services Framework later (Windows' own dictation path). Elevated apps can't receive SendInput | Accessibility API (`AXUIElement`, `kAXSelectedTextAttribute`) with a CGEvent/paste fallback; needs the user-granted **Accessibility** permission and a signed, non-sandboxed app | The biggest per-OS effort. The existing routed ladder and the one-dispatch rule (F-C2) carry over unchanged |
| **Global shortcut** (toggle and hold) | The compositor's custom shortcut runs `narrivy-cli toggle` (today). The portal GlobalShortcuts where a desktop supports it. Hold needs key-release events (T2.13) | `RegisterHotKey`; a low-level keyboard hook for hold | `CGEventTap` (needs **Input Monitoring** permission) | The [`global-hotkey`](https://github.com/tauri-apps/global-hotkey) crate covers Windows/macOS and X11, **not Wayland** |
| **Audio capture** | The `pw-record` helper in an isolated process (today) | [cpal](https://github.com/RustAudio/cpal) → WASAPI | cpal → CoreAudio | cpal also has a PipeWire backend. Keep `pw-record` on Linux for its isolation; add cpal adapters for Windows/macOS behind the same capture contract |
| **Background service** | systemd user unit (today) | Per-user startup task (Task Scheduler or a Run key) | launchd LaunchAgent (`SMAppService`) | D5 (opt-in autostart) maps to all three |
| **Local IPC** | Unix socket with `SO_PEERCRED` (today, `adapters/ipc-unix`) | Named pipe with an owner-only ACL | Unix socket (the same adapter works) | [`interprocess`](https://docs.rs/interprocess) offers local sockets (named pipes on Windows) with peer credentials. Either adopt it for Windows, or add `adapters/ipc-windows` (PORT-007). **`shell-core` must not hard-wire `ipc-unix`** (§4) |
| **Speech recognition** | transcribe.cpp CPU (today) | transcribe.cpp CPU/Vulkan/CUDA | transcribe.cpp **Metal** | [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) supports all three, so the engine is already cross-platform |
| **Overlay window** | Must not take focus (F-H11). Handy [disables its overlay on Wayland](https://github.com/cjpais/Handy) for exactly this reason. Layer-shell is the proper fix | Tool window, non-activating | Non-activating panel | Part of each host |
| **Packaging** | `.deb` (1.0). Flatpak is **hard** for this app: sandboxed input injection and a background daemon | MSI/MSIX installer, code-signed | Signed and notarized `.dmg` | Per-OS release lanes, like WP5/WP6 |

## 3. Finding raised by this review

**F-C8 — `wtype` on COSMIC: conflicting reports (native-verify).**
- **Against:** a February 2026 write-up
  ([ai-lab-notes](https://codeshrew.github.io/ai-lab-notes/posts/2026-02-11_voice-dictation-cosmic-wayland/))
  saw `wtype` output arrive as key numbers on COSMIC ("hello" →
  `84 101 108 108 111`). It also saw `wtype` fail silently when launched
  from a COSMIC keybinding's `Spawn()` context.
- **For:** an OpenWhispr maintainer states "wtype works correctly on
  cosmic-comp (verified: cosmic-comp implements the virtual-keyboard
  protocol)" ([issue #1987](https://github.com/OpenWhispr/openwhispr/issues/1987)).
  The owner's Handy fork (Handy-Pop-OS 0.9.5, tested on COSMIC in August
  2026) uses `wtype` as its default Wayland typing tool.
- **Narrivy has no recorded evidence either way:** the shipped package is
  Inbox-only (F-C1), and the P0F probe never typed anything.
- **Check (WP3 smoke, 5 minutes):**
  - in a COSMIC Text window, `sleep 3; wtype 'hello café — it’s'` from a
    terminal;
  - the same through the installed daemon, which runs as a systemd service,
    **not** from a keybinding's `Spawn()`, once F-C1 is fixed.
- **If it fails:** replace the rung with a native `zwp_virtual_keyboard_v1`
  client with a per-text keymap, as
  [cosmic-voice](https://github.com/techgeek1/cosmic-voice) does. That also
  fixes F-H8 and drops the external dependency.
- **If it works:** keep `wtype` for 1.0 (with the F-H8 locale fix), and
  consider the native client later.

## 3a. What Handy does (for comparison)

Handy ([upstream](https://github.com/cjpais/Handy), about 20k stars,
biweekly releases) is the closest open-source peer. Checked: **upstream
0.9.7** (released 2026-09-18, its `Cargo.toml` and `clipboard.rs` on main,
plus the 0.9.6 and 0.9.7 release notes), and the owner's fork Handy-Pop-OS
(a local checkout, based on 0.9.5), read locally.

**What changed in 0.9.6/0.9.7 (relevant here):**
- **Wayland overlay restored:** Linux now depends on `gtk-layer-shell` 0.8
  (with `gtk` 0.18, i.e. **still GTK3**), and macOS uses `tauri-nspanel`, a
  non-activating panel. The older upstream README advice to disable the
  overlay on Wayland is out of date.
- **GPU speech on all three OSes:** `transcribe-cpp` 0.2.4 with Vulkan and
  dynamic backends on Linux and Windows, and Metal on macOS.
- **Paste:** clipboard paste methods now work on GNOME Wayland; ydotool key
  syntax is detected before pasting; stale modifiers are cleared after
  xdotool typing.
- **Audio:** the mic callback is real-time safe; tail audio is no longer lost
  at stop; Handy falls back to the default mic after a disconnect (compare
  Narrivy F-H2); an experimental VAD was added.
- **Push-to-talk:** toggle parity is kept when presses arrive mid-pipeline.
- **Unchanged:** the Linux typing ladder (`kwtype` / `wtype` → `dotool` →
  `ydotool`, then xdotool on X11, then enigo). There is still no libei,
  portal or native virtual keyboard, and no evdev crate in Handy itself;
  evdev comes through `handy-keys`.

- **Stack (0.9.7):** one Tauri 2.11.5 app, Rust with a React/TypeScript UI, on all three
  OSes. It uses Tauri plugins (global-shortcut, clipboard-manager,
  autostart, single-instance, updater, macos-permissions), `cpal` + `rubato`
  (audio capture and resampling), `vad-rs` (Silero VAD), `enigo` (simulated
  input), `rdev` and `handy-keys` (keyboard listening), and
  **`transcribe-cpp`** plus `transcribe-rs` (ONNX) for speech. Windows uses
  the `windows` crate. There is **no separate daemon**: everything runs in
  the one app process.
- **Cross-platform approach:** one codebase with `cfg(target_os)` branches
  inside the app. The same Tauri shell runs everywhere (WebKitGTK/GTK3 on
  Linux, WebView2 on Windows, WKWebView on macOS).
- **Text insertion:**
  - Windows and macOS default to **clipboard + Ctrl/Cmd+V** via enigo, then
    restore the clipboard.
  - Linux defaults to **"Direct" typing** through an external tool, in
    order:
    - KDE Wayland: `kwtype` (KDE Fake Input; handles umlauts);
    - other Wayland: `wtype` → `dotool` → `ydotool`;
    - X11: `xdotool` → `ydotool`;
    - `enigo` as the last fallback.
  - The user can pin a tool (`TypingTool`: Auto/Wtype/Kwtype/Dotool/Ydotool/Xdotool).
  - Clipboard paste on Wayland uses `wl-copy` plus a key combo sent by the
    same tools. Upstream notes that `wtype` doesn't work on KDE or on
    GNOME/Mutter.
  - It uses **no libei, portal or native virtual-keyboard client**.
- **Global shortcuts:** Tauri's plugin (which [doesn't work on COSMIC or other
  Wayland compositors](https://github.com/cjpais/Handy/issues/949)), plus
  `handy-keys`, which **reads `/dev/input/event*` directly (evdev)** and
  needs the user in the `input` group. That gives real press-and-hold with
  release events on any Wayland desktop. The cost: `input` group membership
  lets any process of that user read all keystrokes. Also available: CLI
  flags and `SIGUSR2`.
- **Overlay:** since 0.9.6, upstream uses a GTK3 layer-shell overlay on
  Wayland (older docs said to disable it because it
  [stole focus](https://github.com/cjpais/Handy)). **The owner's fork solved
  this on COSMIC before that:** a native GTK layer-shell card that never takes focus,
  placed on the right monitor through a 1×1 probe surface. See the fork's
  `docs/2026-08-01-overlay-changes-investigation.md`, the file Narrivy's
  overlay spec cites but doesn't contain. This is owner-authored work, so
  under the clean-room policy it can be adapted for F-H11.

**Lessons for Narrivy:**
- **Keep:**
  - Narrivy's separate daemon and IPC. It allows a background service, a
    CLI and crash recovery, which Handy's single process doesn't.
  - The strict helper isolation.
  - The per-OS adapter contracts; they're stronger than Handy's inline
    `cfg` branches.
- **Adopt:**
  - clipboard + paste as the **default on Windows/macOS** (SQ2/SQ3);
  - a **tool ladder with a user override** on Linux (Narrivy's routed
    ladder already has the shape);
  - the fork's **layer-shell overlay** approach for F-H11.
- **Decide (T2.13 hold on COSMIC):** evdev (`input` group) gives reliable
  press-and-hold but is a keylogging-capable permission. That's an owner
  trade-off to present if T2.13 finds no portal route.

## 4. Refined path 4 (what to do now)

1. Create **`apps/narrivy-desktop/shell-core`** (crate `narrivy-desktop-core`)
   holding the OS-neutral desktop code: `bridge.rs`, `session_events.rs`,
   `theme.rs`, `capability_ui.rs`. It has no GTK and no Tauri. Its tests
   run on every OS.
2. `bridge.rs` and `session_events.rs` use `narrivy-ipc-unix` directly.
   Keep that for now, behind `#[cfg(unix)]`, and add a small `transport`
   seam (a trait or a module) so a Windows named-pipe transport can slot
   in later without touching the command logic. (Implement it in SQ2, not
   now.)
3. `portal_parent.rs` (Linux portal parent handles) stays in the **GTK
   crate**; it is Linux-specific.
4. The GTK shell depends on `shell-core`. The Linux Tauri wrapper
   (`src-tauri`) is removed; it stays in git history.
5. The frontend keeps a single native-bridge transport. Outside a native
   host, every call rejects with a typed `daemon_unavailable` error.
6. Governance tests keep checking the command list between the frontend
   and the GTK dispatcher, and `shell-core` keeps the capability-state and
   no-OS-branching checks.

## 5. What this changes in the plan

- New finding **F-C8** (above), in the WP3 smoke and WP2.
- WP1 T1.4 (Tauri removal) is done the refined path-4 way.
- `07-post-release-side-quests.md` SQ2/SQ3: start from §2's adapter table;
  the host choice (Tauri 2 vs others) is made at that time.
- Track [wry PR #1767](https://github.com/tauri-apps/wry/pull/1767): if an
  official Tauri release adopts GTK4/webkit6, reconsider Tauri for Linux
  with a new ADR.

## Sources

- wry GTK4/webkit6 migration PR: <https://github.com/tauri-apps/wry/pull/1767>
- Tauri CEF runtime: <https://github.com/SableClient/tauri-runtime-cef>; cef-rs: <https://github.com/tauri-apps/cef-rs>
- Tauri Verso runtime: <https://github.com/versotile-org/tauri-runtime-verso>; Tauri blog: <https://v2.tauri.app/blog/tauri-verso-integration/>
- Handy (Tauri dictation app, Wayland limits): <https://github.com/cjpais/Handy>
- wtype on COSMIC: <https://codeshrew.github.io/ai-lab-notes/posts/2026-02-11_voice-dictation-cosmic-wayland/>
- cosmic-voice (virtual keyboard / input-method-v2 on COSMIC): <https://github.com/techgeek1/cosmic-voice>
- global-hotkey Wayland limitation: <https://github.com/tauri-apps/global-hotkey/issues/28>
- enigo (libei experimental): <https://github.com/enigo-rs/enigo>
- cpal backends: <https://github.com/RustAudio/cpal>
- interprocess local sockets: <https://docs.rs/interprocess/latest/interprocess/local_socket/index.html>
- transcribe.cpp backends: <https://github.com/handy-computer/transcribe.cpp>
- CXX-Qt: <https://github.com/KDAB/cxx-qt>
- macOS text insertion approaches: <https://levelup.gitconnected.com/swift-macos-insert-text-to-other-active-applications-two-ways-9e2d712ae293>
- Windows SendInput limits: <https://dev.to/howmindswork/how-i-inject-text-into-any-windows-app-including-elevated-processes-4jl6>
- OpenWhispr on COSMIC (wtype verified working): <https://github.com/OpenWhispr/openwhispr/issues/1987>
- Handy Wayland meta issue: <https://github.com/cjpais/Handy/issues/1555>
- Handy global shortcuts on COSMIC/Wayland: <https://github.com/cjpais/Handy/issues/949>
- Handy upstream text insertion: <https://raw.githubusercontent.com/cjpais/Handy/main/src-tauri/src/clipboard.rs>
- Owner's fork, read locally: a local checkout (Handy-Pop-OS 0.9.5): `src-tauri/Cargo.toml`, `src-tauri/src/clipboard.rs`, `settings.rs`, `shortcut/handy_keys.rs`, `LINUX.md`, `HANDOVER.md`; `handy-keys` 0.3.4 README (evdev)
- Handy releases (0.9.6, 0.9.7): <https://github.com/cjpais/Handy/releases>; upstream Cargo.toml: <https://raw.githubusercontent.com/cjpais/Handy/main/src-tauri/Cargo.toml>

## 6. Narrivy's approach vs Handy's for cross-platform capability (2026-09-29, measured)

| Criterion | Narrivy (daemon + GTK4 shell) | Handy (one Tauri app) | Better |
|---|---|---|---|
| Windows/macOS readiness | `platform-windows`/`platform-macos` empty; Unix-socket IPC only; `pw-record`/`wtype` Linux-only | Ships on all three through Tauri plugins, `cpal` and `enigo` | Handy |
| COSMIC behaviour today | Careful designs, but the package can't type yet | Tool ladder, evdev hold and layer-shell overlay, field-tested | Handy (adopt Narrivy's safety rules) |
| Robustness | Daemon: crash recovery, CLI activation, never-lost text, isolated helpers | Single process | Narrivy |
| Security | Stricter; 0 advisories (GTK4) | Tauri core on Linux: 9 advisories (6 unmaintained, 3 DoS) in the retained shell; the Handy fork lock has 22, mostly from network/updater features Narrivy lacks | Narrivy (gap is moderate and partly fixable by upgrading) |
| Contributor familiarity | Two shells and custom machinery | Standard Tauri | Handy |

**Decision (D22):** a hybrid. Handy's Tauri 2 host and cross-platform crates,
plus Narrivy's daemon, portable core and delivery safety. See ADR-0075.
