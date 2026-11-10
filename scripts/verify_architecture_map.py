#!/usr/bin/env python3
"""Architecture-map consistency guard.

Owner decision 2026-08-22 (FINAL): the whisper.cpp RUNTIME is removed from
Narrivy entirely - no runtime entry, no bindings, no plans. Whisper MODEL
WEIGHTS remain in the catalog and are executed via transcribe.cpp on
CPU/iGPU/dGPU. OpenVINO device support is model-specific; Qwen3-ASR on NPU
remains candidate-only until an exact tuple passes native qualification.

This guard enforces BOTH directions of that decision:
  presence : qwen3-asr reference engine, qwen3-asr-openvino (linux-1.0),
             whisper + parakeet families executed through transcribe.cpp,
             the transcribe.cpp runtime itself;
  absence  : any whisper.cpp runtime/binding remnants.
"""
from pathlib import Path
import json,csv,sys
R=Path(__file__).resolve().parents[1]; C=R/'spec/catalogs'; errors=[]
def load(n): return json.loads((C/n).read_text())
features=load('features.json')['features']; pipes=load('pipelines.json')['pipelines']; engines=load('engines.json')['engines']; mods=load('modules.json')['modules']; runtimes=load('runtimes.json')['runtimes']
req=[r['requirement_id'] for r in csv.DictReader((R/'spec/requirements-traceability.csv').open())]
mapped=[x for f in features for x in f['requirements']]
if sorted(mapped)!=sorted(req): errors.append('feature map does not cover requirements exactly')
if len(mapped)!=len(set(mapped)): errors.append('duplicate requirement ownership in feature map')
mods_by_cap={}
for m in mods: mods_by_cap.setdefault(m['capability'],[]).append(m)
for p_ in pipes:
  for stage in p_['stages']:
    if not any(c in mods_by_cap for c in stage.rstrip('?').split('|')): errors.append(f"{p_['id']}: no module for stage {stage}")

# --- ASR contract (owner decision 2026-08-22) ---
asr_ref=[e for e in engines if e['id']=='qwen3-asr']
if not asr_ref: errors.append('reference ASR engine qwen3-asr missing')
elif asr_ref[0].get('status')!='reference': errors.append('qwen3-asr must be the reference ASR engine')
if not any(r['id']=='transcribe.cpp' for r in runtimes): errors.append('runtime transcribe.cpp missing')
for mid in ['qwen3-asr-openvino','parakeet-transcribecpp','gguf-whisper-models-transcribecpp']:
  if not any(m['id']==mid for m in mods): errors.append(f'{mid} binding missing')
qo=[m for m in mods if m['id']=='qwen3-asr-openvino']
if qo and (qo[0].get('status')!='linux-1.0'): errors.append('qwen3-asr-openvino must remain in the linux-1.0 module catalog; device support is qualification-derived')
gw=[m for m in mods if m['id']=='gguf-whisper-models-transcribecpp']
if gw and gw[0].get('runtime')!='transcribe.cpp': errors.append('Whisper MODEL weights must execute via transcribe.cpp')

# --- Revocation guard: Whisper exists ONLY as model weights, never as an engine/runtime entity ---
blob=json.dumps({'engines':engines,'modules':mods,'runtimes':runtimes}).lower()
if 'whisper.cpp' in blob or 'whisper-cpp' in blob: errors.append('revoked runtime remnant present: whisper.cpp')
for bad in ('whisper-cpp','whisper-openvino','whisper-onnx'):
  if any(m['id']==bad for m in mods): errors.append(f'revoked binding present: {bad}')
if any(e['id']=='whisper' for e in engines): errors.append('revoked ENGINE entity present: whisper (model weights live under the gguf-whisper-models binding only)')
if 'whisper' not in blob: errors.append('Whisper MODEL variants must remain referenced (via gguf-whisper-models binding / transcribe.cpp description)')
knownr={r['id'] for r in runtimes}|{'provider','remote','pipewire','fst','native-rust','openvino/onnxruntime/llama.cpp'}
# Runtime fields may list composite choices; verify each simple single runtime.
for m in mods:
  if '/' not in m['runtime'] and m['runtime'] not in knownr: errors.append(f"{m['id']}: unknown runtime {m['runtime']}")
if errors:
 print('Architecture map FAILED'); [print(' -',e) for e in errors]; sys.exit(1)
print(f'Architecture map OK: {len(features)} feature groups / {len(req)} requirements, {len(pipes)} pipelines, {len(engines)} engines, {len(mods)} modules, {len(runtimes)} runtimes.')
