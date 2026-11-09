# Platform Portability

## Terminology (owner-clarified 2026-08-22)

"**Portable**" in this repository means **cross-platform-neutral**: the
portable core is domain code with zero OS coupling that compiles on
Linux/Windows/macOS, with each OS providing its own integration path
(`adapters/platform-*`, per-platform modules/bindings, per-OS capabilities).
It never means a "portable app" distribution. The one unrelated usage is
packaging prose that once called the AppImage a "portable artifact" (=
self-contained single-file package; AppImage itself was dropped from 1.0
scope by owner decision 2026-08-22); prefer "self-contained artifact" in
new text to avoid ambiguity.

## Rule

Linux is the first product adapter, not the architectural substrate.

## Capability model

A platform implementation reports capabilities such as:
- global shortcut press/release;
- mouse/alternate trigger support;
- active app/window identity;
- selected/surrounding text;
- text insert/replace;
- direct text patching/streaming support;
- overlay anchoring;
- media pause/resume;
- secret store;
- autostart/service lifecycle;
- system audio capture;
- accessibility tree access.

The UI renders based on these capabilities.

## Portable IPC
- Unix: Unix domain sockets preferred initially.
- Windows: named pipes.
- Protocol types/versioning are transport-independent.
- D-Bus may be an optional Linux integration surface, never an internal hard dependency.

## Compile gate

Portable crates must compile in CI for Linux, Windows MSVC and macOS targets even before full platform adapters exist.
