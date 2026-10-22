# Phase Report — P0F Linux Platform Feasibility

**Status:** failed — superseded by Recovery Phase 1 evidence quarantine.
**Started:** 2026-08-22 (session resume)
**Completed:** 2026-08-22

> Authoritative machine-readable record: `agent/phase-reports/P0F.json`
> (validated against `spec/schemas/phase-report.schema.json`; this markdown file
> is the human-readable companion kept because pack validation requires a
> per-phase `.md`).

## Summary
P0F completed on COSMIC-native evidence per the amended owner gate (owner
decision 2026-08-22: GNOME lane deferred to P11 with recorded consent; never
simulated). Six real probes ran on Pop!_OS 24.04 / COSMIC / Wayland and pass
truthfully; the GNOME activation scenario fails off-lane (exit 3) by design.

## Evidence
- `agent/evidence/P0F/p0f-activation-cosmic.json` — portal GlobalShortcuts
  interface ABSENT (portal 1.18.4); toggle-via-portal unqualified; 50/50
  activation cycles clean; restart path + permission store present.
- `agent/evidence/P0F/p0f-delivery-matrix.json` — wtype present; semantic /
  libei / AT-SPI absent; wl-clipboard NOT installed; Unicode round-trip true;
  Inbox fallback always available.
- `agent/evidence/P0F/p0f-least-privilege.json` — privilege ladder; uinput
  present but opt-in-only, never default.
- `agent/evidence/P0F/p0f-target-identity.json` — cosmic-comp introspection
  works, no window API; XWayland identity available via X properties;
  selection/surrounding-text unavailable → fallback semantics required.
- `agent/evidence/P0F/p0f-mic-pipewire.json` — PipeWire 1.5.85; discovery,
  open/start/stop (~90 kB probe capture deleted after), hotplug monitor,
  non-blocking verified.
- `agent/evidence/P0F/p0f-tauri-build.json` — WebKitGTK 2.52.3 links,
  workspace cargo check green, tauri-cli 2.11.4; GUI launch deferred to P3.
- All bound to git HEAD at generation; regenerated via
  `python3 scripts/run_gate.py --phase P0F`.

## Measurements
See `metrics` in `P0F.json` (activation cycles, portal version, delivery
backend availability, mic capture bytes, WebKitGTK/tauri-cli versions).

## Deviations
1. GNOME lane not probed — deferred to P11 by owner decision (recorded in
   `agent/DECISIONS.md`, B-HW-003).
2. wl-clipboard/xclip absent on host — transactional-clipboard rung must be
   installed/handled and re-proven at P2 before claiming that backend.
3. pkg-config `gtk-3.0` module-name quirk recorded (webkit link flags are
   authoritative: gtk3 links correctly).
4. Tauri GUI launch intentionally deferred to P3 (builder/link boundary only).

## Gate
`python3 scripts/run_gate.py --phase P0F` → PASSED (exit 0, commit 845f481,
dirty=True per evidence-ahead convention); state advanced to P1.
