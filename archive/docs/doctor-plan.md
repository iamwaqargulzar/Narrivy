# Doctor & Diagnostics Plan — legacy planning Part N

`scripts/doctor.py` is the non-destructive host capability checker. Per the current build contract, it must emit both human-readable and structured JSON (`--json`).

## Progressive checks

| Phase when relevant | Check | JSON field | How |
|---|---|---|---|
| P0 | toolchain, cargo-deny, node, git, cmake | `toolchain.*` | `which` + `--version` |
| P0F | desktop/session (`XDG_SESSION_TYPE`, `XDG_CURRENT_DESKTOP`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`) | `desktop.*` | env |
| P0F | portal backend (`org.freedesktop.portal.Desktop`) | `portal` | `busctl --user status` |
| P4 | PipeWire / WirePlumber (`pw-cli`, `wpctl`, `pactl`) | `pipewire`, `microphone` | version/status |
| P4 | libei/EIS or chosen delivery dep | `libei_eis` | `pkg-config` |
| P3 | clipboard tools (`wl-copy`, `xclip`) | `clipboard.*` | version |
| P3 | uinput availability (advanced diagnostic only) | `uinput` | `/dev/uinput` exists |
| P8 | Secret Service / credential facility | `secret_service` | `busctl` |
| P2 | WebKitGTK / Tauri prerequisites | `webkit_tauri` | `pkg-config` |
| P5 | CPU/NPU/dGPU/iGPU discovery plus relevant runtime-pack eligibility (`vulkaninfo`, vendor/runtime probes as installed) | `compute.*`, `runtime_packs.*` | facts/version; discovery is not qualification |
| P5 | disk / model cache budget | `disk.*` | `XDG_CACHE_HOME/narrivy` du |
| P10 | package prerequisites (`dpkg`, `appimagetool`) | `toolchain` + extra | version |

Diagnostics must **redact** transcript/audio/clipboard/selection/context content by default (see `PRIV-006`, `REL-008`).

## Integration with other docs

- `agent/phase-reports/*.md` may embed `doctor --json` output hash.
- `narrivy-qualify` captures `doctor --json` as part of environment metadata for qualification evidence.
- `docs/product/ux/screens/diagnostics.md` maps doctor checks to the Diagnostics screen's “Run Full System Test”.

## Failure policy

Doctor never mutates system policy, never installs runtime packs/packages automatically (bootstrap/provisioning is explicit), and never requires `sudo`. Runtime-pack recommendations include expected size and reason but acquisition is a separate policy/user-authorized action.
