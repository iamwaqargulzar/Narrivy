#!/usr/bin/env python3
from __future__ import annotations
import hashlib,json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
out=ROOT/'BUILD-PACK-MANIFEST.json'
EXCLUDE_DIRS = {"target", "node_modules", "dist", ".vite", "__pycache__", ".pytest_cache", "artifacts", "code"}
def is_excluded(rel: str) -> bool:
    parts = Path(rel).parts
    return any(part in EXCLUDE_DIRS for part in parts)
files=[]
for p in sorted(x for x in ROOT.rglob('*') if x.is_file() and x != out and '.git' not in x.parts and not is_excluded(x.relative_to(ROOT).as_posix())):
    b=p.read_bytes()
    files.append({'path':p.relative_to(ROOT).as_posix(),'bytes':len(b),'sha256':hashlib.sha256(b).hexdigest()})
data={
  'schema_version':3,
  'pack':'Narrivy Full Autonomous Build Pack — v1.3 Scaffold',
  'pack_version':'1.3',
  'generated_date':'2026-08-22',
  'manifest_excludes_self':True,
  'file_count':len(files),
  'files':files
}
out.write_text(json.dumps(data,indent=2)+'\n')
print(f'Wrote {out.name}: {len(files)} files (manifest excludes itself).')
