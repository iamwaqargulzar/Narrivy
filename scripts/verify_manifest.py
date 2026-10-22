#!/usr/bin/env python3
from __future__ import annotations
import hashlib,json,sys,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'BUILD-PACK-MANIFEST.json'
try: m=json.loads(p.read_text())
except Exception as e: raise SystemExit(f'Invalid manifest: {e}')
# Exclude build artifacts and caches from manifest verification (they are not part of pack)
EXCLUDE_DIRS = {"target", "node_modules", "dist", ".vite", "__pycache__", ".pytest_cache", "artifacts", "code"}
EXCLUDE_FILES = set()
def is_excluded(rel: str) -> bool:
    parts = Path(rel).parts
    return any(part in EXCLUDE_DIRS for part in parts)
actual={x.relative_to(ROOT).as_posix():x for x in ROOT.rglob('*') if x.is_file() and x!=p and '.git' not in x.parts and not is_excluded(x.relative_to(ROOT).as_posix())}
listed={x['path']:x for x in m.get('files',[])}
# Filter listed similarly (in case old manifest had target entries, but it shouldn't)
listed = {k:v for k,v in listed.items() if not is_excluded(k)}
errors=[]
if m.get('file_count')!=len(listed): errors.append(f'manifest file_count {m.get("file_count")} != entries {len(listed)}')
for rel in sorted(set(actual)-set(listed)): errors.append(f'unlisted file: {rel}')
for rel in sorted(set(listed)-set(actual)): errors.append(f'missing shipped file: {rel}')
for rel in sorted(set(actual)&set(listed)):
    b=actual[rel].read_bytes(); row=listed[rel]
    if len(b)!=row.get('bytes'): errors.append(f'size mismatch: {rel}')
    if hashlib.sha256(b).hexdigest()!=row.get('sha256'): errors.append(f'hash mismatch: {rel}')
if errors:
    print('Manifest verification FAILED')
    for error in errors:
        print(' -', error)
    print('Validation is read-only; run scripts/build_manifest.py only as an explicit, separately reviewed packaging action.')
    sys.exit(1)
print(f'Manifest OK: {len(actual)} files verified.')
