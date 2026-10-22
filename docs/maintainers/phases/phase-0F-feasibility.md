# P0F — Linux Platform Feasibility (Blocking)

> **Scope note (2026-09-28, owner):** the first release is qualified on Pop!_OS
> COSMIC only. Mentions below of Ubuntu/GNOME as a release lane, or of AppImage,
> are superseded; see `docs/roadmap/README.md`. This phase spec is kept as the
> historical definition of the phase.

## Outcome
Use small disposable probes/spikes to establish what is genuinely possible on the current target desktops **before** architecture and UI harden. Do not build polished product code merely to pass P0F. Truthful failure/limit results are acceptable.

## Primary requirements
No FUN release-gated requirements directly; P0F is a capability-gating phase whose evidence drives honest capability claims and scope decisions for Linux 1.0. Blocked if probes are missing or dishonest.

## Required feasibility probes (minimum)

### 1. Global activation on Pop!_OS / COSMIC / Wayland
- activation/toggle behavior;
- whether key-release semantics required for true PTT are available;
- repeated activation reliability (≥50 cycles without leak/hang);
- restart behavior;
- suspend/resume behavior where practical;
- permission/onboarding requirements (portal GlobalShortcuts).

### 2. Global activation on Ubuntu / GNOME / Wayland
- Same categories as (1) on the second first-class lane.

### 3. Text delivery on both desktops
Test at least:
- one native Wayland app (e.g., GNOME Text Editor / COSMIC Edit);
- browser (Firefox/Chromium);
- Electron target where practical (VS Code);
- terminal (GNOME Terminal / COSMIC Terminal);
- at least one XWayland target;
with fixtures:
- Unicode (ASCII, accented Latin, emoji);
- non-ASCII scripts (Urdu/Arabic, CJK);
- multiline text;
- punctuation/spacing behavior (no glued words);
- large text (≥5 kB);
- failure/fallback behavior (verify Inbox fallback when delivery unavailable).

Record per-backend class: security/permission model, target compatibility, Unicode capability, multiline capability, layout dependence, focus requirements, verification possibility, clipboard impact, failure detection, fallback path.

### 4. Least-privilege analysis
- portal/compositor/semantic input route where available;
- clipboard transaction route where appropriate;
- emulated-input route only if permissions/security are acceptable;
- Inbox fallback when direct delivery is not safe/reliable.

### 5. No default broad input privilege
- `/dev/uinput`, input-group membership, privileged daemons, or equivalent broad synthetic-input capability **must not** become the default 1.0 onboarding path merely because it is easy.
- If retained, classify as opt-in/advanced unless explicit owner decision approves it as default after threat review (record in ADR).

### 6. Target identity capabilities
- what app/window identity can actually be obtained on each desktop;
- whether selection/surrounding-text discovery exists;
- what permissions are needed;
- explicit fallback semantics when identity is missing/ambiguous.

### 7. Microphone / PipeWire capture feasibility
- device discovery;
- open/start/stop;
- hotplug basics;
- suspend/resume basics;
- non-blocking capture architecture (no inference on real-time thread).

### 8. Tauri / WebKitGTK / package feasibility
- known builder/runtime constraints;
- required system dependencies;
- clean target launch on the named supported OS lanes (Pop!_OS 24.04 COSMIC, Ubuntu 24.04 GNOME).

## Decision rule
Do not fake platform parity. If a feature cannot be proven on one reference desktop, choose one truthful outcome:
- change the 1.0 capability claim to the reliable fallback;
- make the feature capability-dependent rather than universally promised;
- move the feature post-1.0 (R12/R14);
- document a safe advanced backend;
- or record a genuine owner blocker if the product requirement itself must be changed.

Example: if toggle is reliable but key-release events for true PTT are not, expose toggle as supported and PTT only on capability-proven routes unless the owner explicitly decides lack of PTT blocks release.

## Exit evidence
- Disposable probe code under `tools/p0f-probes/` (or similar) with hashes;
- `agent/evidence/P0F/*.json` evidence files validating the evidence schema and bound to Git commit, builder, environment (native vs VM, OS/kernel/desktop), with scenario script hash, timestamps, pass/fail and sanitized diagnostics;
- Capability matrix `docs/platforms/capability-matrix.md` updated from evidence (not assumptions);
- P0F phase report `agent/phase-reports/P0F.md` validates against phase-report schema;
- `python3 scripts/phase_gate.py --phase P0F` exits 0.

## Prohibited shortcuts
Building polished product code to fake feasibility; claiming PTT where only toggle is proven; suppressing missing target identity; assuming portal availability without probing; leaving `/dev/uinput` as silent default.
