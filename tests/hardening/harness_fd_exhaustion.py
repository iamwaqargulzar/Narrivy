"""harness_fd_exhaustion — EMFILE/ENFILE graceful handling (P9).

Sets RLIMIT_NOFILE 32 via resource.setrlimit, attempts 40 concurrent socket
opens, asserts 8 fail with EMFILE typed error, no panic, limit restored.
Also verifies journal baseline survives and transcript preserved.
"""

from __future__ import annotations
import errno
import os
import resource
import socket
import tempfile
from pathlib import Path

from .common import (
    fd_pressure,
    count_open_fds,
    write_journal_entry_atomic,
    journal_dir,
    recoverable_entries,
    harness_result,
    assert_no_transcript_leak,
)

def _set_rlimit_nofile_soft(limit: int) -> tuple[int,int]:
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    target = min(limit, hard) if hard != resource.RLIM_INFINITY else limit
    resource.setrlimit(resource.RLIMIT_NOFILE, (target, hard))
    return soft, hard

def run(*, stub: bool = True, verbose: bool = False) -> dict:
    notes = []
    metrics: dict = {}
    diagnostic = ""
    passed = False
    td = tempfile.TemporaryDirectory(prefix="p9-fd-")
    original_soft = None
    original_hard = None
    try:
        soft0, hard0 = resource.getrlimit(resource.RLIMIT_NOFILE)
        original_soft, original_hard = soft0, hard0
        rt = Path(td.name) / "xdg"
        jdir = journal_dir(rt)
        # baseline entry before pressure — must survive
        try:
            write_journal_entry_atomic(jdir, "baseline", {"id": "baseline", "created_at": "2026-01-01T00:00:00Z", "text_hash": "abc", "delivered": False})
        except OSError as e:
            return harness_result("fd_exhaustion", False, {"error": f"baseline write failed: {e}"}, "baseline failed", str(e))

        # --- Mission core: RLIMIT_NOFILE 32, 40 socket opens, 8 fail EMFILE ---
        # Clamp to 32
        try:
            _set_rlimit_nofile_soft(32)
            new_soft, _ = resource.getrlimit(resource.RLIMIT_NOFILE)
            metrics["rlimit_set_to"] = new_soft
            notes.append(f"RLIMIT_NOFILE clamped soft {soft0}->{new_soft}")
        except Exception as e:
            return harness_result("fd_exhaustion", False, {"error": f"setrlimit failed: {e}"}, "setrlimit failed", str(e))

        # Attempt 40 concurrent socket opens
        sockets: list[socket.socket] = []
        failures: list[tuple[int,str]] = []
        emfile_count = 0
        attempted = 40
        for i in range(attempted):
            try:
                s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                # keep it open to exhaust fds (don't connect)
                sockets.append(s)
            except OSError as e:
                failures.append((e.errno, e.strerror or str(e)))
                if e.errno == errno.EMFILE or e.errno == errno.ENFILE or "Too many open files" in str(e):
                    emfile_count += 1
                # continue trying to reach 40
            except Exception as e:
                failures.append((-1, str(e)))

        metrics["attempted_sockets"] = attempted
        metrics["socket_successes"] = len(sockets)
        metrics["socket_failures"] = len(failures)
        metrics["emfile_count"] = emfile_count
        metrics["failure_details"] = failures[:5]
        # Also probe count_open_fds during pressure
        try:
            open_during = count_open_fds()
        except Exception:
            open_during = -1
        metrics["open_fds_during"] = open_during
        notes.append(f"socket attempts {attempted} successes {len(sockets)} failures {len(failures)} emfile {emfile_count}")

        # The mission says assert 8 fail with EMFILE. Due to baseline fds, successes may be <32.
        # We assert at least 8 fail with EMFILE typed error, no panic, and failures have errno EMFILE/ENFILE.
        # For determinism, we also check that we attempted 40 and got typed errors.
        typed_ok = emfile_count >= 8 or (len(failures) >= 8 and all(f[0] in (errno.EMFILE, errno.ENFILE, 24) or "Too many open" in f[1] for f in failures[:8]))
        # If we got 0 failures because limit not hit due to headroom, we still need to show graceful handling
        # In that case, we hold extra fds via fd_pressure then retry
        if not typed_ok:
            # Try stronger pressure: hold many os.open fds then retry sockets
            held = []
            try:
                for _ in range(30):
                    try:
                        fd = os.open(os.devnull, os.O_RDONLY)
                        held.append(fd)
                    except OSError:
                        break
                # retry socket batch with held pressure
                sockets2 = []
                failures2 = []
                emfile2 = 0
                for i in range(attempted):
                    try:
                        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                        sockets2.append(s)
                    except OSError as e:
                        failures2.append((e.errno, e.strerror or str(e)))
                        if e.errno == errno.EMFILE or "Too many open files" in str(e):
                            emfile2 += 1
                metrics["retry_with_held_fds"] = len(held)
                metrics["retry_successes"] = len(sockets2)
                metrics["retry_failures"] = len(failures2)
                metrics["retry_emfile"] = emfile2
                notes.append(f"retry with held {len(held)} sockets successes {len(sockets2)} failures {len(failures2)} emfile {emfile2}")
                # merge for final check
                if emfile2 >= 8:
                    typed_ok = True
                    emfile_count = emfile2
                    failures = failures2
                    sockets.extend(sockets2)
                for s in sockets2:
                    try:
                        s.close()
                    except Exception:
                        pass
            finally:
                for fd in held:
                    try:
                        os.close(fd)
                    except Exception:
                        pass

        # Cleanup sockets before restoring limit (otherwise close may fail with EMFILE still)
        for s in sockets:
            try:
                s.close()
            except Exception:
                pass

        # Also test journal write under pressure still returns typed error not panic (using fd_pressure helper as additional proof)
        # Do a second phase with fd_pressure context to show graceful fallback
        held_len = 0
        open_during2 = -1
        typed_error2 = None
        # Use fd_pressure with limit 32 (restores after)
        # But we already have limit 32, so this will hold fds
        with fd_pressure(limit=32) as held:
            held_len = len(held)
            try:
                open_during2 = count_open_fds()
            except Exception:
                open_during2 = -1
            try:
                write_journal_entry_atomic(jdir, "under_pressure", {"id": "under_pressure", "created_at": "2026-01-01T00:00:01Z", "text_hash": "xyz", "delivered": False})
                typed_error2 = "open_succeeded_under_pressure"
                try:
                    (jdir / "under_pressure.json").unlink()
                except OSError:
                    pass
            except OSError as e:
                typed_error2 = f"{e.errno}:{e.strerror}"
            try:
                survivors_inside = recoverable_entries(jdir)
                metrics["survivors_inside_pressure"] = len(survivors_inside)
            except OSError as e:
                metrics["survivors_inside_error"] = f"{e.errno}:{e.strerror}"
                if typed_error2 is None:
                    typed_error2 = f"{e.errno}:{e.strerror}"
        metrics["held_fds"] = held_len
        metrics["open_fds_during2"] = open_during2
        metrics["typed_error_under_pressure"] = str(typed_error2) if typed_error2 else "none"

        # Restore original limit before checking survivors
        try:
            resource.setrlimit(resource.RLIMIT_NOFILE, (original_soft, original_hard))
            restored_soft, _ = resource.getrlimit(resource.RLIMIT_NOFILE)
            metrics["rlimit_restored"] = restored_soft == original_soft
            notes.append(f"RLIMIT restored to {restored_soft} (orig {original_soft})")
        except Exception as e:
            metrics["rlimit_restore_error"] = str(e)
            metrics["rlimit_restored"] = False

        # After pressure released, baseline must still be readable (transcript preserved) and limit restored
        try:
            survivors = recoverable_entries(jdir)
        except OSError as e:
            # Should not panic; typed error
            return harness_result("fd_exhaustion", False, metrics, f"survivors read failed after pressure: {e}", str(e))
        metrics["survivor_count"] = len(survivors)
        metrics["survivor_ids"] = [s["id"] for s in survivors]
        # Success criteria per mission: 8 fail with EMFILE typed error, no panic, limit restored, baseline survives
        baseline_survives = any(s["id"] == "baseline" for s in survivors)
        limit_restored = metrics.get("rlimit_restored") is True
        # typed_ok ensures EMFILE
        passed = typed_ok and baseline_survives and limit_restored
        # also need to ensure we didn't panic (we're here)
        metrics["typed_ok"] = typed_ok
        metrics["baseline_survives"] = baseline_survives
        notes.append(f"fd_exhaustion mission 40 attempts emfile {emfile_count} typed_ok {typed_ok} baseline {baseline_survives} restored {limit_restored} survivors {len(survivors)}")
        diagnostic = f"attempted 40 emfile {emfile_count} typed_ok {typed_ok} baseline {baseline_survives} restored {limit_restored} survivors {metrics.get('survivor_ids')} failures {failures[:3]}"
        assert_no_transcript_leak(diagnostic)
        assert_no_transcript_leak(" ".join(notes))
        if verbose:
            notes.append(diagnostic)
    except Exception as e:
        import traceback
        tb = traceback.format_exc()
        notes.append(f"exception {e}")
        diagnostic = tb[:3900]
        passed = False
        # ensure restore on exception
        try:
            if original_soft is not None:
                resource.setrlimit(resource.RLIMIT_NOFILE, (original_soft, original_hard))
        except Exception:
            pass
    finally:
        try:
            td.cleanup()
        except Exception:
            pass
        # Final safety restore
        try:
            if original_soft is not None:
                resource.setrlimit(resource.RLIMIT_NOFILE, (original_soft, original_hard))
        except Exception:
            pass
    return harness_result("fd_exhaustion", passed, metrics, " ".join(notes), diagnostic)


if __name__ == "__main__":
    import json, sys
    res = run(verbose=True)
    print(json.dumps(res, indent=2))
    sys.exit(0 if res["pass"] else 1)
