#!/usr/bin/env python3
"""Validate Narrivy Pipeline examples beyond JSON Schema (read-only)."""
from __future__ import annotations
import json,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
try:
    import jsonschema
except Exception as e:
    raise SystemExit(f"jsonschema required: {e}")
schema=json.loads((ROOT/'spec/schemas/pipeline.schema.json').read_text())
errors=[]
examples=sorted((ROOT/'spec/examples').glob('pipeline-*.json'))
if not examples: errors.append('no pipeline examples')
for p in examples:
    try:
        d=json.loads(p.read_text()); jsonschema.validate(d,schema)
    except Exception as e:
        errors.append(f'{p.name}: schema: {e}'); continue
    ids=[n['id'] for n in d['nodes']]
    if len(ids)!=len(set(ids)): errors.append(f'{p.name}: duplicate node id')
    idset=set(ids)
    tids=[t['id'] for t in d.get('triggers',[])]
    if len(tids)!=len(set(tids)): errors.append(f'{p.name}: duplicate trigger id')
    adj={x:[] for x in ids}; indeg={x:0 for x in ids}
    for e in d['edges']:
        a,b=e['from'],e['to']
        if a not in idset or b not in idset:
            errors.append(f'{p.name}: dangling edge {a}->{b}'); continue
        adj[a].append(b); indeg[b]+=1
    q=[x for x,v in indeg.items() if v==0]; seen=0
    while q:
        x=q.pop(); seen+=1
        for y in adj[x]:
            indeg[y]-=1
            if indeg[y]==0:q.append(y)
    if seen!=len(ids): errors.append(f'{p.name}: graph contains cycle')
    pol=d['policy']
    if pol['privacy']=='local_only':
        if pol.get('send_transcript_remote') or pol.get('send_context_remote'):
            errors.append(f'{p.name}: local_only cannot allow remote data')
        if any(n.get('execution')=='remote_allowed' for n in d['nodes']):
            errors.append(f'{p.name}: local_only contains remote_allowed node')
    for n in d['nodes']:
        prefs=n.get('device_preference',[])
        if prefs and len(prefs)!=len(set(prefs)): errors.append(f'{p.name}:{n["id"]}: duplicate device preference')
    # Singleton capability families: at most one instance per pipeline (pipeline-engine.md § Node multiplicity)
    SINGLETONS={'audio.capture','speech.asr','speech.endpoint','context.provider','delivery.direct','quality.qualifier'}
    from collections import Counter
    cap_counts=Counter(n['capability'] for n in d['nodes'])
    for cap,cnt in cap_counts.items():
        if cap in SINGLETONS and cnt>1:
            errors.append(f'{p.name}: singleton capability {cap} has {cnt} instances')
    # Trigger-binding conflicts within a pipeline (cross-pipeline arbitration is the P1 trigger-router's job)
    bindings=[t.get('binding') for t in d.get('triggers',[]) if t.get('binding') and t.get('enabled',True)]
    if len(bindings)!=len(set(bindings)):
        errors.append(f'{p.name}: duplicate enabled trigger binding')
# Taxonomy integrity: every node capability must be a canonical catalog slot
catalog=json.loads((ROOT/'spec/catalogs/modules.json').read_text())
catalog_caps={m['capability'] for m in catalog['modules']}
catalog_mods={m['id'] for m in catalog['modules']}
for p2 in examples:
    d2=json.loads(p2.read_text())
    for n2 in d2['nodes']:
        if n2['capability'] not in catalog_caps:
            errors.append(f'{p2.name}:{n2["id"]}: capability {n2["capability"]} not in spec/catalogs/modules.json')
        for fmid in n2.get('fallback_modules',[]):
            if fmid not in catalog_mods:
                errors.append(f'{p2.name}:{n2["id"]}: fallback module {fmid} not in spec/catalogs/modules.json')
# Specific product invariants for canonical examples
fast=json.loads((ROOT/'spec/examples/pipeline-fast.json').read_text())
caps={n['capability'] for n in fast['nodes']}
needed={'audio.capture','audio.normalize','audio.denoise','speech.vad','speech.endpoint','speech.asr','text.punctuation','text.itn','text.semantic-guard','delivery.direct'}
if not needed.issubset(caps): errors.append('pipeline-fast missing lean default capabilities')
asr=next((n for n in fast['nodes'] if n['capability']=='speech.asr'),None)
if not asr or asr.get('config',{}).get('gpu_api_preference')!='vulkan': errors.append('pipeline-fast must prefer Vulkan general GPU execution where qualified')
if not asr or asr.get('device_preference')!=['npu','dgpu','igpu','cpu']: errors.append('pipeline-fast ASR device preference must be NPU→dGPU→iGPU→CPU')
smart=json.loads((ROOT/'spec/examples/pipeline-smart-local.json').read_text())
scaps={n['capability'] for n in smart['nodes']}
if not {'text.llm-postprocess','text.semantic-guard'}.issubset(scaps): errors.append('pipeline-smart-local missing LLM+semantic guard')
if errors:
    print('Pipeline contract validation FAILED')
    for e in errors: print(' -',e)
    sys.exit(1)
print(f'Pipeline contract validation OK: {len(examples)} examples')
