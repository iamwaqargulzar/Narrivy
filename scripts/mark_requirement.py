#!/usr/bin/env python3
from __future__ import annotations
import argparse, csv, json, os, subprocess, sys, re
from pathlib import Path
from evidence_policy import (
    validate_requirement_evidence,
)

ROOT = Path(os.environ.get("NARRIVY_ROOT", Path(__file__).resolve().parents[1])).resolve()

def inside(rel: str) -> Path | None:
    try:
        p = (ROOT/rel).resolve()
        rr = ROOT.resolve()
        if p == rr or rr in p.parents:
            return p
        return None
    except Exception:
        return None

ap = argparse.ArgumentParser(description="Mark requirement status with evidence validation per the legacy planning amendment Part E3")
ap.add_argument('requirement_id')
ap.add_argument('status', choices=['planned','implementing','passed','blocked','roadmap'])
ap.add_argument('--evidence', default='')
a = ap.parse_args()
p = ROOT/'spec/requirements-traceability.csv'
rows = list(csv.DictReader(p.open(newline='')))
fields = list(rows[0].keys()) if rows else ['requirement_id','severity','scope','primary_phase','secondary_phase','component','acceptance_summary','verification_id','verification_layer','release_gate','status','evidence','evidence_classification','evidence_disposition']
match = [r for r in rows if r['requirement_id']==a.requirement_id]
if len(match)!=1:
    raise SystemExit(f'Expected exactly one row for {a.requirement_id}')

r = match[0]

if a.status == 'passed':
    if not a.evidence:
        raise SystemExit('passed requires --evidence relative path')
    ep = inside(a.evidence)
    if ep is None or not ep.exists():
        raise SystemExit(f'evidence path does not exist inside repo or escapes repo: {a.evidence}')
    # Validate evidence schema and verification ID
    try:
        data = json.loads(ep.read_text())
    except Exception as e:
        raise SystemExit(f'evidence {a.evidence} invalid JSON: {e}')
    # Schema validation is mandatory for a pass claim.
    try:
        import jsonschema
        schema = json.loads((ROOT/'spec/schemas/evidence.schema.json').read_text())
        jsonschema.validate(instance=data, schema=schema)
    except ImportError:
        raise SystemExit('jsonschema is required to mark a requirement passed')
    except Exception as e:
        raise SystemExit(f'evidence {a.evidence} schema validation failed: {e}')
    # Check verification ID matches
    if data.get('verification_id') != r['verification_id']:
        raise SystemExit(f"evidence verification_id {data.get('verification_id')} != traceability {r['verification_id']} for {a.requirement_id}")
    # Check requirement_ids includes this requirement
    if a.requirement_id not in data.get('requirement_ids', []):
        raise SystemExit(f"evidence requirement_ids {data.get('requirement_ids')} does not include {a.requirement_id}")
    # Check phase matches
    if data.get('phase') != r['primary_phase']:
        # Allow if secondary_phase matches? But we enforce primary
        raise SystemExit(f"evidence phase {data.get('phase')} != requirement primary_phase {r['primary_phase']} for {a.requirement_id}")
    try:
        current = subprocess.run(["git","rev-parse","HEAD"], cwd=ROOT, capture_output=True, text=True, timeout=5).stdout.strip()
    except Exception as exc:
        raise SystemExit(f'cannot resolve candidate commit for evidence validation: {exc}')
    trust_errors = validate_requirement_evidence(
        data,
        evidence_path=ep,
        root=ROOT,
        verification_id=r['verification_id'],
        verification_layer=r['verification_layer'],
        current_commit=current or 'unknown',
    )
    if trust_errors:
        raise SystemExit('evidence is not eligible for a passed claim: ' + '; '.join(trust_errors))
    # Check unsafe paths already done
    if '..' in a.evidence or a.evidence.startswith('/'):
        raise SystemExit(f'unsafe evidence path: {a.evidence}')

# Roadmap status must have empty evidence? Not required but we clear if roadmap
if a.evidence:
    if '..' in a.evidence or a.evidence.startswith('/'):
        raise SystemExit(f'unsafe evidence path: {a.evidence}')
    ep = inside(a.evidence)
    if ep is None:
        raise SystemExit(f'evidence path escapes repo: {a.evidence}')
    # For non-passed, evidence may point to future file; but we check existence only for passed
    if a.status == 'passed' and not ep.exists():
        raise SystemExit(f'evidence path does not exist: {a.evidence}')
    r['evidence'] = a.evidence
else:
    # If status is passed without evidence, already rejected above
    if a.status == 'roadmap':
        r['evidence'] = ''
    elif a.status != 'passed':
        # Keep existing or clear? We keep existing if not provided
        pass

r['status'] = a.status
if a.status == 'passed':
    r['evidence_disposition'] = 'qualified'
    r['evidence_classification'] = data['evidence_type']

# Also handle severity/scope for roadmap vs required consistency
if a.status == 'roadmap' and r['release_gate'] != 'roadmap':
    print(f"WARN: marking {a.requirement_id} as roadmap but release_gate is {r['release_gate']} — ensure traceability scope is post-1.0", file=sys.stderr)
if a.status == 'passed' and r['release_gate'] == 'roadmap':
    print(f"WARN: marking roadmap requirement {a.requirement_id} as passed — usually roadmap is not gated", file=sys.stderr)

with p.open('w', newline='') as f:
    w = csv.DictWriter(f, fieldnames=fields, lineterminator='\n')
    w.writeheader()
    w.writerows(rows)
print(f'{a.requirement_id} -> {a.status}' + (f' ({r["evidence"]})' if r['evidence'] else ''))
