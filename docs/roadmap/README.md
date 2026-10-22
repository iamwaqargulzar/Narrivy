# Narrivy Roadmap

Nothing on this page is a promise of a date. Each stage starts only after the
previous release is out and properly tested (owner decision 2026-09-28).

## 1.0 — Pop!_OS 24.04 COSMIC (Wayland)

Local, private system-wide dictation:
- a keyboard shortcut (toggle, and press-and-hold per owner decision D2);
- offline speech recognition (whisper; `base.en` if it meets the latency
  target, else `tiny.en`);
- text typed into the focused app;
- a History that keeps undelivered text recoverable (default 10 entries,
  configurable 0–1000);
- Raw/Clean modes, spoken aliases, a microphone picker, diagnostics and
  accessibility;
- a `.deb` package.

Status and remaining work: `agent/completion-plan/README.md`.

## After 1.0, in order

1. **Other Linux desktops and distributions:** Ubuntu 24.04 GNOME first,
   then KDE Plasma, wlroots, Fedora, Arch and Debian derivatives, each
   qualified natively.
2. **Windows.**
3. **macOS.**
4. **AMD accelerator (NPU/iGPU)** qualification, whenever hardware is
   available.

Features deferred from 1.0 by owner decision:
- privacy zones (D13);
- audio ducking and pause, and pre-roll (D14).

Details: `agent/completion-plan/07-post-release-side-quests.md`.

## Longer term (R12–R14)

- **R12:** advanced voice workflows: writing and translation modes,
  structured dictation, meetings, captions.
- **R13:** native Windows and macOS clients on the same portable core.
- **R14:** ecosystem: extension SDK, domain packs, teams, companion devices.

Roadmap work never weakens or delays the current release unless the owner
changes `BUILD_TARGET.json`.
