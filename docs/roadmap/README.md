# Roadmap

This page lists what is planned, in rough order. It is not a schedule: each
step starts once the previous release has been tested.

## 1.0

1.0.0-rc.33 is the current release candidate, for Pop!_OS 24.04 (COSMIC,
Wayland) and 64-bit Windows 11. Before 1.0:

- the meeting add-on, so the packages transcribe meetings with speakers;
- code signing for the Windows installer;
- wider hardware testing on both systems.

## After 1.0

1. **More Linux desktops and distributions:** Ubuntu 24.04 with GNOME first
   ([notes](platforms/gnome.md)), then KDE Plasma and wlroots compositors
   ([notes](platforms/kde-wlroots.md)), Fedora, Arch and Debian.
2. **macOS** ([notes](platforms/macos.md)).
3. **AMD NPU and GPU acceleration**, and an NVIDIA CUDA add-on.
4. **Firefox** support for the writing assistant.
5. Features held back from 1.0: privacy zones, audio ducking and pause, and
   audio pre-roll.

## Longer term

- **Advanced voice workflows:** writing and translation modes, structured
  dictation, live captions ([R12](phases/R12-advanced-voice-meetings.md)).
- **Ecosystem:** an extension SDK, domain packs and companion devices
  ([R14](phases/R14-ecosystem.md)).

The design notes in [`design/`](design/) and [`speech/`](speech/) describe
features that are not built yet.
