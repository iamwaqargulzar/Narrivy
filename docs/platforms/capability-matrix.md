# Platform Capability Matrix — Claim Policy

Capabilities are graded only from evidence: `supported`, `supported-with-fallback`, `experimental`, `unavailable`, `unknown/unqualified`.

Per the current build contract, P0F probes establish real capabilities before architecture hardens; P2 proves the walking skeleton on both lanes; P11 records bounded qualification evidence. Aspirational words such as “target” are not support claims. The UI must surface degradation honestly and must not derive support from this static table; runtime capability probes are authoritative for the current machine.

## Evidence basis (P0F, 2026-08-22, native COSMIC host)

Rows below marked *P0F* are graded from the probe evidence in
`agent/evidence/P0F/` (Pop!_OS 24.04, COSMIC Wayland, xdg-desktop-portal
1.18.4, PipeWire 1.5.85, WebKitGTK 2.52.3), bound to git HEAD at generation
time. The GNOME lane is **post-release** (owner decision 2026-09-28: the first
release is Pop!_OS COSMIC only). Its column is kept as planning input for
side quest SQ1 and is not a 1.0 claim.

| Capability | COSMIC Wayland (1.0) | GNOME Wayland (post-release) | KDE/wlroots | Windows | macOS |
|---|---|---|---|---|---|
| Global toggle activation (portal GlobalShortcuts) | experimental — *P0F*: host portal exposes **no GlobalShortcuts interface** (`cosmic`/`gtk` impls, portal 1.18.4); activation state cycling is leak/hang-free (50/50 synthetic cycles, 0.511 s); restart path present; permission store present but prompt flow untestable without the interface → a non-portal trigger route must be qualified at P2 | deferred to P11 (lane absent, B-HW-003) | roadmap | future | future |
| Hold-to-talk / PTT key-release semantics | unavailable/unqualified — *P0F*: no proven key-release route on this host portal; stays capability-dependent per locked decision (toggle is default) | deferred to P11 | roadmap | future | future |
| Active app/window metadata | limited — *P0F*: cosmic-comp introspection works but exposes no window API; generic Wayland window identity unavailable; XWayland window identity available via X properties; fallback semantics required | deferred to P11 | roadmap | future | future |
| Selection/context discovery | unavailable on host class — *P0F*: no wl-clipboard/xclip/libei/AT-SPI text-input route; selection and surrounding-text discovery unavailable; re-examined at P8 only with new evidence | unqualified P8 | roadmap | future | future |
| Text insertion router (semantic→clipboard→wtype→Inbox) | supported-with-fallback design — *P0F*: `wtype` (zwp_virtual_keyboard) present and proven mechanism; semantic/libei/AT-SPI unavailable; **wl-copy/wl-paste/xclip absent on this host** (transactional-clipboard route needs install or explicit handling at P2); Inbox fallback always available; Unicode fixture round-trip true | P2 proven target | roadmap | future | future |
| Least-privilege delivery policy | proven — *P0F*: 5-rank privilege ladder documented (portal/compositor semantic → a11y → clipboard transaction → wtype/libei virtual keyboard → uinput opt-in only); `/dev/uinput` present and user in `input` group but **never default**; no broad input privilege used by probes | same policy at P11 | roadmap | future | future |
| Non-focus-stealing overlay | unqualified P2/P9 (COSMIC layer-shell lessons encoded in docs/product/ux/overlay.md) | unqualified P2/P9 | roadmap | future | future |
| Microphone capture (PipeWire) | basic-proven — *P0F*: PipeWire 1.5.85; 10 sources discovered (default: Samson Meteor Mic USB); open/start/stop verified (~90 kB captured, file deleted after); hotplug monitor possible; non-blocking capture verified; P4 hardens lifecycle/suspend-resume | P4 hardened target | roadmap | future | future |
| Audio policy/restoration | unqualified P4/P9 | unqualified P4/P9 | roadmap | future | future |
| Desktop shell build boundary (Tauri/WebKitGTK) | builder-proven — *P0F*: webkit2gtk-4.1/JSC 2.52.3 + libsoup 3.4.4 present; tauri-cli 2.11.4; workspace `cargo check` green; webkit link flags resolve (gtk3 linked via webkit); GUI launch intentionally deferred to P3 client shell; dpkg tooling present for P10 .deb lane | builder-parity expected at P11 qualification | roadmap | future | future |
| Secret store | unqualified P8/P9 | unqualified P8/P9 | roadmap | future | future |

## Known gaps carried into implementation phases

1. **Trigger route on COSMIC**: portal GlobalShortcuts absent → P2 must qualify an
   alternate route (compositor-specific binding, X11 fallback lane per catalog
   `linux-x11-hotkey`, or libei where available) without defaulting to uinput.
2. **wl-clipboard absent** on the probe host: the transactional-clipboard rung of
   the delivery ladder is currently unproven in practice; install/handle at P2 and
   re-run the delivery probe before claiming the clipboard backend.
3. **GNOME lane**: post-release (SQ1). No GNOME evidence exists and none is
   required for 1.0.

See `linux-global-shortcut-matrix.md` and `linux-text-delivery-matrix.md` for Linux qualification contracts.
