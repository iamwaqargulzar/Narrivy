#!/usr/bin/env python3
"""Fail-closed phase gate for Narrivy v1.3.

The gate recomputes acceptance from traceability, owner minima, scenario/evidence
records, commands, phase report, blockers, regression closure and smoke tests.
An implementation phase is never allowed to pass on a status claim alone.
"""
from __future__ import annotations

import argparse
import csv
from datetime import datetime
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path

from evidence_policy import (
    RELEASED_EVIDENCE_SCHEMA_VERSIONS,
    assess_evidence,
    validate_requirement_evidence,
)

ROOT = Path(os.environ.get("NARRIVY_ROOT", Path(__file__).resolve().parents[1])).resolve()


def load_json(path: Path):
    return json.loads(path.read_text())


def git_commit() -> str:
    try:
        r = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, timeout=5)
        return r.stdout.strip() if r.returncode == 0 else "unknown"
    except Exception:
        return "unknown"


def git_dirty() -> bool:
    try:
        r = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True, timeout=5)
        return bool(r.stdout.strip()) if r.returncode == 0 else False
    except Exception:
        return False


def inside_repo(rel: str) -> Path | None:
    try:
        if not rel or Path(rel).is_absolute() or ".." in Path(rel).parts:
            return None
        p = (ROOT / rel).resolve()
        rr = ROOT.resolve()
        return p if p == rr or rr in p.parents else None
    except Exception:
        return None


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def parse_dt(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def evidence_files(phase: str) -> list[Path]:
    base = ROOT / "agent" / "evidence" / phase
    return sorted(base.rglob("*.json")) if base.is_dir() else []


def read_evidence(path: Path) -> tuple[dict | None, list[str]]:
    errors: list[str] = []
    try:
        data = load_json(path)
    except Exception as exc:
        return None, [f"evidence {path.relative_to(ROOT)} invalid JSON: {exc}"]
    try:
        import jsonschema

        schema = load_json(ROOT / "spec/schemas/evidence.schema.json")
        jsonschema.validate(instance=data, schema=schema)
    except ImportError:
        errors.append("jsonschema unavailable while validating evidence")
    except Exception as exc:
        errors.append(f"evidence {path.relative_to(ROOT)} schema fail: {exc}")
    return data, errors


def verify_evidence(
    path: Path,
    *,
    phase: str,
    commit: str,
    verification_id: str | None = None,
    verification_layer: str | None = None,
    requirement_id: str | None = None,
    scenario_id: str | None = None,
    native_required: bool = False,
) -> list[str]:
    errors: list[str] = []
    if not path.is_file():
        return [f"evidence missing: {path}"]

    if path.suffix == ".md":
        return [f"historical Markdown evidence is quarantined and cannot qualify: {path.relative_to(ROOT)}"]

    data, schema_errors = read_evidence(path)
    errors.extend(schema_errors)
    if data is None:
        return errors

    if data.get("phase") != phase:
        errors.append(f"{path.relative_to(ROOT)} phase {data.get('phase')} != {phase}")
    if verification_id and data.get("verification_id") != verification_id:
        errors.append(
            f"{path.relative_to(ROOT)} verification_id {data.get('verification_id')} != {verification_id}"
        )
    if requirement_id and requirement_id not in data.get("requirement_ids", []):
        errors.append(f"{path.relative_to(ROOT)} does not bind requirement {requirement_id}")
    if scenario_id and data.get("scenario_id") != scenario_id:
        errors.append(f"{path.relative_to(ROOT)} scenario_id {data.get('scenario_id')} != {scenario_id}")

    if data.get("schema_version") in RELEASED_EVIDENCE_SCHEMA_VERSIONS:
        if verification_layer is None:
            rows = list(csv.DictReader((ROOT / "spec/requirements-traceability.csv").open(newline="")))
            requirement_ids = data.get("requirement_ids") if isinstance(data.get("requirement_ids"), list) else []
            bound_rows = [
                row
                for row in rows
                if row["verification_id"] == data.get("verification_id")
                and row["requirement_id"] in requirement_ids
            ]
            layers = {row["verification_layer"] for row in bound_rows}
            if len(layers) != 1:
                errors.append(f"{path.relative_to(ROOT)} cannot resolve one verification profile from traceability")
            else:
                verification_layer = layers.pop()
        if verification_layer is not None:
            for trust_error in validate_requirement_evidence(
                data,
                evidence_path=path,
                root=ROOT,
                verification_id=verification_id or str(data.get("verification_id", "")),
                verification_layer=verification_layer,
                current_commit=commit,
            ):
                errors.append(f"{path.relative_to(ROOT)} {trust_error}")
    else:
        for trust_error in assess_evidence(data, current_commit=commit):
            errors.append(f"{path.relative_to(ROOT)} {trust_error}")
    if native_required and data.get("environment", {}).get("native_or_vm") != "native":
        errors.append(f"{path.relative_to(ROOT)} requires native evidence")

    try:
        if parse_dt(data["ended_at"]) < parse_dt(data["started_at"]):
            errors.append(f"{path.relative_to(ROOT)} ended_at precedes started_at")
    except Exception as exc:
        errors.append(f"{path.relative_to(ROOT)} timestamp parse failed: {exc}")

    # Evidence may bind generated artifacts. If it lists a repository path, hash it now.
    for rel, expected in (data.get("artifact_hashes") or {}).items():
        fp = inside_repo(rel)
        if fp is None or not fp.is_file():
            errors.append(f"{path.relative_to(ROOT)} artifact_hash path missing/unsafe: {rel}")
            continue
        got = sha256(fp)
        if got != expected:
            errors.append(f"{path.relative_to(ROOT)} artifact hash mismatch: {rel}")

    return errors


def find_evidence(phase: str, *, verification_id: str | None = None, scenario_id: str | None = None):
    matches: list[Path] = []
    for path in evidence_files(phase):
        try:
            data = load_json(path)
        except Exception:
            continue
        if verification_id and data.get("verification_id") != verification_id:
            continue
        if scenario_id and data.get("scenario_id") != scenario_id:
            continue
        matches.append(path)
    return matches


def scenario_hash(scenario_id: str) -> tuple[str | None, str | None]:
    path = ROOT / "spec" / "scenarios" / f"{scenario_id}.json"
    if not path.is_file():
        return None, f"missing scenario spec spec/scenarios/{scenario_id}.json"
    return sha256(path), None


def blocker_errors(phase: str) -> list[str]:
    path = ROOT / "agent/BLOCKERS.md"
    if not path.is_file():
        return []
    text = path.read_text(errors="replace")
    errors: list[str] = []
    for section in re.split(r"(?=^## B-)", text, flags=re.M):
        if not section.startswith("## B-") or phase not in section:
            continue
        header = section.splitlines()[0]
        pending = any(tag in header.upper() for tag in ("[PENDING", "[OPEN", "[BLOCKED"))
        explicitly_nonblocking = bool(re.search(r"not blocking|non-blocking|deferred", section, re.I))
        explicitly_blocking = bool(re.search(r"blocking now|blocks?\s+" + re.escape(phase), section, re.I))
        if pending and explicitly_blocking and not explicitly_nonblocking:
            errors.append(f"unresolved blocking item for {phase}: {header}")
    return errors


def run_command(raw: str) -> int:
    argv = shlex.split(raw)
    if not argv:
        return 0
    print(f"+ {raw}", flush=True)
    return subprocess.run(argv, cwd=ROOT).returncode


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--phase", required=True)
    args = ap.parse_args()
    phase = args.phase

    registry = load_json(ROOT / "agent/PHASES.json")
    phases = registry["phases"]
    phase_ids = [p["id"] for p in phases]
    if phase not in phase_ids:
        raise SystemExit(f"FAIL: unknown phase {phase}; registry={phase_ids}")
    reg_entry = next(p for p in phases if p["id"] == phase)

    spec_path = ROOT / reg_entry["acceptance"]
    if not spec_path.is_file():
        raise SystemExit(f"FAIL: missing acceptance spec {spec_path.relative_to(ROOT)}")
    spec = load_json(spec_path)
    if spec.get("phase") != phase:
        raise SystemExit(f"FAIL: acceptance phase mismatch in {spec_path.relative_to(ROOT)}")

    print(f"[{phase}] checking owner-gate integrity...")
    rc = subprocess.run([sys.executable, str(ROOT / "scripts/verify_owner_gates.py")], cwd=ROOT).returncode
    if rc:
        raise SystemExit("FAIL: owner-gate integrity failed")

    if not spec.get("implemented", False):
        print(f"FAIL: {phase} is fail-closed until its real acceptance commands/evidence checks are implemented.")
        raise SystemExit(4)

    commit = git_commit()
    dirty = git_dirty()
    errors: list[str] = []

    # Every implemented phase requires provenance; historical P0 material is never evidence.
    if phase != "P0" and commit == "unknown":
        errors.append("implemented phase requires a real Git HEAD; initialize/commit the repository before evidence")

    rows = list(csv.DictReader((ROOT / "spec/requirements-traceability.csv").open(newline="")))
    phase_rows = [r for r in rows if r["primary_phase"] == phase and r["release_gate"] == "required"]
    expected = set(spec.get("primary_requirements", []))
    actual = {r["requirement_id"] for r in phase_rows}
    if expected != actual:
        errors.append(f"acceptance requirement set differs from traceability: traceability={sorted(actual)} spec={sorted(expected)}")

    owner_path = ROOT / "spec/gates/owner" / f"{phase}.json"
    if not owner_path.is_file():
        errors.append(f"missing owner gate {owner_path.relative_to(ROOT)}")
        owner = {}
    else:
        owner = load_json(owner_path)

    # Required artifact existence + optional acceptance-spec pinned hashes.
    artifacts = list(dict.fromkeys(spec.get("required_artifacts", []) + owner.get("required_artifacts", [])))
    for rel in artifacts:
        fp = inside_repo(rel)
        if fp is None or not fp.exists():
            errors.append(f"missing/unsafe required artifact: {rel}")
    for rel, expected_hash in (spec.get("required_artifact_hashes") or {}).items():
        fp = inside_repo(rel)
        if fp is None or not fp.is_file():
            errors.append(f"missing/unsafe hashed required artifact: {rel}")
        elif sha256(fp) != expected_hash:
            errors.append(f"required artifact hash mismatch: {rel}")

    # Traceability requirement evidence.
    for row in phase_rows:
        rid, vid = row["requirement_id"], row["verification_id"]
        if row["status"] != "passed":
            errors.append(f"{rid} status is {row['status']}, not passed")
            continue
        rel = row.get("evidence", "").strip()
        fp = inside_repo(rel)
        if fp is None or not fp.is_file():
            errors.append(f"{rid} evidence missing/unsafe: {rel!r}")
            continue
        errors.extend(
            verify_evidence(
                fp,
                phase=phase,
                commit=commit,
                verification_id=vid,
                verification_layer=row["verification_layer"],
                requirement_id=rid,
            )
        )

    # Owner minimum checks are executable acceptance, not documentation.
    owner_commands: list[str] = []
    native_seen = False
    for check in owner.get("minimum_checks", []):
        kind = check["kind"]
        cid = check["id"]
        if kind == "artifact":
            rel = check.get("artifact")
            fp = inside_repo(rel or "")
            if fp is None or not fp.exists():
                errors.append(f"{cid}: owner artifact missing/unsafe: {rel}")
        elif kind == "command":
            raw = check.get("command")
            if not raw:
                errors.append(f"{cid}: owner command is blank")
            else:
                owner_commands.append(raw)
        elif kind in {"evidence", "manual"}:
            vid = check.get("verification_id")
            if not vid:
                errors.append(f"{cid}: {kind} check lacks verification_id")
                continue
            matches = find_evidence(phase, verification_id=vid)
            if not matches:
                errors.append(f"{cid}: no evidence for {vid}")
            for fp in matches:
                errors.extend(
                    verify_evidence(
                        fp,
                        phase=phase,
                        commit=commit,
                        verification_id=vid,
                        native_required=bool(check.get("native_required")),
                    )
                )
                try:
                    if load_json(fp).get("environment", {}).get("native_or_vm") == "native":
                        native_seen = True
                except Exception:
                    pass
        elif kind == "scenario":
            sid = check.get("scenario_id")
            if not sid:
                errors.append(f"{cid}: scenario check lacks scenario_id")
                continue
            expected_hash, err = scenario_hash(sid)
            if err:
                errors.append(f"{cid}: {err}")
                continue
            matches = find_evidence(phase, scenario_id=sid)
            if not matches:
                errors.append(f"{cid}: no evidence for scenario {sid}")
                continue
            for fp in matches:
                errors.extend(
                    verify_evidence(
                        fp,
                        phase=phase,
                        commit=commit,
                        scenario_id=sid,
                        native_required=bool(check.get("native_required")),
                    )
                )
                try:
                    data = load_json(fp)
                    if data.get("scenario_hash") != expected_hash:
                        errors.append(f"{fp.relative_to(ROOT)} scenario_hash does not match spec/scenarios/{sid}.json")
                    if data.get("environment", {}).get("native_or_vm") == "native":
                        native_seen = True
                except Exception:
                    pass
        elif kind == "schema":
            files = evidence_files(phase)
            if not files:
                errors.append(f"{cid}: schema check requires phase evidence but none exists")
            for fp in files:
                _, schema_errors = read_evidence(fp)
                errors.extend(schema_errors)
        elif kind == "env":
            # Environment checks must be evidenced; a bare declaration is not enough.
            files = evidence_files(phase)
            if not files:
                errors.append(f"{cid}: environment check has no phase evidence")
        else:
            errors.append(f"{cid}: unsupported owner check kind {kind}")

    # Owner required verification IDs may map to traceability or standalone evidence.
    for vid in owner.get("required_verification_ids", []):
        matches = find_evidence(phase, verification_id=vid)
        if not matches:
            # Legacy P0 rows may be markdown evidence and therefore not discoverable by JSON scan.
            trace_matches = [r for r in phase_rows if r["verification_id"] == vid and r["status"] == "passed"]
            if not trace_matches:
                errors.append(f"owner gate requires verification {vid} but no passed evidence exists")
        else:
            for fp in matches:
                errors.extend(verify_evidence(fp, phase=phase, commit=commit, verification_id=vid))

    if owner.get("native_required"):
        if not native_seen:
            for fp in evidence_files(phase):
                try:
                    if load_json(fp).get("result") == "pass" and load_json(fp).get("environment", {}).get("native_or_vm") == "native":
                        native_seen = True
                        break
                except Exception:
                    continue
        if not native_seen:
            errors.append("owner gate requires native evidence but no passing native evidence was found")

    errors.extend(blocker_errors(phase))

    # Prior working-phase closure is a real regression prerequisite.
    phase_index = phase_ids.index(phase)
    for prior in phase_ids[:phase_index]:
        prior_entry = next(p for p in phases if p["id"] == prior)
        if not prior_entry.get("working_app_required"):
            continue
        prior_rows = [r for r in rows if r["primary_phase"] == prior and r["release_gate"] == "required"]
        for row in prior_rows:
            if row["status"] != "passed":
                errors.append(f"regression prerequisite {prior}/{row['requirement_id']} is {row['status']}")
                continue
            fp = inside_repo(row.get("evidence", ""))
            if fp is None or not fp.is_file():
                errors.append(f"regression prerequisite {prior}/{row['requirement_id']} evidence missing")

    if errors:
        print(f"{phase} gate FAILED before commands")
        for error in errors:
            print(" -", error)
        raise SystemExit(1)

    commands = list(spec.get("commands", []))
    for raw in owner_commands:
        if raw not in commands:
            commands.append(raw)
    for raw in commands:
        rc = run_command(raw)
        if rc:
            raise SystemExit(f"FAIL: command exited {rc}: {raw}")

    # Phase report must be semantically passed.
    json_report = ROOT / "agent/phase-reports" / f"{phase}.json"
    md_report = ROOT / "agent/phase-reports" / f"{phase}.md"
    if json_report.is_file():
        try:
            import jsonschema

            data = load_json(json_report)
            jsonschema.validate(instance=data, schema=load_json(ROOT / "spec/schemas/phase-report.schema.json"))
            if data.get("status") != "passed" or data.get("gate_exit_code") != 0:
                raise ValueError("phase report status/gate_exit_code is not passing")
            if parse_dt(data["ended_at"]) < parse_dt(data["started_at"]):
                raise ValueError("phase report ended_at precedes started_at")
            if commit != "unknown" and data.get("git_end_commit") != commit:
                raise ValueError("phase report git_end_commit does not match current commit")
            print(f"phase report {json_report.relative_to(ROOT)} validates")
        except Exception as exc:
            raise SystemExit(f"FAIL: phase report validation failed: {exc}")
    else:
        if not md_report.is_file():
            raise SystemExit(f"FAIL: missing phase report {md_report.relative_to(ROOT)}")
        text = md_report.read_text(errors="replace")
        if phase != "P0" and not re.search(r"(?:\*\*)?Status:(?:\*\*)?\s*passed", text, re.I):
            raise SystemExit(f"FAIL: {md_report.relative_to(ROOT)} does not report Status: passed")

    # Working application must have an explicit smoke command once the phase is implemented.
    if reg_entry.get("working_app_required") or owner.get("working_app_required"):
        smoke = spec.get("smoke_commands") or []
        if not smoke:
            raise SystemExit(
                f"FAIL: {phase} requires a working application but acceptance has no smoke_commands; "
                "add real non-mock runnable smoke checks before enabling the gate"
            )
        for raw in smoke:
            rc = run_command(raw)
            if rc:
                raise SystemExit(f"FAIL: working-app smoke command exited {rc}: {raw}")

    print(f"{phase} gate PASSED: {len(phase_rows)} primary release requirements closed, commit {commit[:7]}, dirty={dirty}")


if __name__ == "__main__":
    main()
