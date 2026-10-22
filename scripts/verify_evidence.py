#!/usr/bin/env python3
"""Validate and classify evidence without changing any repository file."""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

from evidence_policy import (
    RELEASED_EVIDENCE_SCHEMA_VERSIONS,
    assess_evidence,
    validate_requirement_evidence,
)

ROOT = Path(os.environ.get("NARRIVY_ROOT", Path(__file__).resolve().parents[1])).resolve()


def current_commit() -> str:
    result = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else "unknown"


def evidence_paths(path: Path | None) -> list[Path]:
    if path is not None:
        return [path if path.is_absolute() else ROOT / path]
    return sorted((ROOT / "agent" / "evidence").rglob("*.json"))


def rel(path: Path) -> str:
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return str(path)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--path", type=Path)
    parser.add_argument("--production", action="store_true")
    parser.add_argument("--expected-lane")
    parser.add_argument("--required-artifact-hash")
    args = parser.parse_args()

    errors: list[str] = []
    try:
        import jsonschema

        evidence_schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
    except ImportError:
        print("Evidence validation FAILED")
        print(" - jsonschema is required to validate evidence")
        return 1
    except Exception as exc:
        print("Evidence validation FAILED")
        print(f" - evidence schema is unavailable or invalid: {exc}")
        return 1
    historical = 0
    candidates = 0
    commit = current_commit()
    try:
        import csv

        traceability = {
            row["requirement_id"]: row
            for row in csv.DictReader((ROOT / "spec/requirements-traceability.csv").open(newline=""))
        }
    except Exception as exc:
        print("Evidence validation FAILED")
        print(f" - traceability is unavailable for verification profiles: {exc}")
        return 1
    for path in evidence_paths(args.path):
        try:
            data = json.loads(path.read_text())
        except Exception as exc:
            errors.append(f"{rel(path)}: invalid JSON: {exc}")
            continue
        if path.name == "INDEX.json":
            continue
        try:
            jsonschema.validate(instance=data, schema=evidence_schema)
        except Exception as exc:
            errors.append(f"{rel(path)}: schema validation failed: {exc}")
            continue
        if data.get("schema_version") == 1:
            historical += 1
        else:
            candidates += 1
        if data.get("schema_version") in RELEASED_EVIDENCE_SCHEMA_VERSIONS:
            requirement_ids = data.get("requirement_ids")
            if not isinstance(requirement_ids, list) or not requirement_ids:
                errors.append(f"{rel(path)}: released evidence has no requirement binding")
            for requirement_id in requirement_ids if isinstance(requirement_ids, list) else []:
                row = traceability.get(requirement_id)
                if row is None:
                    errors.append(f"{rel(path)}: evidence binds unknown requirement {requirement_id!r}")
                    continue
                for error in validate_requirement_evidence(
                    data,
                    evidence_path=path,
                    root=ROOT,
                    verification_id=row["verification_id"],
                    verification_layer=row["verification_layer"],
                    current_commit=commit,
                    expected_lane=args.expected_lane,
                    required_artifact_hash=args.required_artifact_hash,
                ):
                    errors.append(f"{rel(path)}: {error}")
        else:
            for error in assess_evidence(data, current_commit=commit, expected_lane=args.expected_lane):
                errors.append(f"{rel(path)}: {error}")
        # Schema-v1 material is already quarantined and cannot qualify. Do not
        # reinterpret its stale artifact bindings as a new qualification claim.
        if data.get("schema_version") not in RELEASED_EVIDENCE_SCHEMA_VERSIONS:
            continue

    print(
        f"Evidence inventory: {historical} historical/quarantined, "
        f"{candidates} released candidate record(s)."
    )
    if errors:
        print("Evidence validation FAILED")
        for error in errors:
            print(" -", error)
        return 1
    print("Evidence validation OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
