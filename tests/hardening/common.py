"""common — shared helpers for P9 failure-injection harnesses.

Portable, no unwrap/panic: typed errors via exceptions, explicit boundaries.
No transcript/audio/clipboard capture.
"""
from __future__ import annotations
import contextlib
import hashlib
import json
import os
import resource
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Iterator

ROOT = Path(__file__).resolve().parents[2]

# ---------------------------------------------------------------------------
# Sanitization
# ---------------------------------------------------------------------------

TRANSCRIPT_NEEDLES = ["hello world from mock asr", "Hello world from mock asr."]
SECRET_NEEDLES = ["token", "secret"]

def assert_no_transcript_leak(text: str) -> None:
    lowered = text.lower()
    for needle in TRANSCRIPT_NEEDLES:
        assert needle.lower() not in lowered, f"transcript leak detected: {needle!r} in output"

def redacted_excerpt(text: str, limit: int = 3900) -> str:
    # naive redaction: ensure needles not present; real code uses narrivy-diagnostics sanitize
    out = text[:limit]
    for needle in TRANSCRIPT_NEEDLES:
        out = out.replace(needle, "[redacted]")
    return out

# ---------------------------------------------------------------------------
# Temp dirs / journal helpers (tmp→fsync→rename→fsync-dir pattern)
# ---------------------------------------------------------------------------

def mk_runtime_tmp(prefix: str = "p9-harness-") -> tempfile.TemporaryDirectory:
    return tempfile.TemporaryDirectory(prefix=prefix)

def journal_dir(state_home: Path) -> Path:
    """Future real-daemon tests must pass an isolated XDG_STATE_HOME."""
    return state_home / "narrivy" / "journal"

def write_journal_entry_atomic(jdir: Path, entry_id: str, payload: dict) -> Path:
    jdir.mkdir(parents=True, exist_ok=True)
    tmp = jdir / f".{entry_id}.tmp"
    final = jdir / f"{entry_id}.json"
    tmp.write_text(json.dumps(payload))
    # fsync file
    with open(tmp, "rb") as f:
        try:
            os.fsync(f.fileno())
        except OSError:
            pass
    tmp.rename(final)
    # fsync dir
    try:
        dfd = os.open(jdir, os.O_DIRECTORY)
        try:
            os.fsync(dfd)
        finally:
            os.close(dfd)
    except OSError:
        pass
    return final

def corrupt_tail(path: Path, keep_bytes: int) -> None:
    data = path.read_bytes()
    path.write_bytes(data[:keep_bytes])

def quarantine_corrupt(jdir: Path) -> list[Path]:
    quarantined = []
    for p in jdir.glob("*.json"):
        try:
            json.loads(p.read_text())
        except Exception:
            q = p.with_suffix(".json.corrupt")
            try:
                p.rename(q)
                quarantined.append(q)
            except OSError:
                pass
    # stray .tmp files are left / ignored
    return quarantined

def recoverable_entries(jdir: Path) -> list[dict]:
    out = []
    for p in sorted(jdir.glob("*.json")):
        if p.suffixes[-1] == ".corrupt":
            continue
        if p.name.startswith("."):
            continue
        try:
            data = json.loads(p.read_text())
            # minimal TTL / delivered filter: skip delivered==True in stub
            if data.get("delivered"):
                continue
            out.append(data)
        except Exception:
            continue
    # sorted by time if present
    out.sort(key=lambda d: d.get("created_at", ""))
    return out

# ---------------------------------------------------------------------------
# FD exhaustion helper
# ---------------------------------------------------------------------------

@contextlib.contextmanager
def fd_pressure(limit: int = 32) -> Iterator[list[int]]:
    """Clamp RLIMIT_NOFILE soft to `limit` and open fds to near-limit.

    Yields list of held fds. Restores limit on exit. Never raises on restore.
    """
    try:
        soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    except Exception:
        yield []
        return
    target_soft = min(limit, hard) if hard != resource.RLIM_INFINITY else limit
    held: list[int] = []
    old_soft = soft
    try:
        try:
            resource.setrlimit(resource.RLIMIT_NOFILE, (target_soft, hard))
        except Exception:
            pass
        # open until EMFILE
        for _ in range(target_soft + 16):
            try:
                fd = os.open(os.devnull, os.O_RDONLY)
                held.append(fd)
            except OSError:
                break
        yield held
    finally:
        for fd in held:
            try:
                os.close(fd)
            except OSError:
                pass
        try:
            resource.setrlimit(resource.RLIMIT_NOFILE, (old_soft, hard))
        except Exception:
            pass

def count_open_fds() -> int:
    try:
        return len(os.listdir("/proc/self/fd"))
    except OSError as e:
        # under EMFILE, /proc/self/fd listing itself fails; return best-effort
        if e.errno == 24:
            try:
                soft, _ = resource.getrlimit(resource.RLIMIT_NOFILE)
                return soft
            except Exception:
                return -1
        return -1
    except Exception:
        return -1

# ---------------------------------------------------------------------------
# OOM / cgroup stub helper
# ---------------------------------------------------------------------------

def cgroup_v2_available() -> bool:
    return Path("/sys/fs/cgroup/cgroup.controllers").exists()

def oom_stub_limits() -> dict:
    """Return stub limits for OOM simulation (no host OOM)."""
    return {
        "cgroup_available": cgroup_v2_available(),
        "rlimit_as_bytes": 64 * 1024 * 1024,  # 64 MiB stub
        "note": "stub does not kill host; uses RLIMIT_AS + allocation probe",
    }

@contextlib.contextmanager
def rlimit_as_stub(limit_bytes: int = 64 * 1024 * 1024) -> Iterator[None]:
    try:
        soft, hard = resource.getrlimit(resource.RLIMIT_AS)
    except Exception:
        yield
        return
    try:
        resource.setrlimit(resource.RLIMIT_AS, (limit_bytes, hard))
        yield
    finally:
        try:
            resource.setrlimit(resource.RLIMIT_AS, (soft, hard))
        except Exception:
            pass

def probe_allocation_fails(limit_bytes: int = 64 * 1024 * 1024) -> bool:
    """Try allocating > limit and expect failure under RLIMIT_AS."""
    with rlimit_as_stub(limit_bytes):
        try:
            # try to allocate ~2x limit via bytearray
            _ = bytearray(limit_bytes * 2)
            return False
        except MemoryError:
            return True
        except Exception:
            return True

# ---------------------------------------------------------------------------
# Process kill -9 helper
# ---------------------------------------------------------------------------

def spawn_sleep_process(extra_env: dict | None = None) -> subprocess.Popen:
    env = dict(os.environ, **(extra_env or {}))
    # use sleep as stand-in for migration; real harness spawns narrivyd with migration flag
    return subprocess.Popen(["sleep", "30"], env=env)

def kill9_and_reap(proc: subprocess.Popen, timeout: float = 5.0) -> dict:
    info: dict = {"pid": proc.pid, "killed": False, "exit_code": None}
    try:
        proc.kill()  # SIGKILL
        info["killed"] = True
    except OSError as e:
        info["error"] = str(e)
    try:
        proc.wait(timeout=timeout)
        info["exit_code"] = proc.returncode
    except subprocess.TimeoutExpired:
        info["error"] = "wait timeout"
    return info

def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()

# ---------------------------------------------------------------------------
# Generic harness result
# ---------------------------------------------------------------------------

def harness_result(name: str, passed: bool, metrics: dict, notes: str, diagnostic: str) -> dict:
    return {
        "harness": name,
        "pass": passed,
        "metrics": metrics,
        "notes": notes[:2000],
        "diagnostic_excerpt": redacted_excerpt(diagnostic),
    }
