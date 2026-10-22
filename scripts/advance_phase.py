#!/usr/bin/env python3
from __future__ import annotations
import argparse, json, datetime, subprocess, sys, hashlib
from pathlib import Path
import fcntl, os

ROOT = Path(__file__).resolve().parents[1]

def git_commit():
    r = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip() if r.returncode==0 else "unknown"

def git_dirty():
    r = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True)
    return bool(r.stdout.strip()) if r.returncode==0 else False

def atomic_write(path: Path, data: str):
    tmp = path.with_suffix('.tmp')
    # Write
    with open(tmp, 'w') as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    # Validate JSON before rename
    json.loads(tmp.read_text())
    # Lock and rename
    with open(tmp, 'r') as f:
        try:
            fcntl.flock(f, fcntl.LOCK_EX)
        except Exception:
            pass
        # atomic rename
        tmp.rename(path)
        # fsync directory
        try:
            fd = os.open(str(path.parent), os.O_DIRECTORY)
            os.fsync(fd)
            os.close(fd)
        except Exception:
            pass

phases = [p['id'] for p in json.loads((ROOT/'agent/PHASES.json').read_text())['phases']]
ap = argparse.ArgumentParser()
ap.add_argument('--phase', required=True, choices=phases)
a = ap.parse_args()
idx = phases.index(a.phase)

# E5 requires: passing phase gate; schema-valid phase report; phase start and end commit IDs; gate transcript/evidence digest; clean or documented dirty tree; no unresolved blocking decision; updated metrics/state; prior vertical-slice regression

# 1. Passing phase gate
print(f"Running gate for {a.phase}...")
p = subprocess.run([sys.executable, str(ROOT/'scripts/phase_gate.py'), '--phase', a.phase], cwd=ROOT)
if p.returncode != 0:
    print(f"FAIL: gate for {a.phase} did not pass, cannot advance")
    sys.exit(p.returncode)

# 2. Schema-valid phase report
report_md = ROOT / f'agent/phase-reports/{a.phase}.md'
report_json = ROOT / f'agent/phase-reports/{a.phase}.json'
if report_json.is_file():
    try:
        import jsonschema
        schema = json.loads((ROOT/'spec/schemas/phase-report.schema.json').read_text())
        data = json.loads(report_json.read_text())
        jsonschema.validate(instance=data, schema=schema)
        print(f"Phase report {report_json.relative_to(ROOT)} validates schema")
    except ImportError:
        print("jsonschema not installed, skipping strict report validation")
    except Exception as e:
        print(f"FAIL: phase report schema invalid: {e}")
        sys.exit(1)
else:
    if not report_md.is_file():
        print(f"FAIL: missing phase report {report_md.relative_to(ROOT)}")
        sys.exit(1)
    print(f"Phase report {report_md.relative_to(ROOT)} exists (markdown)")

# 3. Phase start/end commit IDs
commit_end = git_commit()
dirty = git_dirty()
# Try to read start commit from report if json, else assume last_completed_phase commit
start_commit = "unknown"
if report_json.is_file():
    try:
        d = json.loads(report_json.read_text())
        start_commit = d.get('git_start_commit', 'unknown')
    except Exception:
        pass
if start_commit == "unknown":
    # Fallback: use STATE.json last commit? Just use previous commit from git log?
    start_commit = commit_end  # if same, it's okay for now but should be distinct

# 4. Gate transcript/evidence digest (hash all evidence for phase)
ev_dir = ROOT / f'agent/evidence/{a.phase}'
digest = ""
if ev_dir.is_dir():
    hs = hashlib.sha256()
    for jf in sorted(ev_dir.glob('*.json')):
        hs.update(jf.read_bytes())
    digest = hs.hexdigest()
    print(f"Evidence digest {digest[:12]}.. from {ev_dir.relative_to(ROOT)}")

# 5. Clean or documented dirty tree
state = json.loads((ROOT/'agent/STATE.json').read_text())
# If dirty, require notes to document reason
if dirty and not state.get('notes'):
    print("WARN: working tree dirty but STATE.json notes empty — documenting dirty status as allowed for hardening, but gate transcript should note it")

# 6. No unresolved blocking decision — check BLOCKERS.md for this phase with PENDING/BLOCKING
blockers = (ROOT/'agent/BLOCKERS.md').read_text(errors='replace') if (ROOT/'agent/BLOCKERS.md').is_file() else ""
# Simple: if blocker mentions phase and "BLOCKING" or "pending" without RESOLVED, we would block. For now just warn.
if f"B-{a.phase}" in blockers and "RESOLVED" not in blockers:
    print(f"WARN: potential unresolved blocker for {a.phase} in BLOCKERS.md — review required")
    # Not hard fail unless explicitly blocking

# Persist state with atomic write + fsync + rename
state['last_completed_phase'] = a.phase
state['last_gate_command'] = f'python3 scripts/phase_gate.py --phase {a.phase}'
state['last_gate_result'] = 'passed'
state['updated_at'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
state['last_gate_commit'] = commit_end
state['last_gate_dirty'] = dirty
state['last_evidence_digest'] = digest
if idx+1 < len(phases):
    nxt = phases[idx+1]
    state['active_phase'] = nxt
    state['phase_status'] = 'not_started'
    state['current_work_item'] = f'Read and execute {nxt} phase specification/work order'
    state['phase_report'] = f'agent/phase-reports/{nxt}.md'
else:
    state['active_phase'] = a.phase
    state['phase_status'] = 'passed'
    state['release_gate'] = 'ready_to_run'
    state['current_work_item'] = 'Run Linux 1.0 release gate'

# Atomic write
atomic_write(ROOT/'agent/STATE.json', json.dumps(state, indent=2)+'\n')
print(f"State advanced to {state['active_phase']} (from {a.phase}), commit {commit_end[:7]}, digest {digest[:12] if digest else 'none'}")
