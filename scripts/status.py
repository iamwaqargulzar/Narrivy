#!/usr/bin/env python3
from __future__ import annotations
import csv,json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
target=json.loads((ROOT/'BUILD_TARGET.json').read_text()); state=json.loads((ROOT/'agent/STATE.json').read_text()); reg=json.loads((ROOT/'agent/PHASES.json').read_text())
pid=state['active_phase']; phase=next(p for p in reg['phases'] if p['id']==pid)
rows=list(csv.DictReader((ROOT/'spec/requirements-traceability.csv').open(newline='')))
owned=[r for r in rows if r['primary_phase']==pid and r['release_gate']=='required']
print(f"Target: {target['product_target']} through {target['target_phase']}")
print(f"Active: {pid} — {phase['name']} ({state['phase_status']})")
print(f"Spec: {phase['spec']}\nPrompt: {phase['prompt']}\nReport: agent/phase-reports/{pid}.md\nGate: {phase['gate']}")
print('Requirements:')
for r in owned: print(f" - {r['requirement_id']}: {r['status']}" + (f" -> {r['evidence']}" if r['evidence'] else ''))
