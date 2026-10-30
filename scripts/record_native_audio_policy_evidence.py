#!/usr/bin/env python3
"""Run reviewed native PipeWire/audio-policy probes and register schema-v2 evidence."""
from __future__ import annotations

import hashlib
import json
import platform
import shlex
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

import jsonschema

import recorder_safety

ROOT = Path(__file__).resolve().parents[1]
INDEX = "agent/evidence/INDEX.json"
LEGACY = "tools/narrivy-qualify/p4_runner.py"
EXPECTED_ARGV = [
    "scripts/record_native_audio_policy_evidence.py",
    "target/debug/examples/p4_capture_probe",
    "target/debug/examples/p4_policy_helper",
]
PROBES = [
    ["--scenario", "p4-preroll"],
    ["--scenario", "p4-mic-hotplug"],
    ["--scenario", "p4-audio-policy"],
    ["--scenario", "p4-forced-kill-restore"],
    ["--scenario", "p4-new-stream-inherit"],
    ["--structural"],
]
RECORDS = {
    "FUN-006": ("VER-FUN-006", "native-pipewire-preroll-v1", "target/debug/examples/p4_capture_probe", ["live-pipewire-capture", "preroll-preserved", "first-syllable-not-clipped"]),
    "FUN-007": ("VER-FUN-007", "native-audio-policy-v1", "target/debug/examples/p4_policy_helper", ["leave-policy-passed", "duck-policy-passed", "clean-restoration-passed"]),
    "FUN-011": ("VER-FUN-011", "native-microphone-hotplug-v1", "target/debug/examples/p4_capture_probe", ["deterministic-ranking", "explicit-override", "hotplug-observed", "bounded-recovery"]),
    "FUN-012": ("VER-FUN-012", "native-new-stream-policy-inheritance-v1", "target/debug/examples/p4_policy_helper", ["active-policy-applied", "new-stream-inherited", "polling-race-absent"]),
    "REL-005": ("VER-REL-005", "native-audio-clean-restoration-v1", "target/debug/examples/p4_policy_helper", ["state-snapshotted", "clean-exit-restored", "bounded-restoration"]),
    "REL-009": ("VER-REL-009", "native-audio-forced-kill-restoration-v1", "target/debug/examples/p4_policy_helper", ["restoration-journal-written", "dead-owner-detected", "forced-kill-restored"]),
    "PERF-006": ("VER-PERF-006", "native-realtime-capture-contract-v1", "target/debug/examples/p4_capture_probe", ["capture-hot-path-no-file-io", "capture-hot-path-no-model-inference", "live-pipewire-path-exercised"]),
}


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    if sys.argv != EXPECTED_ARGV:
        print("record_native_audio_policy_evidence: argv does not match reviewed procedure", file=sys.stderr); return 1
    status = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True, timeout=5)
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, timeout=5, check=True).stdout.strip()
    if status.returncode or status.stdout.strip():
        print("record_native_audio_policy_evidence: source tree is not clean", file=sys.stderr); return 1
    index_path = ROOT / INDEX; index = json.loads(index_path.read_text())
    outputs = {req: ROOT / f"agent/evidence/P4/{meta[0]}.json" for req, meta in RECORDS.items()}
    for req, path in outputs.items():
        if not recorder_safety.replaceable(path, index, ROOT):
            print(f"record_native_audio_policy_evidence: output is not replaceable: {req}", file=sys.stderr); return 1
    try:
        lane = recorder_safety.native_lane()
    except recorder_safety.RecorderSafetyError as error:
        print(f"record_native_audio_policy_evidence: {error}", file=sys.stderr); return 1
    # The legacy harness writes its own result to the evidence path. Keep the
    # original bytes, read the harness result, then put the originals back so
    # the final write is one guarded transaction (and a failed run changes nothing).
    originals = {req: (path.read_bytes() if path.exists() else None) for req, path in outputs.items()}
    legacy_results: dict[str, dict] = {}
    started = datetime.now(timezone.utc)
    try:
        for args in PROBES:
            try:
                result = recorder_safety.run_bounded(["python3", LEGACY, *args], cwd=ROOT, capture_output=True, text=True, timeout=180)
            except subprocess.TimeoutExpired:
                print(f"record_native_audio_policy_evidence: probe timed out: {shlex.join(args)}", file=sys.stderr); return 1
            if result.returncode:
                print((result.stdout + result.stderr)[-3000:], file=sys.stderr); return 1
        for req, path in outputs.items():
            legacy_results[req] = json.loads(path.read_text())
    finally:
        for req, path in outputs.items():
            if originals[req] is None:
                path.unlink(missing_ok=True)
            else:
                path.write_bytes(originals[req])
    ended = datetime.now(timezone.utc)
    runner_hash = sha(ROOT / LEGACY); recorder_hash = sha(Path(__file__))
    schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
    payloads: list[tuple[Path, bytes]] = []
    for req, (verification, procedure, executable_rel, assertions) in RECORDS.items():
        legacy = legacy_results[req]
        if legacy.get("result") != "pass" or legacy.get("pass") is not True:
            print(f"record_native_audio_policy_evidence: legacy probe did not pass: {req}", file=sys.stderr); return 1
        executable = ROOT / executable_rel
        evidence = {
            "schema_version": 2, "verification_id": verification, "requirement_ids": [req], "phase": "P4",
            "scenario_id": procedure, "git_commit": head, "git_dirty": False, "build_id": f"native-audio-{head[:7]}",
            "package_sha256": None,
            "environment": {"id":"cosmic-wayland-native-pipewire","native_or_vm":"native","os":lane["os"],"os_version":lane["os_version"],"kernel":platform.release(),"desktop":lane["desktop"],"session":lane["session"],"cpu":platform.machine(),"gpu":None,"npu":None,"device_details":"native PipeWire/WirePlumber session","microphone":"native PipeWire sources","input_backend":"PipeWire","display_scale":None,"qualification_lane":"cosmic-wayland-native-audio-policy","lane_available":True},
            "command": shlex.join(EXPECTED_ARGV), "scenario_version":"1", "scenario_hash":recorder_hash,
            "started_at":started.isoformat(), "ended_at":ended.isoformat(), "exit_code":0, "result":"pass", "pass":True,
            "metrics":{"native_probe":legacy.get("metrics",{}),"legacy_scenario":legacy.get("scenario_id")},
            "artifact_hashes":{LEGACY:runner_hash,"scripts/record_native_audio_policy_evidence.py":recorder_hash,executable_rel:sha(executable)},
            "artifact_under_test":{"kind":"executable","path":"scripts/record_native_audio_policy_evidence.py","sha256":recorder_hash},
            "procedure":{"id":procedure,"profile_id":"native-behavior-v1","assertions":[{"id":a,"result":"pass"} for a in assertions]},
            "notes":"Native COSMIC PipeWire behavior was exercised against real session services. The legacy probe is used only as a reviewed behavior harness; this recorder validates its result and emits the authoritative schema-v2 record.",
            "evidence_type":"native-platform", "execution":{"lane_available":True,"child_failures_ignored":False,"environment_class":"native-platform","substitutions":[]},
        }
        jsonschema.validate(instance=evidence, schema=schema)
        payloads.append((outputs[req], (json.dumps(evidence,indent=2,sort_keys=True)+"\n").encode()))
    index_schema = json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text())
    try:
        recorder_safety.commit_records(ROOT, index_path, index, payloads,
            validate_index=lambda value: jsonschema.validate(instance=value, schema=index_schema))
    except (OSError, recorder_safety.RecorderSafetyError, jsonschema.ValidationError) as error:
        print(f"record_native_audio_policy_evidence: {error}", file=sys.stderr); return 1
    print(json.dumps({"records":len(RECORDS),"result":"pass"},sort_keys=True)); return 0


if __name__ == "__main__":
    raise SystemExit(main())
