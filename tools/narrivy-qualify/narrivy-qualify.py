#!/usr/bin/env python3
"""
narrivy-qualify — Standard scenario runner per the legacy planning amendment Part F.

Lists named scenarios, runs automatable portions, guides manual/native steps,
captures sanitized environment metadata, hashes scenario definition, binds results
to Git/build/package identity, and emits schema-valid JSON evidence.

Usage:
  python3 tools/narrivy-qualify/narrivy-qualify.py list
  python3 tools/narrivy-qualify/narrivy-qualify.py run <scenario_id> [--json] [--out <path>]
  python3 tools/narrivy-qualify/narrivy-qualify.py verify <evidence.json>
"""
from __future__ import annotations
import argparse, json, hashlib, platform, subprocess, datetime, os, sys, glob
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCENARIOS_DIR = ROOT / "spec/scenarios"
EVIDENCE_SCHEMA = ROOT / "spec/schemas/evidence.schema.json"

def git_commit():
    try:
        r=subprocess.run(["git","rev-parse","HEAD"], cwd=ROOT, capture_output=True, text=True, timeout=5)
        return r.stdout.strip() if r.returncode==0 else "unknown"
    except Exception:
        return "unknown"

def git_dirty():
    try:
        r=subprocess.run(["git","status","--porcelain"], cwd=ROOT, capture_output=True, text=True, timeout=5)
        return bool(r.stdout.strip())
    except Exception:
        return False

def env_info():
    return {
        "id": os.getenv("NARRIVY_ENV_ID","local"),
        "native_or_vm": "vm" if os.path.exists("/run/.containerenv") or os.path.exists("/.dockerenv") else "native",
        "os": platform.system(),
        "os_version": platform.freedesktop_os_release().get("PRETTY_NAME", platform.release()) if hasattr(platform,"freedesktop_os_release") else platform.release(),
        "kernel": platform.release(),
        "desktop": os.getenv("XDG_CURRENT_DESKTOP","unknown"),
        "session": os.getenv("XDG_SESSION_TYPE","unknown"),
        "cpu": platform.processor() or "unknown",
        "microphone": os.getenv("NARRIVY_MIC","stub"),
    }

def list_scenarios():
    for p in sorted(SCENARIOS_DIR.glob("*.json")):
        data=json.loads(p.read_text())
        print(f"{data.get('id')}: {p.name} — phase {data.get('phase')} kind {data.get('kind')}")

PROBE_DISPATCH = {
    "p0f-activation-cosmic": [sys.executable, "tools/p0f-probes/probe-activation.py", "--scenario", "p0f-activation-cosmic"],
    "p0f-activation-gnome":  [sys.executable, "tools/p0f-probes/probe-activation.py", "--scenario", "p0f-activation-gnome"],
    "p0f-delivery-matrix":   [sys.executable, "tools/p0f-probes/probe-delivery.py"],
    "p0f-target-identity":   [sys.executable, "tools/p0f-probes/probe-target-identity.py"],
    "p0f-mic-pipewire":      [sys.executable, "tools/p0f-probes/probe-mic.py"],
    "p0f-tauri-build":       [sys.executable, "tools/p0f-probes/probe-tauri.py"],
    # P2 walking skeleton: real daemon binary + real CLI over a real socket
    # (deterministic asr-mock contract; GNOME lane fails truthfully off-lane).
    "p2-trigger-cosmic":     [sys.executable, "tools/narrivy-qualify/p2_runner.py", "--scenario", "p2-trigger-cosmic"],
    "p2-trigger-gnome":      [sys.executable, "tools/narrivy-qualify/p2_runner.py", "--scenario", "p2-trigger-gnome"],
    # P2 real-path scenarios (owner gate P2-003..P2-007)
    "p2-e2e-native-wayland": [sys.executable, "tools/narrivy-qualify/p2_runner.py", "--e2e-native-wayland"],
    "p2-inbox-fallback":     [sys.executable, "tools/narrivy-qualify/p2_runner.py", "--inbox-fallback"],
    "p2-cancel-race":        [sys.executable, "tools/narrivy-qualify/p2_runner.py", "--cancel-race"],
    "p2-cpu-fallback":       [sys.executable, "tools/narrivy-qualify/p2_runner.py", "--cpu-fallback"],
}


def run_scenario(sid, json_out_only=False, out_path=None):
    """Run a scenario through its real probe, or fail closed.

    This tool must NEVER fabricate a pass. If a scenario has no automated
    runner, it writes truthful `skip` evidence and exits non-zero so no gate
    can mistake it for qualification (master prompt: no pass-shaped stubs).
    """
    scenario_file = None
    for p in SCENARIOS_DIR.glob("*.json"):
        try:
            if json.loads(p.read_text()).get("id") == sid:
                scenario_file = p
                break
        except Exception:
            continue
    if not scenario_file:
        print(f"Scenario {sid} not found in {SCENARIOS_DIR}", file=sys.stderr)
        sys.exit(2)
    scenario = json.loads(scenario_file.read_text())
    scenario_hash = hashlib.sha256(scenario_file.read_bytes()).hexdigest()
    cmd = PROBE_DISPATCH.get(sid)
    if cmd is None:
        # Fail closed: truthful skip evidence, non-zero exit.
        started = datetime.datetime.now(datetime.timezone.utc).isoformat()
        evidence = {
            "schema_version": 1,
            "verification_id": scenario.get("verification_id", "VER-TEST-001"),
            "requirement_ids": [],
            "phase": scenario.get("phase", "P0"),
            "scenario_id": sid,
            "git_commit": git_commit(),
            "git_dirty": git_dirty(),
            "build_id": f"qual-skip-{git_commit()[:7]}",
            "environment": env_info(),
            "command": f"narrivy-qualify run {sid}",
            "scenario_hash": scenario_hash,
            "started_at": started,
            "ended_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "exit_code": 3,
            "result": "skip",
            "pass": False,
            "artifact_hashes": {},
            "notes": "No automated runner implemented for this scenario yet — implement the real probe before any capability claim. Fail-closed by design.",
        }
        out = Path(out_path) if out_path else ROOT / f"agent/evidence/{evidence['phase']}/{sid}.json"
        if not out.is_absolute():
            out = ROOT / out
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(evidence, indent=2) + "\n")
        print(f"No runner for {sid}; wrote truthful skip evidence and exiting non-zero (fail-closed).", file=sys.stderr)
        sys.exit(3)
    print(f"Dispatching real probe for {sid}: {' '.join(cmd)}")
    proc = subprocess.run(cmd, cwd=ROOT)
    sys.exit(proc.returncode)


def verify_evidence(ev_path):
    p = Path(ev_path)
    if not p.is_file():
        # Try relative to ROOT
        p = ROOT / ev_path
    if not p.is_file():
        print(f"Evidence file not found: {ev_path}", file=sys.stderr)
        sys.exit(2)
    data=json.loads(p.read_text())
    try:
        import jsonschema
        schema=json.loads(EVIDENCE_SCHEMA.read_text())
        jsonschema.validate(instance=data, schema=schema)
        print(f"{p}: schema OK, result {data.get('result')}, phase {data.get('phase')}, scenario {data.get('scenario_id')}")
    except ImportError:
        print(f"{p}: jsonschema not installed, JSON parse OK")
        print(json.dumps(data, indent=2))
    except Exception as e:
        print(f"{p}: schema FAIL: {e}", file=sys.stderr)
        sys.exit(1)

def main():
    ap=argparse.ArgumentParser(prog="narrivy-qualify")
    sub=ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list", help="list scenarios")
    run= sub.add_parser("run", help="run scenario")
    run.add_argument("scenario_id")
    run.add_argument("--json", action="store_true", help="emit JSON to stdout")
    run.add_argument("--out", help="output evidence path")
    ver= sub.add_parser("verify", help="verify evidence")
    ver.add_argument("evidence")
    args=ap.parse_args()
    if args.cmd=="list":
        list_scenarios()
    elif args.cmd=="run":
        run_scenario(args.scenario_id, json_out_only=args.json, out_path=args.out)
    elif args.cmd=="verify":
        verify_evidence(args.evidence)

if __name__=="__main__":
    main()
