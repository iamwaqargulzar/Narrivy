# P0F Probes — Disposable Linux Feasibility Spikes (P0F)

These probes are **disposable** and must not become polished product code. They exist to establish what is genuinely possible on Pop!_OS/COSMIC Wayland and Ubuntu/GNOME Wayland before architecture hardens.

> **Salvage note (2026-08-22):** probe logic was salvaged from the v0.4 archive
> (`code/narrivy-v0.4/tools/p0f-probes/`) and adapted to the v1.3 evidence contract:
> scenario hashes are taken from `spec/scenarios/<id>.json`, environment facts
> are detected (never hardcoded), and desktop-lane mismatches fail truthfully
> instead of being simulated. Shared helpers live in `p0f_common.py`.

## Probes

- `probe-activation.py` — GlobalShortcuts portal activation/toggle/PTT truth, repeated cycles (50), restart path, permission store. Requires `--scenario p0f-activation-cosmic` or `p0f-activation-gnome`.
- `probe-delivery.py` — Delivery backend/tool truth (semantic, wtype, wl-clipboard, libei, AT-SPI, uinput presence), Unicode/multiline/5kB fixtures, installed target inventory, per-backend capability matrix.
- `probe-least-privilege.py` — Privilege ladder analysis + host truth (uinput presence/mode, input-group, libei); satisfies owner gate P0F-004 (`VER-P0F-LEAST-PRIV`).
- `probe-target-identity.py` — App/window identity, selection/surrounding-text discovery, permission checks, fallback semantics.
- `probe-mic.py` — PipeWire device discovery, open/start/stop (~1s capture, deleted after), hotplug-monitor basics, suspend/resume infrastructure, non-blocking check.
- `probe-tauri.py` — Tauri/WebKitGTK builder deps, toolchain, workspace compile viability, webkit link flags, packaging tooling (GUI launch intentionally deferred to P3).

## Result semantics (fail-closed)

- `result: pass` — the probe **ran its real checks on the matching desktop lane** and recorded the truth. Truthfully-unavailable capabilities (e.g. portal GlobalShortcuts absent) still yield `pass` with explicit `metrics` booleans; the capability matrix and scope decisions consume those metrics.
- `result: fail` — the probe could not run truthfully (wrong lane, missing tools). The GNOME scenarios fail on a COSMIC host by design; they must be run on a native GNOME Wayland session (see `agent/BLOCKERS.md` B-HW-003).
- Evidence is bound to the git commit at run time. Order of operations for gate-valid evidence: commit probe/source changes first, then run probes, then run the phase gate (the gate compares each evidence `git_commit` to current HEAD).

## Running

```bash
# COSMIC lane (this host):
python3 tools/p0f-probes/probe-activation.py --scenario p0f-activation-cosmic
python3 tools/p0f-probes/probe-delivery.py
python3 tools/p0f-probes/probe-least-privilege.py
python3 tools/p0f-probes/probe-target-identity.py
python3 tools/p0f-probes/probe-mic.py
python3 tools/p0f-probes/probe-tauri.py

# GNOME lane (must run on a native Ubuntu/GNOME Wayland machine):
python3 tools/p0f-probes/probe-activation.py --scenario p0f-activation-gnome
```

Evidence lands under `agent/evidence/P0F/` and validates against
`spec/schemas/evidence.schema.json`. Update
`docs/platforms/capability-matrix.md` only from evidence, not assumptions.
See `docs/maintainers/phases/phase-0F-feasibility.md` for the decision rule.
