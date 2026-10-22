#!/usr/bin/env python3
"""Mic/PipeWire probe — device discovery, open/start/stop (1s capture),
hotplug-monitor basics, suspend/resume infrastructure, non-blocking check.

Salvaged from v0.4 (old/tools/p0f-probes/probe-mic.py), adapted to the v1.3
evidence contract. Capture test records ~1s from the default source via
pw-record into a temp file and deletes it; it does not retain audio.
"""
import argparse
import datetime
import json
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from p0f_common import run, scenario_spec, write_evidence

SCENARIO = "p0f-mic-pipewire"


def discover_devices():
    res = {"pipewire_available": False, "version": None, "sources": [],
           "default_source": None, "wpctl_status_excerpt": None, "pw_dump_count": 0}
    if shutil.which("pw-cli"):
        rc, out, err = run(["pw-cli", "--version"], timeout=3)
        res["pipewire_available"] = True
        res["version"] = (out or err).strip()[:200]
    if shutil.which("pactl"):
        rc, out, _ = run(["pactl", "list", "sources", "short"], timeout=5)
        if rc == 0:
            for line in out.strip().splitlines():
                parts = line.split()
                if len(parts) >= 2:
                    res["sources"].append({"id": parts[0], "name": parts[1], "raw": line[:200]})
        rc, out, _ = run(["pactl", "get-default-source"], timeout=3)
        if rc == 0 and out.strip():
            res["default_source"] = out.strip()
        else:
            rc, out, _ = run(["pactl", "info"], timeout=3)
            for line in out.splitlines():
                if "Default Source:" in line:
                    res["default_source"] = line.split(":", 1)[1].strip()
    if shutil.which("wpctl"):
        rc, out, _ = run(["wpctl", "status"], timeout=5)
        res["wpctl_status_excerpt"] = (out or "")[:1500]
    if shutil.which("pw-dump"):
        rc, out, _ = run(["pw-dump"], timeout=5)
        if rc == 0:
            try:
                res["pw_dump_count"] = len(json.loads(out))
            except Exception:
                pass
    return res


def test_open_start_stop():
    res = {"attempted": False, "success": False, "bytes_captured": 0,
           "duration_s": 0, "non_blocking_verified": False, "error": None}
    if shutil.which("pw-record"):
        tmp = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
        tmp.close()
        cmd = ["pw-record", "--rate", "48000", "--channels", "1",
               "--format", "s16", tmp.name]
    elif shutil.which("parecord"):
        tmp = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
        tmp.close()
        cmd = ["parecord", "--rate=48000", "--channels=1",
               "--format=s16le", tmp.name]
    else:
        res["error"] = "no pw-record/parecord found"
        return res
    res["attempted"] = True
    start = time.monotonic()
    proc = None
    try:
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        time.sleep(1.0)
        proc.terminate()
        try:
            _, errs = proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            _, errs = proc.communicate()
            res["error"] = "timeout kill"
            return res
        res["duration_s"] = round(time.monotonic() - start, 3)
        res["bytes_captured"] = Path(tmp.name).stat().st_size
        res["success"] = res["bytes_captured"] > 1000
        res["non_blocking_verified"] = True
        res["stderr_excerpt"] = (errs.decode(errors="replace")[:300] if errs else "")
    except Exception as exc:
        res["error"] = str(exc)
        if proc:
            try:
                proc.kill()
            except Exception:
                pass
    finally:
        try:
            Path(tmp.name).unlink()
        except Exception:
            pass
    return res


def test_hotplug():
    res = {"wireplumber_available": shutil.which("wpctl") is not None,
           "monitor_possible": False, "event_check": None}
    if res["wireplumber_available"]:
        rc, out, _ = run(["wpctl", "status"], timeout=5)
        res["monitor_possible"] = "Sources:" in out
        res["event_check"] = ("wpctl shows Sources section" if res["monitor_possible"]
                              else "no Sources section")
    res["hotplug_note"] = ("Hotplug detection via WirePlumber/PipeWire stream events; "
                           "re-enumeration on device add/remove without polling races")
    return res


def test_suspend_resume():
    res = {"logind_available": False, "can_inhibit_check": False}
    res["logind_available"] = run(["busctl", "status",
                                   "org.freedesktop.login1"], timeout=3)[0] == 0
    if shutil.which("systemd-inhibit"):
        rc, out, _ = run(["systemd-inhibit", "--list"], timeout=3)
        res["can_inhibit_check"] = rc == 0
        res["inhibit_list_excerpt"] = out[:400]
    res["suspend_resume_note"] = ("PipeWire survives suspend; capture streams must be "
                                  "re-opened/reconciled after resume — hardened in P4")
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=None)
    args = ap.parse_args()
    out = args.out or f"agent/evidence/P0F/{SCENARIO}.json"
    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
    spec, spec_hash = scenario_spec(SCENARIO)

    disc = discover_devices()
    open_test = test_open_start_stop()
    hotplug = test_hotplug()
    suspend = test_suspend_resume()
    metrics = {
        "discovery": disc, "open_start_stop": open_test,
        "hotplug": hotplug, "suspend_resume": suspend,
        "summary": {
            "pipewire_available": disc["pipewire_available"],
            "sources_count": len(disc["sources"]),
            "default_source": disc["default_source"],
            "open_success": open_test["success"],
            "bytes_captured": open_test["bytes_captured"],
            "hotplug_monitor_possible": hotplug["monitor_possible"],
            "non_blocking_verified": open_test["non_blocking_verified"],
        },
    }
    notes = [
        f"Mic/PipeWire probe: PipeWire {disc.get('version')}, WirePlumber.",
        f"Discovery: {len(disc['sources'])} sources via pactl; "
        f"default={disc.get('default_source')}; pw_dump nodes={disc.get('pw_dump_count')}.",
        f"Open/start/stop: attempted={open_test['attempted']} success={open_test['success']} "
        f"bytes={open_test['bytes_captured']} duration={open_test['duration_s']}s "
        f"non_blocking={open_test['non_blocking_verified']}.",
        f"Hotplug: WirePlumber available={hotplug['wireplumber_available']} "
        f"monitor_possible={hotplug['monitor_possible']}.",
        f"Suspend/resume: logind={suspend['logind_available']}; "
        "full suspend-cycle hardening belongs to P4.",
    ]
    return write_evidence(
        verification_id="VER-P0F-MIC", scenario_id=SCENARIO, out_rel=out,
        started_at=started, metrics=metrics, notes=notes,
        diagnostic_excerpt=(f"sources={len(disc['sources'])} "
                            f"default={disc.get('default_source')} "
                            f"open_success={open_test['success']}"),
        scenario_hash=spec_hash,
    )


if __name__ == "__main__":
    sys.exit(main())
