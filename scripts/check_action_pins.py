#!/usr/bin/env python3
from __future__ import annotations
import re,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
errors=[]; uses=0
for p in sorted((ROOT/'.github/workflows').glob('*.y*ml')):
    for n,line in enumerate(p.read_text().splitlines(),1):
        m=re.search(r'\buses:\s*["\']?([^\s"\']+)',line)
        if not m: continue
        ref=m.group(1); uses+=1
        if ref.startswith('./') or ref.startswith('docker://'): continue
        if '@' not in ref:
            errors.append(f'{p.relative_to(ROOT)}:{n}: remote action has no @ ref: {ref}'); continue
        target=ref.rsplit('@',1)[1]
        if not re.fullmatch(r'[0-9a-fA-F]{40}',target):
            errors.append(f'{p.relative_to(ROOT)}:{n}: remote action is not pinned to a 40-hex commit: {ref}')
if errors:
    print('Action pin validation FAILED'); [print(' -',e) for e in errors]; sys.exit(1)
print(f'Action pins OK: checked {uses} uses entries.')
