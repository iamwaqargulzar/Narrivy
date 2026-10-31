"""Shared safety checks for the evidence recorders (WP4 T4.5, finding P-4).

Every recorder that writes reviewed evidence uses these helpers instead of a
private copy, so one fix covers all of them:

* ``replaceable`` / ``commit_records``: a guarded replacement of a recorder's
  own earlier output, and one transaction for the files plus INDEX.
* ``native_lane``: the only way a record may claim ``native_or_vm: native``.
* ``scan_log_privacy``: transcript n-gram and canary scan; a missing or
  unreadable log is a failure, never a clean result.
* ``cargo_test_results`` / ``vitest_results`` / ``require_tests_passed``: a
  named test counts only when its own structured result is ok/passed.
* ``run_bounded``: a subprocess in its own process group with a timeout that
  kills the whole group.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import signal
import stat
import subprocess
import tempfile
import unicodedata
from collections.abc import Callable, Iterable, Mapping, Sequence
from pathlib import Path


class RecorderSafetyError(RuntimeError):
    """A recorder precondition failed; no evidence may be written."""


# ---------------------------------------------------------------------------
# Guarded replacement and the evidence/INDEX transaction.

REPLACEABLE_SCHEMA_VERSIONS = frozenset({1, 2})


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def replaceable(path: Path, index: Mapping[str, object], root: Path) -> bool:
    """Whether a recorder may write ``path``.

    Allowed: no file yet; an unindexed historical schema-v1 file; or an
    indexed schema-v1/v2 record whose bytes still match its INDEX hash. A
    current (v3) record, a duplicate INDEX entry, a hash mismatch or an
    unreadable file is never replaced.
    """
    if not path.exists():
        return True
    relative = path.resolve().relative_to(root.resolve()).as_posix()
    entries = [
        item
        for item in index.get("qualified_evidence", [])
        if isinstance(item, Mapping) and item.get("path") == relative
    ]
    try:
        if path.is_symlink() or not path.is_file():
            return False
        payload = path.read_bytes()
        version = json.loads(payload).get("schema_version")
    except (OSError, ValueError, AttributeError):
        return False
    if not entries:
        return version == 1
    if len(entries) != 1 or version not in REPLACEABLE_SCHEMA_VERSIONS:
        return False
    return entries[0].get("sha256") == _sha256(payload)


def _atomic_write(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def index_with_records(
    index: Mapping[str, object], records: Iterable[tuple[str, bytes]]
) -> dict[str, object]:
    """Return a copy of ``index`` whose entries for ``records`` are replaced."""
    records = list(records)
    targets = {relative for relative, _payload in records}
    updated = dict(index)
    entries = [
        item
        for item in index.get("qualified_evidence", [])
        if not (isinstance(item, Mapping) and item.get("path") in targets)
    ]
    for relative, payload in records:
        entries.append({"path": relative, "sha256": _sha256(payload), "disposition": "qualified"})
    entries.sort(key=lambda item: item["path"])
    updated["qualified_evidence"] = entries
    return updated


def commit_records(
    root: Path,
    index_path: Path,
    index: Mapping[str, object],
    records: Sequence[tuple[Path, bytes]],
    *,
    validate_index: Callable[[Mapping[str, object]], None] | None = None,
    write: Callable[[Path, bytes], None] = _atomic_write,
) -> dict[str, object]:
    """Write every record and its INDEX entry, or nothing.

    ``index`` is the INDEX as read when the recorder started. Each target must
    still be ``replaceable``. If any write fails, every file already touched
    (and INDEX) is restored to its previous bytes before the error propagates.
    """
    relatives = [
        (path, path.resolve().relative_to(root.resolve()).as_posix(), payload)
        for path, payload in records
    ]
    for path, relative, _payload in relatives:
        if not replaceable(path, index, root):
            raise RecorderSafetyError(f"evidence output is not replaceable: {relative}")
    updated = index_with_records(index, [(relative, payload) for _p, relative, payload in relatives])
    if validate_index is not None:
        validate_index(updated)
    index_payload = (json.dumps(updated, indent=2, sort_keys=True) + "\n").encode()
    previous: list[tuple[Path, bytes | None]] = []
    try:
        for path, _relative, payload in relatives:
            previous.append((path, path.read_bytes() if path.exists() else None))
            write(path, payload)
        previous.append((index_path, index_path.read_bytes() if index_path.exists() else None))
        write(index_path, index_payload)
    except BaseException:
        for path, before in reversed(previous):
            try:
                if before is None:
                    path.unlink(missing_ok=True)
                else:
                    _atomic_write(path, before)
            except OSError:
                pass
        raise
    return updated


# ---------------------------------------------------------------------------
# The reviewed native lane.

NATIVE_OS_ID = "pop"
NATIVE_OS_VERSION_ID = "24.04"
CONTAINER_MARKERS = ("/.dockerenv", "/run/.containerenv")


def parse_os_release(text: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip('"').strip("'")
    return values


def _detect_virt(argv: Sequence[str]) -> str:
    try:
        result = subprocess.run(
            list(argv), capture_output=True, text=True, timeout=5.0, check=False
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RecorderSafetyError(f"{argv[0]} is unavailable: {type(error).__name__}") from None
    return result.stdout.strip()


def native_lane(
    *,
    environ: Mapping[str, str] | None = None,
    os_release_path: Path = Path("/etc/os-release"),
    detect_virt: Callable[[Sequence[str]], str] = _detect_virt,
    filesystem_root: Path = Path("/"),
) -> dict[str, object]:
    """Prove the reviewed native lane, or raise ``RecorderSafetyError``.

    Requires Pop!_OS 24.04, a COSMIC desktop, a Wayland session whose
    ``WAYLAND_DISPLAY`` socket exists, ``systemd-detect-virt`` reporting
    ``none`` for both VM and container, and no container marker files.
    Environment strings alone never prove the lane: the socket, the OS file
    and systemd's detection must agree with them.
    """
    env = os.environ if environ is None else environ
    try:
        release = parse_os_release(os_release_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError):
        raise RecorderSafetyError("native lane: /etc/os-release is unreadable") from None
    if release.get("ID") != NATIVE_OS_ID or release.get("VERSION_ID") != NATIVE_OS_VERSION_ID:
        raise RecorderSafetyError("native lane requires Pop!_OS 24.04")
    desktop = env.get("XDG_CURRENT_DESKTOP", "")
    if "cosmic" not in [part.strip().lower() for part in desktop.split(":")]:
        raise RecorderSafetyError("native lane requires XDG_CURRENT_DESKTOP to contain COSMIC")
    session = env.get("XDG_SESSION_TYPE", "")
    display = env.get("WAYLAND_DISPLAY", "")
    if session.lower() != "wayland" or not display:
        raise RecorderSafetyError("native lane requires a Wayland session with WAYLAND_DISPLAY set")
    socket_path = Path(display)
    if not socket_path.is_absolute():
        runtime = env.get("XDG_RUNTIME_DIR", "")
        if not runtime:
            raise RecorderSafetyError("native lane requires XDG_RUNTIME_DIR for the Wayland socket")
        socket_path = Path(runtime) / display
    try:
        if not stat.S_ISSOCK(socket_path.stat().st_mode):
            raise RecorderSafetyError("native lane: WAYLAND_DISPLAY is not a socket")
    except OSError:
        raise RecorderSafetyError("native lane: the Wayland socket does not exist") from None
    virt = detect_virt(["systemd-detect-virt"])
    container = detect_virt(["systemd-detect-virt", "--container"])
    if virt != "none" or container != "none":
        raise RecorderSafetyError(
            f"native lane requires bare metal (systemd-detect-virt: {virt or '?'}, "
            f"container: {container or '?'})"
        )
    for marker in CONTAINER_MARKERS:
        if (filesystem_root / marker.lstrip("/")).exists():
            raise RecorderSafetyError(f"native lane: container marker present: {marker}")
    return {
        "os": release.get("NAME", "Pop!_OS"),
        "os_id": release["ID"],
        "os_version": release.get("PRETTY_NAME", release["VERSION_ID"]),
        "os_version_id": release["VERSION_ID"],
        "desktop": desktop,
        "session": session,
        "wayland_socket_present": True,
        "virtualization": virt,
        "container": container,
    }


# ---------------------------------------------------------------------------
# Log privacy.

LOG_SCAN_NGRAM = 3
MAX_SCANNED_LOG_BYTES = 64 * 1024 * 1024


def normalized_words(text: str) -> list[str]:
    folded = unicodedata.normalize("NFKC", text).casefold()
    return re.findall(r"\w+", folded)


def transcript_ngrams(transcript: str, n: int = LOG_SCAN_NGRAM) -> set[str]:
    words = normalized_words(transcript)
    if not words:
        return set()
    if len(words) < n:
        return {" ".join(words)}
    return {" ".join(words[i : i + n]) for i in range(len(words) - n + 1)}


def scan_log_privacy(
    logs: Iterable[Path | bytes | None],
    *,
    transcripts: Iterable[str] = (),
    canaries: Iterable[str] = (),
) -> dict[str, object]:
    """Scan logs for transcript n-grams and canaries; return measured counts.

    The transcript stays in memory: only counts are returned. ``None`` (a log
    the caller could not obtain), a missing path or an unreadable file raises,
    so "no log" can never be recorded as "clean log".
    """
    ngrams: set[str] = set()
    for transcript in transcripts:
        ngrams |= transcript_ngrams(transcript)
    canary_list = [canary for canary in canaries if canary]
    if not ngrams and not canary_list:
        raise RecorderSafetyError("log privacy scan has nothing to look for")
    scanned = 0
    total_bytes = 0
    ngram_hits = 0
    canary_hits = 0
    for log in logs:
        if log is None:
            raise RecorderSafetyError("log privacy scan: a required log is unavailable")
        if isinstance(log, Path):
            try:
                if log.stat().st_size > MAX_SCANNED_LOG_BYTES:
                    raise RecorderSafetyError("log privacy scan: log exceeds the scan bound")
                payload = log.read_bytes()
            except OSError:
                raise RecorderSafetyError("log privacy scan: a required log is missing or unreadable") from None
        else:
            payload = bytes(log)
        text = payload.decode("utf-8", "replace")
        scanned += 1
        total_bytes += len(payload)
        haystack = " ".join(normalized_words(text))
        ngram_hits += sum(1 for gram in ngrams if f" {gram} " in f" {haystack} ")
        canary_hits += sum(1 for canary in canary_list if canary in text)
    if scanned == 0:
        raise RecorderSafetyError("log privacy scan: no log was scanned")
    return {
        "logs_scanned": scanned,
        "bytes_scanned": total_bytes,
        "transcript_ngrams_checked": len(ngrams),
        "transcript_ngram_hits": ngram_hits,
        "canaries_checked": len(canary_list),
        "canary_hits": canary_hits,
        "clean": ngram_hits == 0 and canary_hits == 0,
    }


# ---------------------------------------------------------------------------
# Structured test results.

_CARGO_RESULT = re.compile(r"^test (\S+?)(?: - should panic)? \.\.\. (ok|FAILED|ignored)(?:,.*)?$")


def cargo_test_results(output: str) -> dict[str, str]:
    """Map each libtest test path to ``ok``, ``FAILED`` or ``ignored``.

    Needs the default (non ``--quiet``) libtest output; ``--nocapture`` keeps
    the result lines intact.
    """
    results: dict[str, str] = {}
    for line in output.splitlines():
        match = _CARGO_RESULT.match(line.strip())
        if match:
            name, status = match.groups()
            if results.get(name, "ok") != "ok":
                continue
            results[name] = "ok" if status == "ok" else status
    return results


def vitest_results(report: Mapping[str, object]) -> dict[str, str]:
    """Map Vitest JSON reporter results to ``ok`` or the observed status.

    Keys: each test's ``fullName``, its ``title``, ``<file>::<fullName>`` and
    the bare file name. A file key is ``ok`` only when the file ran at least
    one test and every one passed; a skipped/pending test is not a pass.
    """
    results: dict[str, str] = {}

    def put(key: str, status: str) -> None:
        if key and results.get(key, "ok") == "ok":
            results[key] = status

    files = report.get("testResults")
    if not isinstance(files, list):
        raise RecorderSafetyError("Vitest JSON report has no testResults")
    for file_result in files:
        if not isinstance(file_result, Mapping):
            continue
        file_name = Path(str(file_result.get("name", ""))).name
        statuses: list[str] = []
        for assertion in file_result.get("assertionResults", []) or []:
            if not isinstance(assertion, Mapping):
                continue
            raw = str(assertion.get("status", ""))
            status = "ok" if raw == "passed" else raw or "unknown"
            statuses.append(status)
            full = str(assertion.get("fullName") or "")
            title = str(assertion.get("title") or "")
            for key in (full, title, f"{file_name}::{full}"):
                put(key, status)
        if file_name:
            put(file_name, "ok" if statuses and all(x == "ok" for x in statuses) else "failed")
    return results


_UNITTEST_HEAD = re.compile(r"^(test\w*) \(([\w.]+)\)(.*)$")


def unittest_results(output: str) -> dict[str, str]:
    """Map ``python -m unittest -v`` results to ``ok``/``skipped``/``FAILED``/``ERROR``.

    Keys are ``<module.Class>::<test>``; a docstring line between the test and
    its result is allowed.
    """
    results: dict[str, str] = {}
    lines = output.splitlines()
    for index, line in enumerate(lines):
        match = _UNITTEST_HEAD.match(line)
        if not match:
            continue
        name, owner, rest = match.groups()
        tail = rest if " ... " in rest else (lines[index + 1] if index + 1 < len(lines) else "")
        if " ... " not in tail:
            continue
        verdict = tail.rsplit(" ... ", 1)[1].strip()
        if verdict == "ok":
            status = "ok"
        elif verdict.startswith("skipped"):
            status = "skipped"
        elif verdict == "FAIL":
            status = "FAILED"
        elif verdict == "ERROR":
            status = "ERROR"
        else:
            status = verdict or "unknown"
        owner = owner.removesuffix("." + name)
        key = f"{owner}::{name}"
        if results.get(key, "ok") == "ok":
            results[key] = status
    return results


def require_tests_passed(
    results: Mapping[str, str], names: Iterable[str], *, passed: str = "ok"
) -> dict[str, str]:
    """Return each named test's status; raise unless every one passed.

    A name matches a result exactly or as its last ``::`` segment(s). Every
    match must have passed: a test that is missing, ignored or skipped (in any
    suite that has a test of that name) is a failure.
    """
    observed: dict[str, str] = {}
    for name in names:
        matches = [
            status
            for test, status in results.items()
            if test == name or test.endswith("::" + name)
        ]
        if not matches:
            observed[name] = "missing"
        else:
            observed[name] = next((status for status in matches if status != passed), passed)
    failed = sorted(name for name, status in observed.items() if status != passed)
    if failed:
        raise RecorderSafetyError("named tests did not pass: " + ", ".join(failed))
    return observed


# ---------------------------------------------------------------------------
# Bounded subprocesses.


def kill_group(process: subprocess.Popen, grace: float = 2.0) -> None:
    if process.poll() is not None:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except OSError:
            pass
        return
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(process.pid, sig)
        except OSError:
            pass
        try:
            process.wait(timeout=grace)
            break
        except subprocess.TimeoutExpired:
            continue
    # The leader is gone; reap any group member that ignored SIGTERM.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except OSError:
        pass


def run_bounded(
    argv: Sequence[str],
    *,
    timeout: float,
    on_timeout: Callable[[], None] | None = None,
    **kwargs: object,
) -> subprocess.CompletedProcess:
    """``subprocess.run`` in a new process group, killing the group on timeout.

    ``on_timeout`` runs after the group is gone (for example to cancel a
    provision the child started). Leftover group members after a normal exit
    are also killed, so a helper cannot outlive the recorder.
    """
    kwargs.setdefault("stdin", subprocess.DEVNULL)
    if "capture_output" in kwargs:
        if kwargs.pop("capture_output"):
            kwargs["stdout"] = subprocess.PIPE
            kwargs["stderr"] = subprocess.PIPE
    process = subprocess.Popen(list(argv), start_new_session=True, **kwargs)  # type: ignore[arg-type]
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        kill_group(process)
        process.communicate()
        if on_timeout is not None:
            on_timeout()
        raise
    except BaseException:
        kill_group(process)
        raise
    kill_group(process)
    return subprocess.CompletedProcess(list(argv), process.returncode, stdout, stderr)
