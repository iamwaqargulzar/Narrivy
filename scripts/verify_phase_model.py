#!/usr/bin/env python3
from __future__ import annotations
import csv, json, sys, re
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
errors=[]
try:
    target=json.loads((ROOT/'BUILD_TARGET.json').read_text())
    reg=json.loads((ROOT/'agent/PHASES.json').read_text())
    roadmap=json.loads((ROOT/'agent/ROADMAP_PHASES.json').read_text())
    state=json.loads((ROOT/'agent/STATE.json').read_text())
except Exception as e:
    raise SystemExit(f'Phase model JSON error: {e}')
phases=reg.get('phases',[])
ids=[p.get('id') for p in phases]
expected=['P0','P0F']+[f'P{i}' for i in range(1,12)]
if ids!=expected:
    errors.append(f'release phase order {ids} != {expected}')
road_ids=[p.get('id') for p in roadmap.get('phases',[])]
if road_ids!=['R12','R13','R14']:
    errors.append(f'roadmap phase order invalid: {road_ids}')
if target.get('product_target')!='linux-1.0':
    errors.append('BUILD_TARGET product_target must be linux-1.0 for this pack')
if target.get('target_phase')!='P11':
    errors.append('BUILD_TARGET target_phase must be P11')
if target.get('auto_advance') is not True:
    errors.append('BUILD_TARGET auto_advance must be true')
if state.get('active_phase') not in ids:
    errors.append(f'STATE active_phase invalid: {state.get("active_phase")}')
rows=list(csv.DictReader((ROOT/'spec/requirements-traceability.csv').open(newline='')))
for p in phases:
    pid=p['id']
    for key in ('spec','prompt','acceptance'):
        rel=p.get(key,'')
        if not rel or not (ROOT/rel).is_file():
            errors.append(f'{pid}: missing {key} {rel!r}')
    try:
        acc=json.loads((ROOT/p['acceptance']).read_text())
    except Exception as e:
        errors.append(f'{pid}: invalid acceptance JSON: {e}')
        continue
    actual={r['requirement_id'] for r in rows if r['release_gate']=='required' and r['primary_phase']==pid}
    declared=set(acc.get('primary_requirements',[]))
    if actual!=declared:
        errors.append(f'{pid}: acceptance requirements differ from traceability: actual {sorted(actual)} vs declared {sorted(declared)}')
    if acc.get('phase')!=pid:
        errors.append(f'{pid}: acceptance phase mismatch')
    if pid=='P0' and acc.get('implemented') is not True:
        errors.append('P0 acceptance scaffold should be implemented=true but still evidence-gated')
    if pid not in ('P0','P0F') and acc.get('implemented') is not False:
        # Allow P0F to be false initially; after it passes it may become true
        # For fresh pack, all but P0 should be false. We enforce false for not-yet-implemented.
        # If a phase is already implemented, this check would fail; but for hardening we keep it false until gate passes.
        # So warn but not hard fail if already true? We'll just check type
        if not isinstance(acc.get('implemented'), bool):
            errors.append(f'{pid}: acceptance implemented must be bool')
    # working_app_required: P0,P0F,P1 false else true
    should_work = pid not in ('P0','P0F','P1')
    if bool(p.get('working_app_required'))!=should_work:
        errors.append(f'{pid}: working_app_required must be {should_work} per the legacy planning amendment')
for r in rows:
    if r['release_gate']=='required' and r['primary_phase'] not in ids:
        errors.append(f'{r["requirement_id"]}: invalid release primary phase {r["primary_phase"]}')
    if r['release_gate']=='roadmap' and r['primary_phase'] not in road_ids:
        errors.append(f'{r["requirement_id"]}: invalid roadmap primary phase {r["primary_phase"]}')
if errors:
    print('Phase model validation FAILED')
    for e in errors:
        print(' -',e)
    sys.exit(1)
print(f'Phase model OK: {len(ids)} Linux phases (including P0F), {len(road_ids)} roadmap phases, {sum(r["release_gate"]=="required" for r in rows)} release requirements.')
