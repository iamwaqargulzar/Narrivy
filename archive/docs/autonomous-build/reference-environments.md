# Reference Environment & Provisioning Policy

Linux 1.0 requires real qualification on both COSMIC/Wayland and GNOME/Wayland. The agent should provision the missing reference environment itself when safe/feasible (VM, spare host, test image, CI runner), rather than silently skipping it. Nested/virtualized environments are useful for install/UI/contract tests but do not replace physical hardware evidence for device/latency claims that virtualization invalidates.

Record exact OS release, kernel, compositor/desktop, portal versions, PipeWire, hardware, drivers, scaling, target app versions, Rust/Tauri/runtime/model versions and whether the session is VM/native. Unsupported or untested combinations are never marketed as supported.

Preferred release lanes per the legacy planning amendment Part H: bounded lanes defined in `docs/platforms/bounded-support.md` — Pop!_OS 24.04 LTS COSMIC and Ubuntu 24.04.2 LTS GNOME (exact kernels/desktops recorded at qualification), not open-ended “24.04+”. The agent rechecks the current supported LTS/desktop at release time and records evidence where practical.
