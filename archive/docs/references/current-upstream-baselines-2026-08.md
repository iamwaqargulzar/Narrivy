# Volatile Upstream Baselines — 2026-08

These are research anchors, not permanent pins. Re-check before implementation/release.

- Rust official releases: https://blog.rust-lang.org/releases/ — Rust 1.97.1 was the latest listed stable point release checked for this pack.
- XDG Desktop Portal GlobalShortcuts: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html — qualify actual desktop portal implementation and activation/deactivation semantics.
- OpenVINO 2026 system requirements: https://docs.openvino.ai/2026/about-openvino/release-notes-openvino/system-requirements.html — accelerator drivers are separate; Linux/NPU support varies by OS/kernel.
- OpenVINO NPU device: https://docs.openvino.ai/2026/openvino-workflow/running-inference/inference-devices-and-modes/npu-device.html — NPU execution requires compatible Intel Core Ultra/NPU hardware, driver and runtime/plugin; recheck exact supported combinations.
- System76 Pop!_OS basics: https://support.system76.com/support/articles/pop-basics — Pop!_OS 24.04 includes COSMIC.
- Ubuntu release lifecycle: https://ubuntu.com/about/release-cycle — recheck supported LTS/current releases at release qualification.

`agent/RESOLVED_VERSIONS.json` stores the versions actually selected and tested; this reference file merely tells the agent where to verify assumptions.
