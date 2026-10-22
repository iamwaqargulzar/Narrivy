# Current Upstream Platform Notes

**Research snapshot:** 2026-08-21. These are volatile facts; coding agent rechecks authoritative sources during the relevant phase and records exact tested versions.

## Rust

Official Rust release page indicated Rust 1.97.1 as the current stable point release at this snapshot. Narrivy v0.3 pins it for deterministic initial work while keeping portable MSRV 1.85.0 separate.

Source: https://blog.rust-lang.org/releases/latest/

## Desktop shell / Linux prerequisites

Narrivy's maintained Linux desktop shell is GTK4 + WebKitGTK 6
(`narrivy-desktop-gtk`, exact `gtk4 0.11.4` / `webkit6 0.6.1`, Rust 1.92
MSRV) per ADR-0064. Debian/Ubuntu prerequisites are the GTK 4 and
WebKitGTK 6 development packages. The earlier Tauri 2 / WebKitGTK 4.1
prerequisites remain recorded here because the Tauri shell is retained
temporarily as a migration parity oracle and the P0F feasibility probe still
targets it; the Tauri shell is excluded from the workspace
(`Cargo.toml` `exclude`) and is not the release runtime. Packaging guidance
notes Linux binary compatibility is influenced by the build system's
glibc/base distribution, so P10 builds artifacts on a suitably compatible
baseline and tests on clean targets.

Sources:
- https://v2.tauri.app/start/prerequisites/ (Tauri parity shell, historical)
- https://v2.tauri.app/distribute/appimage/ (Tauri parity shell, historical)
- https://v2.tauri.app/distribute/ (Tauri parity shell, historical)

## XDG GlobalShortcuts portal

The current portal interface includes both `Activated` and `Deactivated` signals. This makes press/release-style semantics representable, but Narrivy still qualifies the actual desktop portal implementation before advertising reliable PTT. The canonical portal object path is `/org/freedesktop/portal/desktop` (lowercase `desktop`); Narrivy checks the exact interface there, never a desktop name.

Source: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html

## OpenVINO / NPU / Whisper

OpenVINO 2026 documentation describes CPU/GPU/NPU devices and NPU platform/driver requirements. OpenVINO GenAI includes Whisper speech-recognition support and publishes supported model/device combinations. Because support and runtime issues can be model/device/version-specific, Narrivy requires per-pair health qualification plus CPU fallback.

Sources:
- https://docs.openvino.ai/2026/about-openvino/release-notes-openvino/system-requirements.html
- https://docs.openvino.ai/2026/openvino-workflow/running-inference/inference-devices-and-modes/npu-device.html
- https://openvinotoolkit.github.io/openvino.genai/docs/samples/cpp/whisper_speech_recognition/
- https://openvinotoolkit.github.io/openvino.genai/docs/supported-models/
- https://github.com/openvinotoolkit/openvino.genai
- https://github.com/openvinotoolkit/openvino/releases/tag/2026.4.0

## Accessibility

Narrivy uses applicable WCAG 2.2 AA principles for its webview UI plus desktop/platform accessibility semantics and assistive-technology testing.

Source: https://www.w3.org/TR/WCAG22/

## Rule

URLs in this file are research inputs, not permanent support claims. Release evidence records the exact versions actually tested.

## Cross-reference
- Full per-node upstream version/module audit (verified 2026-08-22): `archive/docs/references/upstream-version-audit-2026-08-22.md`
