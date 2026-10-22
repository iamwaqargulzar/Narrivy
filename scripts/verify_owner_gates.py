#!/usr/bin/env python3
"""Verify owner-gate baseline integrity (Part E1)."""
from __future__ import annotations
import hashlib, json, sys
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
baseline_path = ROOT / "spec/gates/owner/BASELINE.json"
owner_dir = ROOT / "spec/gates/owner"

def sha256(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()

errors = []
if not baseline_path.is_file():
    print("FAIL: missing spec/gates/owner/BASELINE.json")
    sys.exit(1)
try:
    baseline = json.loads(baseline_path.read_text())
except Exception as e:
    print(f"FAIL: invalid BASELINE.json: {e}")
    sys.exit(1)

# Check each owner gate file matches baseline
for name, meta in baseline.items():
    fp = owner_dir / name
    if not fp.is_file():
        errors.append(f"baseline {name} missing file")
        continue
    real = sha256(fp)
    if real != meta.get("sha256"):
        errors.append(f"owner gate {name} hash mismatch: baseline {meta.get('sha256')[:12]}.. vs actual {real[:12]}.. (owner-level change requires baseline update)")

# Also detect new un-baselined files
for fp in owner_dir.glob("*.json"):
    if fp.name == "BASELINE.json":
        continue
    if fp.name not in baseline:
        errors.append(f"owner gate {fp.name} not in BASELINE.json — add via owner action")

# Also validate each gate against owner-gate schema
import json as _json
try:
    import jsonschema  # type: ignore
    has_js = True
except ImportError:
    has_js = False

schema_path = ROOT / "spec/schemas/owner-gate.schema.json"
if schema_path.is_file() and has_js:
    schema = json.loads(schema_path.read_text())
    for fp in owner_dir.glob("P*.json"):
        try:
            data = json.loads(fp.read_text())
            jsonschema.validate(instance=data, schema=schema)
        except Exception as e:
            errors.append(f"owner gate schema error {fp.name}: {e}")
elif not has_js:
    # Fallback: minimal required keys check
    for fp in owner_dir.glob("P*.json"):
        try:
            data = json.loads(fp.read_text())
            for k in ["schema_version","phase","minimum_checks","required_verification_ids"]:
                if k not in data:
                    errors.append(f"{fp.name} missing {k}")
        except Exception as e:
            errors.append(f"owner gate JSON error {fp.name}: {e}")

if errors:
    print("Owner-gate integrity FAILED")
    for e in errors:
        print(" -", e)
    sys.exit(1)
print(f"Owner-gate integrity OK: {len(baseline)} gates verified")
