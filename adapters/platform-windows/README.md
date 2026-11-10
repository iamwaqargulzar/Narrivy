# Windows Platform Adapter

`narrivy-platform-windows` is the Windows counterpart of
`narrivy-platform-linux` (PORT-007, owner decision D36): microphone capture,
text delivery, the foreground target and global shortcuts, behind the
contracts in `narrivy-platform-api`.

**Status:** builds for `x86_64-pc-windows-gnu` (check and clippy clean) and
its portable logic is tested on Linux (66 tests). **Not yet run on Windows**:
the `#[ignore]`d tests below and a real dictation still need the Windows VM.
Not wired into `narrivyd` yet (see *Wiring* below).

## Layout

Portable modules (`#![forbid(unsafe_code)]`, build and test everywhere):

| Module | What it holds |
|---|---|
| `audio_format` | Endpoint format parsing (`WAVEFORMATEX`/`EXTENSIBLE`), decode (f32, i16, i24, i32), channel averaging, windowed-sinc resampling to 16 kHz |
| `capture` | Producer thread, bounded queue, batch and streaming sessions with the Linux terminal semantics |
| `delivery` | Typing and paste routes over injected keyboard/clipboard ports, the routed ladder, replay record, file Inbox |
| `text_input` | UTF-16 `KEYEVENTF_UNICODE` key events (surrogate pairs, Enter/Tab), Ctrl+V |
| `shortcuts` | The Linux shortcut model (`Super`/`Ctrl`/`Alt`/`Shift` + key name) mapped to `RegisterHotKey`; hold keys |
| `target` | Application identity from the process image path; bounded titles |
| `WindowsCapabilities` | `PlatformCapabilities`: shortcuts with release, active application, text insert |

`os` (Windows only) holds every `unsafe` call, each with a SAFETY comment:

| Module | Win32 / COM |
|---|---|
| `os::wasapi` | `WasapiCapture` (`AudioCapturePort` + `StreamingAudioCapturePort`), `list_input_devices` |
| `os::input` | `SendInputKeyboard`, `SendInputTextDelivery` (`TextDelivery`) |
| `os::clipboard` | `Win32Clipboard`: snapshot, write, ownership check, restore |
| `os::target` | `ForegroundTarget` (`TargetProvider`) |
| `os::hotkeys` | `ShortcutListener`, `WindowsShortcuts` (`ShortcutProvider`) |
| `os::routed_delivery` | The production `DeliveryPort` |

## Behaviour

**Capture.** WASAPI shared mode on the default console capture device (or a
chosen endpoint id), event-driven on a dedicated thread with MMCSS "Audio"
priority. The stream asks the engine for the contract format (mono f32,
16 kHz, `AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`); if the endpoint refuses, it
opens the mix format and `audio_format` converts. Chunks: 80 ms × 64 queued
for streaming (as Linux), 20 ms for batch. Same ends as Linux: Stop keeps
everything (`FinalizationRequested` / `EndpointDetected`), cancel is terminal,
the bound is `SafetyLimit`, a full queue is `capture_backpressure`. A device
that is unplugged or disabled mid-dictation (`AUDCLNT_E_DEVICE_INVALIDATED`)
ends as `SourceEnded` **with the audio captured so far** (D6, F-H2). A data
discontinuity after the first packet fails as `capture_overrun` (samples are
never dropped silently); watch for this in the VM. No first audio within 5 s
is `capture_ready_timeout`.

**Delivery.** The report has the Linux routed shape (`routed-then-inbox`:
direct attempts, then exactly one `inbox` attempt), so the portable recovery
tail validates it unchanged. Routes, in the order of the user's `type` /
`paste` setting:
- `windows-sendinput`: `KEYEVENTF_UNICODE` events in batches of 64; waits up
  to 1.5 s for modifier keys to be released; refuses control characters other
  than line breaks and tabs before sending anything; `SendInput` returning 0
  (UIPI: an elevated target window) is "nothing sent" and the ladder tries the
  other route; a partial send ends the ladder.
- `windows-clipboard-paste` (only with the clipboard authorization, as on
  Linux): snapshot every global-memory format, put the text on the clipboard
  (marked to stay out of Win+V history, cloud sync and monitors), Ctrl+V, hold
  500 ms, then restore the previous clipboard unless the user copied
  something newer. A clipboard that cannot be captured whole (metafiles, GDI
  objects, more than 64 MiB) is never replaced.
Both routes are unverifiable (`SubmittedUnverified`), so the Inbox write
always follows, as for `wtype` on Linux. Typing goes to the window focused at
delivery time (the `wtype` rule); no pre-bound target is required.

**Shortcuts.** Toggle: `RegisterHotKey` with `MOD_NOREPEAT` on a
message-loop thread; a combination another application holds is reported as
`shortcut_in_use`. Hold: a low-level keyboard hook watching the one chosen key
from the Linux hold-key list (Right Ctrl, Right Alt, Right Super, Menu, Scroll
Lock, Pause, F13, F14); every other key is dropped at once, our own injected
events are ignored, the key is never swallowed, and stopping reports a held
key as released. Callbacks run on a dispatcher thread so the hook never
waits.

## Wiring into narrivyd (not done here)

The `windows-compile` session owns the cfg-gating of `services/narrivyd`.
When it lands:

1. Add `narrivy-platform-windows = { path = "adapters/platform-windows" }` to
   `[workspace.dependencies]`, and to narrivyd under
   `[target.'cfg(windows)'.dependencies]`; move `narrivy-platform-linux` under
   `cfg(target_os = "linux")`.
2. **Capture** (`daemon/session.rs`, where `PipeWireCapture::new` is built):
   on Windows build `narrivy_platform_windows::os::WasapiCapture::new(seconds)`
   and, for a chosen microphone, `.with_device(DeviceSelection::Id(id))`.
   It implements both capture ports, so the batch and streaming lanes take it
   as they take `PipeWireCapture`. Echo cancellation (`super::echo`) has no
   Windows source yet: use the plain microphone.
3. **Delivery** (`routed_then_inbox_delivery` in `daemon/session.rs`): on
   Windows return `os::routed_delivery(method, clipboard_authorized,
   inbox_dir)` where `method` is `DeliveryMethod::parse` of
   `text_options::paste(composed)`'s choice. It is a `DeliveryPort` with the
   same intent backend (`routed-then-inbox`).
4. **State directory**: `text_options.rs` and `main.rs` use
   `narrivy_platform_linux::instance::state_dir()` and a runtime-dir Inbox.
   On Windows use `%LOCALAPPDATA%\Narrivy\state` and `...\Narrivy\inbox`
   (owner-only by inheritance); that helper belongs in this crate or the
   `windows-compile` session's cfg layer.
5. **Activation**: start `os::ShortcutListener` (or `WindowsShortcuts` for the
   `ShortcutProvider` contract) with the configured shortcut parsed by
   `shortcuts::Hotkey::parse`, and route `ShortcutEvent::Pressed` /
   `Hold(Pressed|Released)` to the same start/stop/hold handlers the Linux
   trigger path uses. Keep the listener alive for the daemon's lifetime and
   `stop()` it on shutdown.
6. **Target**: `os::ForegroundTarget` where the session records its target
   context; `WindowsCapabilities` where capabilities are reported.
7. **Session**: the daemon must run in the user's interactive session (started
   at logon), not as a Windows service: services live in session 0, where
   `SendInput`, the clipboard and global shortcuts do not reach the user's
   desktop.

## Testing

Linux (portable logic):

```bash
cargo test -p narrivy-platform-windows
```

Cross-checks on this Linux host:

```bash
cargo clippy --locked --target x86_64-pc-windows-gnu -p narrivy-platform-windows --all-targets -- -D warnings
```

Windows: build the test executable here with the MinGW toolchain
(`~/.local/mingw-bin`), copy it to the VM and run it there, ignored tests
included (they use the microphone, clipboard, desktop and a global shortcut):

```bash
PATH=~/.local/mingw-bin:$PATH cargo test --locked --no-run --target x86_64-pc-windows-gnu -p narrivy-platform-windows
```

```bash
narrivy_platform_windows-<hash>.exe --include-ignored --test-threads=1
```

## Known gaps

- Not run on Windows yet; the emulated HDA card in the VM is the first real
  capture test, real hardware the final one (D36).
- No UI Automation yet (selected/surrounding text, verified insertion), no
  WASAPI loopback (system audio), no Credential Manager secret store, no echo
  cancellation source.
- Typing into an elevated window needs Narrivy elevated or `uiAccess`; until
  then such deliveries end in the Inbox (`delivery_input_blocked`).
