# Post-release side quests (NOT part of the first release)

**Owner decision (2026-09-28):** the first release is **Pop!_OS 24.04 COSMIC
(Wayland) only**. Everything in this file waits until that release is out
**and properly tested**. After that, the order is:

1. **SQ1:** other Linux distributions, starting with Ubuntu 24.04 GNOME;
2. **SQ2:** Windows;
3. **SQ3:** macOS.

**SQ-AMD** (AMD accelerator qualification) is independent hardware work. It
can start any time after the Pop!_OS release, when the owner provides the
machine.

**Rules for the agent:**
- Do **not** start any side quest during WP1–WP6, even if you are idle. Don't
  let side-quest work change the frozen release commit.
- Start a side quest only when the owner explicitly says so.
- Each side quest ends with its own qualified release, using the same method
  as WP5/WP6: freeze, record, gate. The next side quest starts only after
  that release is properly tested.
- Before starting one, re-read `00-review-findings.md` and the owner's latest
  decisions. Several items below were recorded there first.

What already moved out of the release on 2026-09-28 (see `agent/DECISIONS.md`):
- `PORT-001` (portable crates compile on Windows/macOS) moved to roadmap phase
  R13.
- The Windows named-pipe clause of `PORT-004` moved into `PORT-007` (R13).
- `B-HW-003` (Ubuntu/GNOME reference machine) and `B-HW-004` (AMD 8700G NPU)
  are post-release.
- `release_gate.py`, `TEST-010`, `BUILD_TARGET.json`,
  `docs/platforms/bounded-support.md` and `PRODUCT_CHARTER.md` require only
  the Pop!_OS COSMIC lane.

---

## SQ1 — Other Linux distributions (Ubuntu 24.04 GNOME first)

**Entry condition:** the Pop!_OS release is out and the owner confirms it is
properly tested. The owner provides an Ubuntu 24.04 GNOME machine (native, not
a VM, for platform claims).

**Tasks:**
1. **GNOME text insertion (finding F-C3).** `wtype` cannot type on GNOME
   (Mutter lacks `virtual-keyboard-unstable-v1`). Recommended fix: let the
   portal/EIS keyboard rung run without a pre-bound target, using the same
   "type into what is focused at delivery" rule `wtype` has. Gate it on the
   user's attended portal grant. Write an ADR superseding the relevant part of
   ADR-0072. Tests: an unbound target plus a grant runs the rung; no grant
   gives `permission_inactive`; `wtype` never runs after the portal
   dispatched (relies on WP2 T2.1).
2. **WebKitGTK floor (F-M5).** On Ubuntu, run
   `apt-cache policy libwebkitgtk-6.0-4`. Lower the `Depends` floor in
   `scripts/build_deb_installable.py` to the lowest version that supports the
   shell and is in `noble-updates`.
3. **Native smoke on GNOME**, the same checklist as WP3 T3.3, using GNOME
   Settings → Keyboard → Custom Shortcuts for `/usr/bin/narrivy-cli toggle`.
4. **Re-enable the GNOME lane in the release gate:** add `"gnome-wayland"` to
   `REQUIRED_NATIVE_LANES` in `scripts/release_gate.py`. Restore the
   summary check for Ubuntu, amend `TEST-010`, `BUILD_TARGET.json`
   (primary platforms), bounded-support and the charter. Record it in
   `agent/DECISIONS.md`.
5. **Qualification:** run the native recorders on GNOME, including the
   8-class target-app matrix with `environment_kind: gnome-wayland`. Then
   freeze, record and gate exactly as in WP5/WP6.
6. **Further distributions** (KDE Plasma, wlroots, Fedora, Arch,
   Debian-derived) follow one at a time. Each needs a lane definition in
   `bounded-support.md`, a native machine, and the same qualification. Until
   qualified, they may only be described as experimental.

## SQ2 — Windows

**Entry condition:** SQ1's release is properly tested and the owner says to
start.

**Tasks:**
1. **PORT-001, compile proof.** `rustup target add x86_64-pc-windows-gnu`
   (or msvc). Run `cargo check --locked --target <t>` for every portable
   crate; the list is in `scripts/verify-boundaries.py`. Fix any Unix-only
   code that leaked into portable crates. Register a measured procedure.
   Alternatively use the existing Windows job in `.github/workflows/ci.yml`,
   but that needs a push, which is the owner's decision.
2. **PORT-007, native client.** Add a Windows IPC transport (per-user named
   pipe with an explicit ACL; see `docs/architecture/daemon-ipc.md`) behind
   the existing IPC abstraction, plus `adapters/platform-windows` (currently
   empty) for capture, delivery and activation. Keep domain logic in the
   portable crates; do not fork it.
3. Add a Windows lane, packaging (for example MSIX or an installer; owner
   decides) and qualification, following the same freeze/record/gate method.

## SQ3 — macOS

**Entry condition:** SQ2's release is properly tested and the owner says to
start.

**Tasks:**
1. **PORT-001 compile proof** for `aarch64-apple-darwin` (and `x86_64` if
   wanted), as in SQ2.
2. **`adapters/platform-macos`** (currently empty): capture, text delivery
   (Accessibility API), activation, and the IPC transport over Unix sockets.
   Handle permission prompts (microphone, accessibility) with clear UX.
3. Signing and notarization, packaging, lane definition and qualification.

## SQ-AMD — AMD accelerator qualification (B-HW-004)

**Entry condition:** after the Pop!_OS release, when the owner provides the
Ryzen 7 8700G machine (XDNA NPU, RDNA3 780M iGPU).

**Tasks (from `agent/BLOCKERS.md` B-HW-004):**
1. Linux 6.14+ with `amdxdna` and XRT. Record exact versions in
   `agent/RESOLVED_VERSIONS.json`.
2. Qualify an exact tuple (hardware + driver + runtime + model + quant + OS +
   Narrivy version) for the NPU and/or Vulkan iGPU path, with CPU fallback
   still enforced.
3. Only after it passes, add an `accelerator_claims[]` entry with evidence.
   Never claim an accelerator just because it is detected.

## SQ-WRITE — Writing assistant everywhere (owner D26, after the release)

Design: `docs/roadmap/design/writing-assistant.md`.

1. **Rule checker:** local, running on the CPU. Harper is the candidate;
   it goes through the candidate → verified → qualified path.
2. **Native host:**
   - the browser starts it through native messaging, with only
     Narrivy's extension ID allowed;
   - it connects to the daemon as a separate, limited client class that
     may only check and rewrite text (new IPC commands and an
     `ipc_policy` rule);
   - it never opens a local web port.
3. **Extension (Manifest V3, Chrome and Brave first):**
   - underlines while typing;
   - Fix, Improve, Rewrite and Shorter on selected text;
   - a preview before replacing, and undo afterwards.
4. **Privacy:**
   - the extension reads the focused editable field only, and never
     password, payment or one-time-code fields;
   - each site can be switched off, and sensitive categories are off by
     default;
   - there are no network permissions, text is never logged, and it
     enters History only if the user asks.
5. **Language model requests** use the shared model (T2.20 item 5), and
   every result passes the ADR-0018 guard. The model runs on whichever
   accelerator the computer has:
   - OpenVINO (add-on) on Intel;
   - Vulkan by default elsewhere;
   - CUDA (add-on) on NVIDIA;
   - Metal on macOS;
   - CPU as the last resort.

   A ggml-family runtime such as llama.cpp is the candidate for the
   non-OpenVINO paths, to match transcribe.cpp. It is self-tested like
   every other runtime (D21).
6. **Languages:** English by default, and the user picks others. Which
   languages are offered is not decided yet.
7. **Tests:** a policy test that the host cannot reach any other command,
   field-exclusion tests, and a test that no second model load happens.
8. **Global feature:** it ships on every platform Narrivy supports, after
   that platform's own release (SQ1–SQ3).

## Informational: CI matrix

`.github/workflows/ci.yml` has a Windows/macOS job. It is not release
evidence and has never run, because nothing has been pushed. Leave it as is
until SQ2/SQ3.
