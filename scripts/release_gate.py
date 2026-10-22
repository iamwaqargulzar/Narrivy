#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

from evidence_policy import (
    RELEASED_EVIDENCE_SCHEMA_VERSIONS,
    assess_evidence,
    validate_requirement_evidence,
)

ROOT = Path(os.environ.get("NARRIVY_ROOT", Path(__file__).resolve().parents[1])).resolve()
# Owner decision 2026-09-28: the first release is qualified on Pop!_OS COSMIC
# Wayland only. Ubuntu/GNOME and other distributions follow after it is
# properly tested (agent/DECISIONS.md, BUILD_TARGET.json post_release_targets).
REQUIRED_NATIVE_LANES = ("cosmic-wayland",)
REQUIRED_TARGET_CLASSES = {"gtk", "qt", "firefox", "chromium", "electron", "terminal", "libreoffice", "xwayland"}


def inside(rel: str) -> Path | None:
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


def git_commit() -> str:
    r = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else "unknown"


def validate_evidence(
    path: Path,
    *,
    rid: str | None = None,
    vid: str | None = None,
    verification_layer: str | None = None,
    commit: str,
    expected_lane: str | None = None,
    required_artifact_hash: str | None = None,
) -> list[str]:
    errors: list[str] = []
    try:
        data = json.loads(path.read_text())
        import jsonschema

        jsonschema.validate(instance=data, schema=json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text()))
    except Exception as exc:
        return [f"{path.relative_to(ROOT)} invalid evidence: {exc}"]
    if rid and rid not in data.get("requirement_ids", []):
        errors.append(f"{rid}: evidence does not bind requirement")
    if vid and data.get("verification_id") != vid:
        errors.append(f"{rid or path.name}: verification_id mismatch")
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
                errors.append(f"{rid or path.name}: cannot resolve one verification profile from traceability")
            else:
                verification_layer = layers.pop()
        if verification_layer is not None:
            for trust_error in validate_requirement_evidence(
                data,
                evidence_path=path,
                root=ROOT,
                verification_id=vid or str(data.get("verification_id", "")),
                verification_layer=verification_layer,
                current_commit=commit,
                expected_lane=expected_lane,
                required_artifact_hash=required_artifact_hash,
            ):
                errors.append(f"{rid or path.name}: {trust_error}")
    else:
        for trust_error in assess_evidence(data, current_commit=commit, expected_lane=expected_lane):
            errors.append(f"{rid or path.name}: {trust_error}")
    return errors


def require_file(errors: list[str], rel: str, label: str) -> Path | None:
    fp = inside(rel)
    if fp is None or not fp.is_file():
        errors.append(f"missing/unsafe {label}: {rel}")
        return None
    return fp


ap = argparse.ArgumentParser()
ap.add_argument("--target", required=True, choices=["linux-1.0"])
args = ap.parse_args()
errors: list[str] = []
warnings: list[str] = []
commit = git_commit()
if commit == "unknown":
    errors.append("release qualification requires a real Git HEAD")

print("[1/7] running pack validation...")
if subprocess.run([sys.executable, str(ROOT / "scripts/validate_pack.py")], cwd=ROOT).returncode != 0:
    errors.append("validate_pack.py failed")

print("[2/7] checking release-gated requirement evidence...")
rows = list(csv.DictReader((ROOT / "spec/requirements-traceability.csv").open(newline="")))
for row in rows:
    if row["release_gate"] != "required":
        continue
    rid = row["requirement_id"]
    if row["status"] != "passed":
        errors.append(f"{rid}: status {row['status']}, not passed")
        continue
    rel = row.get("evidence", "").strip()
    ep = inside(rel)
    if ep is None or not ep.is_file():
        errors.append(f"{rid}: missing/unsafe evidence {rel!r}")
        continue
    errors.extend(
        validate_evidence(
            ep,
            rid=rid,
            vid=row["verification_id"],
            verification_layer=row["verification_layer"],
            commit=commit,
        )
    )

print("[3/7] checking legal/release evidence metadata...")
for rel, min_size in (("LICENSE", 200), ("THIRD_PARTY_NOTICES.md", 80)):
    fp = ROOT / rel
    if not fp.is_file() or fp.stat().st_size < min_size or "TODO" in fp.read_text(errors="replace"):
        errors.append(f"{rel} missing/incomplete")

base = ROOT / "artifacts/release-evidence/linux-1.0"
summary = base / "summary.md"
qualification = base / "qualification.json"
release_artifacts = base / "release-artifacts.json"
for fp in (summary, qualification, release_artifacts):
    if not fp.is_file():
        errors.append(f"missing release evidence {fp.relative_to(ROOT)}")
if summary.is_file():
    text = summary.read_text(errors="replace")
    if len(text.strip()) < 300 or "REPLACE" in text or "TODO" in text:
        errors.append("release summary appears incomplete/placeholder")
    if "Pop!_OS" not in text:
        errors.append("release summary must state the bounded qualification lane (Pop!_OS)")

p11_json = ROOT / "agent/phase-reports/P11.json"
if p11_json.is_file():
    try:
        import jsonschema

        data = json.loads(p11_json.read_text())
        jsonschema.validate(instance=data, schema=json.loads((ROOT / "spec/schemas/phase-report.schema.json").read_text()))
        if data.get("status") != "passed" or data.get("gate_exit_code") != 0:
            errors.append("P11 JSON phase report is not passing")
        if commit != "unknown" and data.get("git_end_commit") != commit:
            errors.append("P11 report commit mismatch")
    except Exception as exc:
        errors.append(f"P11 phase report invalid: {exc}")

print("[4/7] checking package, SBOM, provenance and hashes...")
manifest = {}
if release_artifacts.is_file():
    try:
        import jsonschema

        manifest = json.loads(release_artifacts.read_text())
        jsonschema.validate(instance=manifest, schema=json.loads((ROOT / "spec/schemas/release-artifacts.schema.json").read_text()))
        if commit != "unknown" and manifest.get("git_commit") != commit:
            errors.append("release-artifacts.json commit mismatch")
        if manifest.get("git_dirty") is not False:
            errors.append("release artifacts must be built from a clean tree")
        kinds = {a.get("kind") for a in manifest.get("artifacts", [])}
        for required_kind in ("deb", "sbom", "provenance"):
            if required_kind not in kinds:
                errors.append(f"release artifact kind missing: {required_kind}")
        seen: set[str] = set()
        for art in manifest.get("artifacts", []):
            rel = art.get("path", "")
            if rel in seen:
                errors.append(f"duplicate release artifact path: {rel}")
            seen.add(rel)
            fp = require_file(errors, rel, "release artifact")
            expected = art.get("sha256", "").lower()
            if not re.fullmatch(r"[a-f0-9]{64}", expected):
                errors.append(f"invalid SHA-256 for {rel}")
                continue
            if fp and sha256(fp) != expected:
                errors.append(f"checksum mismatch: {rel}")
            if fp and fp.stat().st_size != art.get("size_bytes"):
                errors.append(f"size mismatch: {rel}")
        for key, kind in (("sbom_path", "sbom"), ("provenance_path", "provenance")):
            rel = manifest.get(key, "")
            require_file(errors, rel, key)
            if not any(a.get("kind") == kind and a.get("path") == rel for a in manifest.get("artifacts", [])):
                errors.append(f"{key} must point to the declared {kind} artifact")
        signing = manifest.get("signing") or {}
        if signing.get("signed") and signing.get("status") != "signed":
            errors.append("signing.signed=true requires status=signed")
        if not signing.get("signed") and signing.get("status") == "signed":
            errors.append("signing status inconsistent")
    except Exception as exc:
        errors.append(f"invalid release-artifacts.json: {exc}")

print("[5/7] checking native qualification lanes and compute claims...")
q = {}
if qualification.is_file():
    try:
        import jsonschema

        q = json.loads(qualification.read_text())
        jsonschema.validate(instance=q, schema=json.loads((ROOT / "spec/schemas/linux-qualification.schema.json").read_text()))
        if commit != "unknown" and q.get("git_commit") != commit:
            errors.append("qualification.json commit mismatch")
        passing = {e.get("kind"): e for e in q.get("environments", []) if e.get("e2e_pass") is True}
        artifact_hashes = {a.get("sha256") for a in manifest.get("artifacts", [])}
        for kind in REQUIRED_NATIVE_LANES:
            env = passing.get(kind)
            if not env:
                errors.append(f"missing passing {kind} qualification")
                continue
            if env.get("native_or_vm") != "native":
                errors.append(f"{kind} qualification must be native")
            if env.get("session").lower() != "wayland":
                errors.append(f"{kind} qualification must be Wayland")
            ep = require_file(errors, env.get("evidence", ""), f"{kind} evidence")
            if ep:
                errors.extend(
                    validate_evidence(
                        ep,
                        commit=commit,
                        expected_lane=kind,
                        required_artifact_hash=env.get("package_sha256"),
                    )
                )
            if env.get("package_sha256") not in artifact_hashes:
                errors.append(f"{kind} package_sha256 is not a release artifact hash")
        for key in ("cpu_fallback_pass", "accessibility_pass", "privacy_security_pass"):
            if q.get(key) is not True:
                errors.append(f"qualification {key} is not true")
        for claim in q.get("accelerator_claims", []):
            if claim.get("pass") is not True:
                errors.append(f"accelerator claim not passing: {claim.get('device')}")
            ep = require_file(errors, claim.get("evidence", ""), f"accelerator evidence {claim.get('device')}")
            if ep:
                errors.extend(validate_evidence(ep, commit=commit))
            if commit != "unknown" and claim.get("git_commit") != commit:
                errors.append(f"accelerator claim commit mismatch: {claim.get('device')}")
    except Exception as exc:
        errors.append(f"invalid qualification.json: {exc}")

print("[6/7] checking bounded target-application matrix...")
if q:
    matrix = q.get("target_matrix", [])
    classes = {row.get("class") for row in matrix}
    missing = REQUIRED_TARGET_CLASSES - classes
    if missing:
        errors.append("target matrix missing classes: " + ", ".join(sorted(missing)))
    for row in matrix:
        ep = require_file(errors, row.get("evidence", ""), f"target matrix evidence {row.get('class')}")
        if ep:
            errors.extend(validate_evidence(ep, commit=commit))
        if row.get("result") == "unsupported" and not row.get("delivery_outcome"):
            errors.append(f"target class {row.get('class')} lacks fallback/unsupported outcome")

print("[7/7] checking performance qualification protocol...")
if q:
    perf = q.get("performance") or {}
    if perf.get("pass") is not True:
        errors.append("performance qualification did not pass")
    if perf.get("protocol") != "docs/maintainers/qualification/performance-benchmark-protocol.md":
        errors.append("performance qualification used wrong/missing protocol")
    ep = require_file(errors, perf.get("evidence", ""), "performance evidence")
    if ep:
        errors.extend(validate_evidence(ep, commit=commit))
    for name, metric in (perf.get("metrics") or {}).items():
        if metric.get("p95_ms", 0) < metric.get("p50_ms", 0):
            errors.append(f"performance metric {name}: p95 < p50")

if warnings:
    print("Warnings:")
    for warning in warnings:
        print(" -", warning)
if errors:
    print("Linux 1.0 release gate FAILED")
    for error in errors:
        print(" -", error)
    raise SystemExit(1)
print("Linux 1.0 release gate PASSED: requirements, provenance, packages, native lanes, CPU fallback, accelerator claims, target matrix and performance evidence verified.")
