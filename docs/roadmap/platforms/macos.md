# macOS Portability Contract

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Future adapter likely maps:
- audio → CoreAudio;
- app/context/selection → NSWorkspace + Accessibility APIs;
- text delivery → Accessibility/event APIs;
- shortcut → native event/hotkey APIs;
- secret store → Keychain;
- lifecycle → login item/LaunchAgent where appropriate;
- compute → CPU/Metal/CoreML where supported.

Permission UX for microphone/accessibility must be treated as a product journey, not a setup footnote.
