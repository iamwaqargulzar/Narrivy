"""test_p9_hardening_skeleton — pytest smoke for P9 harnesses (stub mode).

Runs each harness in stub mode, asserts pass, no leaks, type checks.
This is the skeleton gate; real-daemon exhaustive tests replace/enhance it.
"""
import importlib
import sys
from pathlib import Path

try:
    import pytest
except ImportError:
    pytest = None  # skeleton runs without pytest via direct python

# allow `tests/hardening` as package
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from tests.hardening.harness_kill_mid_migration import run as run_kill
from tests.hardening.harness_journal_torn_write import run as run_torn
from tests.hardening.harness_fd_exhaustion import run as run_fd
from tests.hardening.harness_oom_cgroup_stub import run as run_oom

def test_kill_mid_migration_stub():
    res = run_kill(stub=True)
    assert res["pass"], f"kill harness failed: {res}"
    assert "survivor" in res["notes"].lower() or "quarantined" in res["diagnostic_excerpt"].lower()
    assert res["metrics"]["survivor_count"] == 3

def test_journal_torn_write_stub():
    res = run_torn(stub=True)
    assert res["pass"], f"torn harness failed: {res}"
    assert res["metrics"]["survivor_count"] == 7  # 10-3 default
    assert res["metrics"]["quarantined_count"] == 3

def test_journal_torn_write_full_proof_counts():
    res = run_torn(stub=True, entries=100, torn=15)
    assert res["pass"], f"full proof counts failed: {res}"
    assert res["metrics"]["survivor_count"] == 85
    assert res["metrics"]["quarantined_count"] == 15

def test_fd_exhaustion_stub():
    res = run_fd(stub=True)
    assert res["pass"], f"fd harness failed: {res}"
    assert res["metrics"]["survivor_count"] == 1

def test_oom_cgroup_stub():
    res = run_oom(stub=True)
    assert res["pass"], f"oom harness failed: {res}"
    # either rlimit rejected or fake cgroup writable
    assert res["metrics"]["allocation_rejected"] or res["metrics"]["cgroup_stub"]["writable"]

def test_no_transcript_leak_in_all():
    for fn in (run_kill, run_torn, run_fd, run_oom):
        res = fn(stub=True)
        text = (res.get("diagnostic_excerpt") or "") + " " + (res.get("notes") or "")
        lowered = text.lower()
        assert "hello world from mock asr" not in lowered
