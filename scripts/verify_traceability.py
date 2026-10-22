#!/usr/bin/env python3
from __future__ import annotations
import csv,re,sys,json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
req_text=(ROOT/'docs/product/requirements.md').read_text()
req_ids=re.findall(r'- \*\*([A-Z]+-\d+)\*\*\s+.+',req_text)
if len(req_ids)!=len(set(req_ids)):
    dup=sorted({x for x in req_ids if req_ids.count(x)>1}); print('ERROR duplicate requirement IDs in catalog:',', '.join(dup)); sys.exit(1)
rows=list(csv.DictReader((ROOT/'spec/requirements-traceability.csv').open(newline='')))
row_ids=[r.get('requirement_id','') for r in rows]; errors=[]
if len(row_ids)!=len(set(row_ids)): errors.append('duplicate requirement IDs in traceability')
missing=sorted(set(req_ids)-set(row_ids)); extra=sorted(set(row_ids)-set(req_ids))
if missing: errors.append('missing traceability: '+', '.join(missing))
if extra: errors.append('traceability IDs not in catalog: '+', '.join(extra))
required_cols={'requirement_id','severity','scope','primary_phase','component','acceptance_summary','verification_id','verification_layer','release_gate','status','evidence','evidence_classification','evidence_disposition'}
if rows and not required_cols.issubset(rows[0]): errors.append('traceability missing required columns: '+', '.join(sorted(required_cols-set(rows[0]))))
valid_status={'planned','implementing','passed','blocked','roadmap'}
valid_evidence_classification={'unit','mock','simulated','integration','native-platform','package-artifact','manual','not-applicable'}
valid_evidence_disposition={'qualified','historical-quarantined','not-applicable'}
release_phases={p['id'] for p in json.loads((ROOT/'agent/PHASES.json').read_text())['phases']}
roadmap_phases={p['id'] for p in json.loads((ROOT/'agent/ROADMAP_PHASES.json').read_text())['phases']}
try:
    import jsonschema
    verification_contract=json.loads((ROOT/'spec/evidence-verification-profiles.json').read_text())
    verification_contract_schema=json.loads((ROOT/'spec/schemas/evidence-verification-profiles.schema.json').read_text())
    jsonschema.validate(instance=verification_contract, schema=verification_contract_schema)
    layer_profiles={}
    for profile in verification_contract['profiles']:
        for layer in profile['verification_layers']:
            if layer in layer_profiles:
                errors.append(f'verification layer maps to multiple profiles: {layer}')
            layer_profiles[layer]=profile
except Exception as exc:
    errors.append(f'evidence verification profile contract invalid: {exc}')
    layer_profiles={}
for i,r in enumerate(rows,2):
    rid=r.get('requirement_id',f'row{i}')
    for k in ['severity','scope','primary_phase','component','acceptance_summary','verification_id','verification_layer','release_gate','status','evidence_classification','evidence_disposition']:
        if not r.get(k,'').strip(): errors.append(f'{rid}: blank {k}')
    if r.get('status') not in valid_status: errors.append(f'{rid}: invalid status {r.get("status")}')
    if r.get('evidence_classification') not in valid_evidence_classification: errors.append(f'{rid}: invalid evidence_classification {r.get("evidence_classification")}')
    if r.get('evidence_disposition') not in valid_evidence_disposition: errors.append(f'{rid}: invalid evidence_disposition {r.get("evidence_disposition")}')
    if r.get('release_gate') not in {'required','roadmap'}: errors.append(f'{rid}: invalid release_gate')
    if r.get('release_gate')=='required' and r.get('primary_phase') not in release_phases: errors.append(f'{rid}: required requirement primary_phase must be a release phase')
    if r.get('release_gate')=='roadmap' and r.get('primary_phase') not in roadmap_phases: errors.append(f'{rid}: roadmap requirement primary_phase must be a roadmap phase')
    if r.get('release_gate')=='roadmap' and r.get('scope')!='post-1.0': errors.append(f'{rid}: roadmap row must be post-1.0')
    if r.get('status')=='passed' and not r.get('evidence','').strip(): errors.append(f'{rid}: passed without evidence')
    if r.get('status')=='passed' and r.get('evidence_disposition')!='qualified': errors.append(f'{rid}: passed evidence must be qualified')
    if r.get('release_gate')=='required' and r.get('evidence_disposition')=='not-applicable': errors.append(f'{rid}: release-gated requirement cannot have not-applicable evidence')
    if r.get('release_gate')=='roadmap' and r.get('evidence_disposition')!='not-applicable': errors.append(f'{rid}: roadmap requirement must have not-applicable evidence')
    profile=layer_profiles.get(r.get('verification_layer'))
    if profile is None:
        errors.append(f'{rid}: verification_layer has no reviewed evidence profile')
    elif r.get('release_gate')=='required' and profile.get('qualifiable') is not True:
        errors.append(f'{rid}: required requirement maps to a non-qualifiable evidence profile')
if errors:
    print('Traceability validation FAILED'); [print(' -',e) for e in errors]; sys.exit(1)
print(f'Traceability OK: {len(req_ids)} catalog requirements mapped exactly once; {sum(r["release_gate"]=="required" for r in rows)} Linux 1.0 gated, {sum(r["release_gate"]=="roadmap" for r in rows)} roadmap.')
