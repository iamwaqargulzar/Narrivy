#!/usr/bin/env python3
from __future__ import annotations
import json, subprocess, sys, tomllib, hashlib, csv, re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
required = ['agent/completion-plan/README.md','SOURCE_OF_TRUTH.md','BUILD_TARGET.json','docs/product/vision-and-scope.md','AGENTS.md','agent/AUTONOMY_POLICY.md','agent/STATE.json','agent/PHASES.json','agent/ROADMAP_PHASES.json','agent/METRICS.csv','docs/product/requirements.md','spec/requirements-traceability.csv','scripts/phase_gate.py','scripts/release_gate.py','docs/architecture/README.md','scripts/verify_architecture_map.py','spec/catalogs/engines.json','spec/catalogs/modules.json','spec/catalogs/runtimes.json','spec/catalogs/pipelines.json','spec/catalogs/features.json']
errors = []

for rel in required:
    if not (ROOT/rel).is_file():
        errors.append(f'missing required file {rel}')

# JSON syntactic check
for p in ROOT.rglob('*.json'):
    # skip target artifacts, caches, and the salvage code archive (reference-only,
    # contains upstream JSONC-style configs that are not pack JSON contracts)
    if any(part in ('target','node_modules','.git','code','old') for part in p.parts):
        continue
    try:
        json.loads(p.read_text())
    except Exception as e:
        errors.append(f'invalid JSON {p.relative_to(ROOT)}: {e}')

for p in ROOT.rglob('*.toml'):
    if any(part in ('target','node_modules','.git') for part in p.parts):
        continue
    try:
        tomllib.loads(p.read_text())
    except Exception as e:
        errors.append(f'invalid TOML {p.relative_to(ROOT)}: {e}')

# --- Phase/Target registry checks (now includes P0F) ---
try:
    target = json.loads((ROOT/'BUILD_TARGET.json').read_text())
    reg = json.loads((ROOT/'agent/PHASES.json').read_text())
    phases = reg['phases']
    ids = [p['id'] for p in phases]
    # Expected: P0, P0F, P1..P11
    expected = ['P0','P0F'] + [f'P{i}' for i in range(1,12)]
    if ids != expected:
        errors.append(f'phase order invalid: {ids}; expected {expected}')
    if target.get('target_phase') != ids[-1]:
        errors.append('BUILD_TARGET target_phase must match final Linux 1.0 phase')
    for p in phases:
        for k in ['spec','prompt','acceptance']:
            if not (ROOT/p[k]).is_file():
                errors.append(f'{p["id"]} missing {k}: {p[k]}')
        if not (ROOT/'agent/phase-reports'/f'{p["id"]}.md').is_file():
            errors.append(f'{p["id"]} phase report template missing')
        # Check that working_app_required matches new policy: P0,P0F,P1 false, else true
        pid = p['id']
        should_work = pid not in ('P0','P0F','P1')
        if bool(p.get('working_app_required')) != should_work:
            errors.append(f'{pid}: working_app_required must be {should_work} per current phase model (P0,P0F,P1 false, else true)')
except Exception as e:
    errors.append(f'phase/target registry invalid: {e}')

# --- Schema validation (real JSON Schema) ---
try:
    import jsonschema
    has_js = True
except ImportError:
    has_js = False
    errors.append('jsonschema not installed — run `uv pip install jsonschema` or `pip install jsonschema`; schema validation required')

if has_js:
    schema_map = {
        'spec/schemas/build-target.schema.json': ['BUILD_TARGET.json'],
        'spec/schemas/evidence.schema.json': [],  # evidence files are under agent/evidence/**/*.json when present
        'spec/schemas/evidence-index.schema.json': ['agent/evidence/INDEX.json'],
        'spec/schemas/phase-report.schema.json': [], # phase reports have markdown, not json - skip for now but validate example if exists
        # No qualification record is presented as valid: the previously tracked
        # one asserted a full pass for both lanes while its sibling summary.md
        # said GNOME was deferred, and it was bound to a commit that does not
        # exist in this repository. It is retained under
        # archive/quarantined-release-evidence-2026-09-27/ for audit. `release_gate.py` fails
        # closed until a real qualification is generated, and this schema is
        # exercised against a genuine candidate bundle instead.
        'spec/schemas/linux-qualification.schema.json': [],
        'spec/schemas/release-artifacts.schema.json': [
            'artifacts/candidates/linux-1.0-supply-chain/release-artifacts.json'
        ],
        'spec/schemas/owner-gate.schema.json': list(str(p.relative_to(ROOT)) for p in (ROOT/'spec/gates/owner').glob('P*.json')),
        'spec/schemas/mode.schema.json': ['spec/examples/mode-clean.json'],
        'spec/schemas/profile.schema.json': ['spec/examples/profile-browser.json'],
        'spec/schemas/model-manifest.schema.json': ['spec/examples/model-reference.example.json']
        + sorted(str(p.relative_to(ROOT)) for p in (ROOT/'tools/reference-models/manifests').glob('*.json')),
        'spec/schemas/privacy-policy.schema.json': ['spec/examples/privacy-default.json'],
        'spec/schemas/plugin-manifest.schema.json': ['spec/examples/plugin-processor.example.json'],
        'spec/schemas/runtime-pack.schema.json': ['spec/examples/runtime-pack.example.json'],
        'spec/schemas/pipeline.schema.json': ['spec/examples/pipeline-fast.json','spec/examples/pipeline-smart-local.json'],
        'spec/schemas/resource-entry.schema.json': ['spec/examples/resource-entry.example.json'],
        'spec/schemas/config-bundle.schema.json': ['spec/examples/config-bundle.example.json'],
        'spec/schemas/inference-provider.schema.json': ['spec/examples/inference-provider.example.json'],
    }
    for schema_rel, instance_rels in schema_map.items():
        sp = ROOT / schema_rel
        if not sp.is_file():
            errors.append(f'missing schema {schema_rel}')
            continue
        try:
            schema = json.loads(sp.read_text())
        except Exception as e:
            errors.append(f'invalid schema {schema_rel}: {e}')
            continue
        for irel in instance_rels:
            ip = ROOT / irel
            if not ip.is_file():
                # For qualification/release, absence is okay until P11 but not an error here; gate will catch
                continue
            try:
                instance = json.loads(ip.read_text())
                jsonschema.validate(instance=instance, schema=schema)
            except Exception as e:
                errors.append(f'schema validation failed {irel} against {schema_rel}: {e}')
    # Also validate all example files that claim a schema
    for p in (ROOT/'spec/examples').glob('*.json'):
        try:
            data = json.loads(p.read_text())
            # If file contains $schema, try to validate against that schema if exists
            sref = data.get('$schema')
            if sref and 'narrivy.local' in sref:
                # map to local file
                # crude: find matching schema by id suffix
                for sfile in (ROOT/'spec/schemas').glob('*.json'):
                    sch = json.loads(sfile.read_text())
                    if sch.get('$id') == sref or sch.get('$id','').endswith(sref.split('/')[-1]):
                        jsonschema.validate(instance=data, schema=sch)
                        break
        except Exception as e:
            errors.append(f'example schema validation failed {p.relative_to(ROOT)}: {e}')

# --- Owner gate integrity ---
# Call verify_owner_gates.py as subprocess to share logic
p = subprocess.run([sys.executable, str(ROOT/'scripts/verify_owner_gates.py')], cwd=ROOT, capture_output=True, text=True)
if p.returncode != 0:
    errors.append(f'verify_owner_gates.py failed:\n{p.stdout}\n{p.stderr}')
else:
    # Also print OK for visibility
    pass

# --- Stale phase reference detection ---
# Ensure no doc still references old P0->P11 without P0F, or old phase numbers that contradict registry
import re
phases_ids = set(ids) if 'ids' in locals() else set()
# Search markdown for stale phase links
stale_patterns = [
    (r'phase-02-walking-skeleton.*Walking Skeleton', 'phase-02 should now be Walking Skeleton per the legacy planning amendment — check spec updated'),
]
# Generic: ensure every phase id mentioned in docs is in registry
for md in list(ROOT.rglob('*.md')):
    if any(part in ('target','node_modules','.git') for part in md.parts):
        continue
    txt = md.read_text(errors='ignore')
    # Find mentions of P[0-9] that look like phase refs but could be false positives; we only flag explicit phase gate references that are stale
    if 'python3 scripts/phase_gate.py --phase P' in txt:
        for m in re.findall(r'phase_gate\.py --phase (P[0-9A-Z]+)', txt):
            if m not in phases_ids:
                errors.append(f'stale phase reference in {md.relative_to(ROOT)}: {m} not in registry {sorted(phases_ids)}')

# --- Traceability consistency ---
try:
    req_text = (ROOT/'docs/product/requirements.md').read_text()
    req_ids = re.findall(r'- \*\*([A-Z]+-\d+)\*\*\s+.+', req_text)
    # Need to handle Post-1.0 markers: they still are requirements, so count them
    # Filter to unique
    if len(req_ids) != len(set(req_ids)):
        dup = sorted({x for x in req_ids if req_ids.count(x) > 1})
        errors.append('duplicate requirement IDs in catalog: ' + ', '.join(dup))
    rows = list(csv.DictReader((ROOT/'spec/requirements-traceability.csv').open(newline='')))
    row_ids = [r.get('requirement_id','') for r in rows]
    if len(row_ids) != len(set(row_ids)):
        errors.append('duplicate requirement IDs in traceability')
    missing = sorted(set(req_ids) - set(row_ids))
    extra = sorted(set(row_ids) - set(req_ids))
    if missing:
        errors.append('missing traceability: ' + ', '.join(missing))
    if extra:
        errors.append('traceability IDs not in catalog: ' + ', '.join(extra))
    required_cols = {'requirement_id','severity','scope','primary_phase','component','acceptance_summary','verification_id','verification_layer','release_gate','status','evidence','evidence_classification','evidence_disposition'}
    if rows and not required_cols.issubset(rows[0].keys()):
        errors.append('traceability missing required columns: ' + ', '.join(sorted(required_cols-set(rows[0].keys()))))
    valid_status = {'planned','implementing','passed','blocked','roadmap'}
    valid_evidence_classification = {'unit','mock','simulated','integration','native-platform','package-artifact','manual','not-applicable'}
    valid_evidence_disposition = {'qualified','historical-quarantined','not-applicable'}
    # For validation, allow roadmap status for deferred
    release_phases = set(ids) if 'ids' in locals() else set()
    roadmap_phases = {p['id'] for p in json.loads((ROOT/'agent/ROADMAP_PHASES.json').read_text())['phases']}
    for i, r in enumerate(rows, 2):
        rid = r.get('requirement_id', f'row{i}')
        for k in ['severity','scope','primary_phase','component','acceptance_summary','verification_id','verification_layer','release_gate','status','evidence_classification','evidence_disposition']:
            if not r.get(k,'').strip():
                errors.append(f'{rid}: blank {k}')
        if r.get('status') not in valid_status:
            errors.append(f'{rid}: invalid status {r.get("status")}')
        if r.get('evidence_classification') not in valid_evidence_classification:
            errors.append(f'{rid}: invalid evidence_classification {r.get("evidence_classification")}')
        if r.get('evidence_disposition') not in valid_evidence_disposition:
            errors.append(f'{rid}: invalid evidence_disposition {r.get("evidence_disposition")}')
        if r.get('release_gate') not in {'required','roadmap'}:
            errors.append(f'{rid}: invalid release_gate')
        if r.get('release_gate')=='required' and r.get('primary_phase') not in release_phases:
            errors.append(f'{rid}: required requirement primary_phase must be a release phase (found {r.get("primary_phase")})')
        if r.get('release_gate')=='roadmap' and r.get('primary_phase') not in roadmap_phases:
            errors.append(f'{rid}: roadmap requirement primary_phase must be a roadmap phase (found {r.get("primary_phase")})')
        if r.get('release_gate')=='roadmap' and r.get('scope')!='post-1.0':
            errors.append(f'{rid}: roadmap row must be post-1.0')
        if r.get('status')=='passed' and not r.get('evidence','').strip():
            errors.append(f'{rid}: passed without evidence')
        if r.get('status')=='passed' and r.get('evidence_disposition')!='qualified':
            errors.append(f'{rid}: passed evidence must be qualified')
        if r.get('release_gate')=='required' and r.get('evidence_disposition')=='not-applicable':
            errors.append(f'{rid}: release-gated requirement cannot have not-applicable evidence')
        if r.get('release_gate')=='roadmap' and r.get('evidence_disposition')!='not-applicable':
            errors.append(f'{rid}: roadmap requirement must have not-applicable evidence')
        # Check that verification IDs are unique where required? Allow duplicates? We'll just ensure no empty
        if r.get('release_gate')=='required' and r.get('scope')!='linux-1.0':
            errors.append(f'{rid}: required row must be linux-1.0 scope')
    # A phase report must equal its authoritative sources. Hand-typed progress
    # figures previously contradicted the traceability CSV six different ways in
    # one document, and the release gate treats the report as authoritative.
    generator = ROOT/'scripts/generate_phase_report.py'
    if generator.is_file():
        drift = subprocess.run(
            [sys.executable, str(generator), '--phase', 'P11', '--check'],
            cwd=ROOT, capture_output=True, text=True,
        )
        if drift.returncode != 0:
            errors.append(
                'phase report drift from traceability/evidence:\n'
                + (drift.stdout or '') + (drift.stderr or '')
            )
    # Duplicate verification IDs check
    ver_ids = [r.get('verification_id','') for r in rows if r.get('verification_id')]
    dup_ver = sorted({x for x in ver_ids if ver_ids.count(x)>1})
    if dup_ver:
        errors.append('duplicate verification IDs: ' + ', '.join(dup_ver))
except Exception as e:
    errors.append(f'traceability validation error: {e}')

# --- No placeholder acceptance specs for active phases ---
try:
    reg = json.loads((ROOT/'agent/PHASES.json').read_text())
    for ph in reg['phases']:
        acc = json.loads((ROOT/ph['acceptance']).read_text())
        if acc.get('phase') != ph['id']:
            errors.append(f"{ph['id']}: acceptance phase mismatch")
        # If phase is active or earlier, implemented should be judged? We just check that implemented=false not left empty
        if not isinstance(acc.get('implemented'), bool):
            errors.append(f"{ph['id']}: acceptance implemented must be bool")
        # Check that placeholder commands are not empty for implemented true phases
        if acc.get('implemented') is True and not acc.get('commands'):
            # P0 is okay to have commands, but generally implemented true needs commands/artifacts
            pass
except Exception as e:
    errors.append(f'acceptance check error: {e}')

# --- Path safety checks ---
for row in rows if 'rows' in locals() else []:
    ev = row.get('evidence','').strip()
    if ev:
        # Must be inside repo, not absolute, no ..
        if ev.startswith('/') or '..' in ev:
            errors.append(f"{row['requirement_id']}: unsafe evidence path {ev!r}")
        # If file exists, ensure it's inside
        try:
            p = (ROOT/ev).resolve()
            rr = ROOT.resolve()
            if p != rr and rr not in p.parents:
                errors.append(f"{row['requirement_id']}: evidence path escapes repo {ev!r}")
        except Exception:
            errors.append(f"{row['requirement_id']}: evidence path resolve failed {ev!r}")

# --- Bounded support claim validation ---
# Ensure no doc claims 24.04+ open-ended without bounded lanes
# Only check user-facing docs that might publish support claims; historical/migration docs may mention old phrasing in context
for md in [ROOT/'README.md']:
    if md.is_file():
        txt = md.read_text(errors='ignore')
        if re.search(r'24\.04\+', txt) and 'bounded' not in txt.lower():
            errors.append(f'{md.relative_to(ROOT)} contains unbounded \"24.04+\" without bounded support lane explanation')
# Note: docs/platforms/bounded-support.md intentionally documents the change from 24.04+ to bounded lanes, so it is excluded
# docs/maintainers/phases and archive/docs/autonomous-build are historical/phase specs and may mention the old phrase in the context of the change

if errors:
    print('Pack validation FAILED')
    for e in errors:
        print(' -', e)
    sys.exit(1)

# Run subordinate validators
for script in ['verify_traceability.py','verify_phase_model.py','check_action_pins.py','verify-boundaries.py','verify_pipeline_contracts.py']:
    pp = subprocess.run([sys.executable, str(ROOT/'scripts'/script)], cwd=ROOT)
    if pp.returncode != 0:
        sys.exit(pp.returncode)

try:
    manifest = json.loads((ROOT/'BUILD-PACK-MANIFEST.json').read_text())
    if manifest.get('schema_version')==3:
        pp = subprocess.run([sys.executable, str(ROOT/'scripts/verify_manifest.py')], cwd=ROOT)
        if pp.returncode != 0:
            sys.exit(pp.returncode)
except Exception as e:
    print(f'Manifest validation error: {e}')
    sys.exit(1)

print('Pack static validation OK.')
