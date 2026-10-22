# Overlay Experience

## Requirements
- never steal focus;
- correct monitor/workspace placement;
- visible within <100 ms target after trigger;
- reduced-motion alternative;
- scalable and high-DPI safe;
- local/cloud indicator when material;
- live waveform/activity only when useful;
- tentative text optional/configurable;
- delay-aware stage messaging (do not flash “Transcribing” for 40 ms);
- actionable error only when automatic recovery fails or is noteworthy.

## Adaptive forms
Normal short dictation: minimal pill/dot.
Long dictation: timer + waveform + optional partial text.
Meeting: duration + speaker/transcript state.
Error: concise recovery message.

## Appearance

The overlay follows the theme (dark or light) and accent (Mono, Blue or Gold)
chosen in Settings. The app window reports both to the host whenever they
change; the native Linux card maps them to the same values as
`src/theme/tokens.css` (a test fails when they drift).

The card has two sizes (owner direction, 2026-10-01):

- **Compact pill** (no live text): the logo's waveform, in the logo's
  gradient, at the centre of a small pill.
  - **Listening:** the waves move, each peak at its own pace, over a soft
    coral halo; a running `m:ss` timer sits beside them; the edge glows
    coral.
  - **Loading a model:** when a dictation starts with a pipeline whose
    model or device is not the loaded one, the waves settle, a light runs
    along them and the pill says a model is loading, until listening
    begins.
  - **Processing:** the same settled waves and running light; the label
    appears only after a short delay, so a quick stage never flashes it.
  - **Error:** still waves in the error colour and the recovery message.
- **Streaming card** (live text, from a streaming model): the pill widens to
  35% of the monitor first, then grows downwards. The logo stays centred
  above the text, the timer moves to the top corner, and the text is
  centred beneath. Each new line grows the card; beyond half the monitor
  height the text scrolls and fades out at the top.
- **Live text:** the newest three words are drawn at full strength and the
  rest slightly quieter; words that just arrived fade in.
- **Motion:** the card fades and rises in when it appears and fades out
  when it leaves. Every change of size eases; when the text ends the card
  shrinks back to the pill (height first, then width).
- The mark shows activity, not loudness: no audio level reaches the overlay,
  so nothing pretends to be a level meter.
- With the desktop's animations turned off, each state is one still frame and
  every size change happens at once.
- The card is click-through: pointer input reaches whatever is underneath.
- On Linux the overlay is a native GTK window with no web view
  (`tauri.linux.conf.json` configures only the main window), so no WebKit
  process runs for it.
- **Position:** bottom centre by default. Settings → Overlay offers six
  places (top or bottom; left, centre or right). The card keeps a small gap
  from the screen edge and stays clear of panels and docks. It always
  appears on the screen the pointer is on.

## COSMIC/Wayland platform truths (owner-fork-verified 2026-08)

Verified on Pop!_OS 24.04 / COSMIC Wayland with a 3× 1920×1200 mixed-DPI
setup during the owner's overlay work (full investigation:
`code/handy-npu/docs/2026-08-01-overlay-changes-investigation.md`). These are
binding design requirements for the P3 overlay, encoded as behavior (not
copied code):

1. **Never trust global pointer coordinates on COSMIC.** GDK reports the
   pointer as `(0,0)` regardless of actual position, and XWayland exposes a
   differently-scaled root (5760-logical vs 8640-px observed), clamping
   native-client pointers to edges. Correct monitor selection pattern: at
   each trigger, create an invisible 1×1 layer-shell surface with no
   requested monitor, let the compositor map it to the current pointer
   output, read that surface's monitor, assign the overlay there, destroy
   the probe. Pure layer-shell behavior — no input permissions, connector
   matching, or helpers.
2. **Clear transcript state on every overlay state entry.** A new session
   must never briefly show the previous session's text (observed defect when
   clearing was conditional on state).
3. **Monitor-relative geometry, not fixed size.** Width 35% of the selected
   monitor clamped to [400, 760] logical px (400 before first text; animate
   the first-text transition ~200 ms ease-out); height starts at one live
   line (~72 px), grows with wrapped text, scrolls beyond 50% of monitor
   height. Reference visual: card background ~rgba(16,18,24,0.82) with a
   restrained shadow — map to design tokens, do not hardcode.
4. **Services initialize in the daemon, not the UI.** A hidden/tray launch
   must not skip initialization the overlay/delivery depend on (observed as
   a paste failure after autostart). Narrivy corollary: the daemon owns all
   service init idempotently at startup; clients are always optional.
5. Mixed-DPI multi-monitor is a first-class test configuration for overlay
   placement (the owner's daily rig is exactly this).
