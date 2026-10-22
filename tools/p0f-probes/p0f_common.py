#!/usr/bin/env python3
"""Shared helpers for P0F disposable probes.

Salvaged from the v0.4 archive (old/tools/p0f-probes) and adapted to the v1.3
evidence contract: scenario hashes come from spec/scenarios/<id>.json, hardware
facts are detected or null (never hardcoded), and desktop-lane mismatches fail
truthfully instead of being simulated.
"""
import datetime
import hashlib
import json
import os
import platform
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_PATH = ROOT / "spec/schemas/evidence.schema.json"


def run(cmd, timeout=5):
    """Run a command, returning (returncode, stdout, stderr); never raises."""
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return r.returncode, r.stdout or "", r.stderr or ""
    except Exception as exc:  # missing binary, timeout, ...
        return 127, "", str(exc)


def git_commit():
    rc, out, _ = run(["git", "rev-parse", "HEAD"], timeout=3)
    return out.strip() if rc == 0 else "unknown"


def git_dirty():
    rc, out, _ = run(["git", "status", "--porcelain"], timeout=3)
    return bool(out.strip()) if rc == 0 else False


def _os_release():
    try:
        return platform.freedesktop_os_release()
    except Exception:
        return {}


def _detect_cpu():
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    except Exception:
        pass
    return platform.processor() or platform.machine() or None


def _detect_gpu():
    # Best-effort via vulkaninfo summary; null when unavailable. Detection is
    # diagnostic metadata only — never a capability claim.
    rc, out, _ = run(["vulkaninfo", "--summary"], timeout=10)
    if rc == 0:
        for line in out.splitlines():
            line = line.strip()
            if line.startswith("deviceName"):
                return line.split("=", 1)[-1].strip()
    return None


def _detect_npu():
    base = Path("/sys/class/accel")
    try:
        accel = sorted(p.name for p in base.iterdir() if p.name.startswith("accel"))
    except Exception:
        return None
    if not accel:
        return None
    drivers = []
    for name in accel:
        try:
            driver = (base / name / "device/driver").resolve().name
        except Exception:
            driver = "unknown"
        drivers.append(f"/dev/{name} ({driver})")
    return "; ".join(drivers)


def _detect_microphone():
    rc, out, _ = run(["pactl", "get-default-source"], timeout=3)
    return out.strip() if rc == 0 and out.strip() else None


def host_desktop():
    raw = os.getenv("XDG_CURRENT_DESKTOP", "")
    # XDG_CURRENT_DESKTOP may be colon-separated (e.g. "pop:GNOME"); the last
    # entry is the running session's desktop.
    return raw.split(":")[-1].strip().upper() or "UNKNOWN"


def detect_env(env_id=None):
    rel = _os_release()
    os_pretty = rel.get("PRETTY_NAME") or platform.release()
    native = "vm" if os.path.exists("/run/.containerenv") or os.path.exists("/.dockerenv") else "native"
    desktop = host_desktop()
    return {
        "id": env_id or f"{desktop.lower()}-{native}",
        "native_or_vm": native,
        "os": rel.get("NAME") or platform.system(),
        "os_version": os_pretty,
        "kernel": platform.release(),
        "desktop": desktop,
        "session": os.getenv("XDG_SESSION_TYPE", "unknown"),
        "cpu": _detect_cpu(),
        "gpu": _detect_gpu(),
        "npu": _detect_npu(),
        "microphone": _detect_microphone(),
    }


def scenario_spec(scenario_id):
    """Return (spec_dict, sha256_of_spec_file) or (None, None) when missing."""
    path = ROOT / "spec/scenarios" / f"{scenario_id}.json"
    if not path.is_file():
        return None, None
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    try:
        return json.loads(path.read_text()), digest
    except Exception:
        return None, digest


def lane_mismatch(spec):
    """Return a truthful explanation when the scenario's desktop lane does not
    match the running session, else None. GNOME evidence may not be inferred
    from a COSMIC host (docs/roadmap/platforms/gnome.md)."""
    want = (spec or {}).get("desktop", "").upper()
    have = host_desktop()
    if want and want != have:
        return (
            f"scenario requires desktop lane {want} but this session is {have}; "
            "cross-lane simulation is forbidden — run this probe on a native "
            f"{want} Wayland session (see agent/BLOCKERS.md B-HW-003 for the "
            "deferred GNOME lane)"
        )
    return None


def write_evidence(
    *,
    verification_id,
    scenario_id,
    out_rel,
    started_at,
    metrics,
    notes,
    diagnostic_excerpt,
    result="pass",
    exit_code=0,
    scenario_hash=None,
):
    """Assemble, schema-validate and write one evidence record.

    `result: pass` means the probe genuinely executed its checks on the correct
    lane and recorded the truth — including truthfully-unavailable capabilities
    in `metrics`. Anything else must use result fail/skip with a non-zero exit
    so the fail-closed gate can never read it as qualified evidence.
    """
    ended = datetime.datetime.now(datetime.timezone.utc).isoformat()
    commit = git_commit()
    ev = {
        "schema_version": 1,
        "verification_id": verification_id,
        "requirement_ids": [],  # P0F is capability-gating; no gated FUN IDs
        "phase": "P0F",
        "scenario_id": scenario_id,
        "git_commit": commit,
        "git_dirty": git_dirty(),
        "build_id": f"p0f-{scenario_id}-{commit[:7] if commit != 'unknown' else 'unknown'}",
        "environment": detect_env(),
        "command": "python3 " + " ".join(sys.argv),
        "scenario_hash": scenario_hash,
        "started_at": started_at,
        "ended_at": ended,
        "exit_code": exit_code,
        "result": result,
        "pass": result == "pass",
        "metrics": metrics,
        "artifact_hashes": {},
        "diagnostic_ref": None,
        "diagnostic_excerpt": diagnostic_excerpt[:3900],
        "tester": "autonomous-agent",
        "notes": " ".join(notes)[:3900] if isinstance(notes, list) else notes,
    }
    out = ROOT / out_rel if not str(out_rel).startswith("/") else Path(out_rel)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(ev, indent=2) + "\n")
    try:
        import jsonschema
        jsonschema.validate(instance=ev, schema=json.loads(SCHEMA_PATH.read_text()))
        print(f"Wrote {out.relative_to(ROOT)} result={result} (schema OK)")
    except ImportError:
        print(f"Wrote {out.relative_to(ROOT)} result={result} (jsonschema not installed; NOT validated)")
    except Exception as exc:
        print(f"  schema FAIL: {exc}", file=sys.stderr)
        sys.exit(1)
    return exit_code
