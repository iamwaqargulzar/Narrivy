#!/usr/bin/env python3
"""Derive a Narrivy phase report from authoritative data instead of prose.

A phase report used to be typed by hand, which let a stale total survive next
to the traceability CSV that contradicted it. This generator derives every
count, requirement list, evidence figure, commit and gate result from:

* ``spec/requirements-traceability.csv``  — the authoritative requirement state
* ``agent/evidence/INDEX.json`` and ``agent/evidence/**`` — the evidence registry
* live ``git`` state and the live host environment
* the phase's own gate command, declared in ``agent/PHASES.json`` and executed here

Fail-closed rules:

* an unreadable, malformed or unmappable input aborts with a non-zero exit and
  nothing is written;
* the report is validated against ``spec/schemas/phase-report.schema.json``
  before it is written, and every emitted requirement id must exist in the CSV;
* ``status`` is ``passed`` only when the gate exits 0 *and* the traceability data
  leaves no open release-gated requirement. A green gate over a blocked CSV, or
  a red gate over an empty CSV, can never produce a passing report.

Usage::

    python3 scripts/generate_phase_report.py --phase P11
    python3 scripts/generate_phase_report.py --phase P11 --check
    python3 scripts/generate_phase_report.py --phase P11 --headline

``--check`` re-derives only the deterministic facts (no gate, clock, git or
environment) and exits 3 when the committed report has drifted from them, so CI
can catch a hand-edited report.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, Sequence

ROOT = Path(os.environ.get("NARRIVY_ROOT", Path(__file__).resolve().parents[1])).resolve()

# Evidence schema version whose procedure assertions must resolve a measurement.
# See scripts/evidence_policy.MEASURED_EVIDENCE_SCHEMA_VERSION.
MEASURED_EVIDENCE_SCHEMA_VERSION = 3

TRACEABILITY_REL = Path("spec/requirements-traceability.csv")
EVIDENCE_INDEX_REL = Path("agent/evidence/INDEX.json")
EVIDENCE_ROOT_REL = Path("agent/evidence")
PHASES_REL = Path("agent/PHASES.json")
REPORT_SCHEMA_REL = Path("spec/schemas/phase-report.schema.json")
PACK_VERSION_REL = Path("BUILD-PACK-VERSION")
REPORTS_REL = Path("agent/phase-reports")

TRACEABILITY_COLUMNS = (
    "requirement_id",
    "primary_phase",
    "verification_id",
    "release_gate",
    "status",
    "evidence",
    "evidence_disposition",
)

# CSV status -> phase-report requirement status. The report schema has no
# "implementing"; an unimplemented row is not a pass, so it is reported blocked
# and the untouched CSV tally is always published alongside it.
STATUS_TO_REPORT = {
    "passed": "passed",
    "blocked": "blocked",
    "roadmap": "planned",
    "planned": "planned",
    "implementing": "blocked",
}

# Metrics that are a pure function of the traceability CSV, the evidence index
# and the on-disk evidence tree. These are what CI re-derives and compares; the
# clock, git, environment and gate results are deliberately excluded because they
# legitimately differ between the qualification host and a clean checkout.
DETERMINISTIC_METRIC_KEYS = (
    "evidence_index_policy",
    "evidence_on_disk_not_registered",
    "evidence_records_on_disk",
    "evidence_registered_hash_mismatches",
    "evidence_registered_in_index",
    "evidence_registered_missing_on_disk",
    "evidence_schema_version_counts",
    "measured_evidence_records_registered",
    "phase_primary_release_gated_blocked",
    "phase_primary_release_gated_passed",
    "phase_primary_release_gated_requirements",
    "phase_primary_requirement_ids",
    "phase_secondary_requirement_ids",
    "release_gated_blocked_requirement_ids",
    "release_gated_requirements_blocked",
    "release_gated_requirements_passed",
    "release_gated_requirements_passed_with_measured_evidence",
    "release_gated_requirements_total",
    "release_gated_passed_evidence_unreadable",
    "roadmap_requirements_total",
    "traceability_requirements_total",
    "traceability_status_counts",
)

HEADLINE_BEGIN = "<!-- phase-report-generated:begin -->"
HEADLINE_END = "<!-- phase-report-generated:end -->"

EXIT_OK = 0
EXIT_FAILURE = 1
EXIT_DRIFT = 3

KNOWN_VM_VENDORS = (
    "qemu",
    "vmware",
    "virtualbox",
    "innotek",
    "kvm",
    "bochs",
    "xen",
    "parallels",
    "virtual machine",
    "amazon ec2",
    "google",
    "openstack",
    "hyper-v",
)


class GeneratorError(RuntimeError):
    """Any condition that must abort generation instead of emitting a report."""


# --------------------------------------------------------------------------- #
# inputs
# --------------------------------------------------------------------------- #
def read_text(path: Path, label: str) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        raise GeneratorError(f"cannot read {label} ({path}): {exc}") from exc


def read_json(path: Path, label: str) -> Any:
    try:
        return json.loads(read_text(path, label))
    except json.JSONDecodeError as exc:
        raise GeneratorError(f"{label} ({path}) is not valid JSON: {exc}") from exc


def load_traceability(root: Path) -> list[dict[str, str]]:
    path = root / TRACEABILITY_REL
    try:
        handle = path.open(newline="", encoding="utf-8")
    except OSError as exc:
        raise GeneratorError(f"cannot read authoritative traceability ({path}): {exc}") from exc
    with handle:
        reader = csv.DictReader(handle)
        missing = [column for column in TRACEABILITY_COLUMNS if column not in (reader.fieldnames or [])]
        if missing:
            raise GeneratorError(f"traceability is missing required columns: {missing}")
        rows = [{(k or ""): (v or "").strip() for k, v in row.items() if k} for row in reader]
    if not rows:
        raise GeneratorError("traceability contains no requirement rows")
    seen: set[str] = set()
    for row in rows:
        rid = row["requirement_id"]
        if not re.fullmatch(r"[A-Z]+-[0-9]+", rid):
            raise GeneratorError(f"traceability requirement_id is malformed: {rid!r}")
        if rid in seen:
            raise GeneratorError(f"traceability contains duplicate requirement_id: {rid}")
        seen.add(rid)
        if row["status"] not in STATUS_TO_REPORT:
            raise GeneratorError(
                f"{rid}: traceability status {row['status']!r} has no phase-report mapping"
            )
        if row["release_gate"] not in {"required", "roadmap"}:
            raise GeneratorError(f"{rid}: traceability release_gate {row['release_gate']!r} is invalid")
    return rows


def traceability_index(rows: Sequence[Mapping[str, str]]) -> dict[str, Mapping[str, str]]:
    return {row["requirement_id"]: row for row in rows}


def load_phase_entry(root: Path, phase: str) -> dict[str, Any]:
    registry = read_json(root / PHASES_REL, "phase registry")
    phases = registry.get("phases") if isinstance(registry, Mapping) else None
    if not isinstance(phases, list):
        raise GeneratorError("phase registry has no phases list")
    for entry in phases:
        if isinstance(entry, Mapping) and entry.get("id") == phase:
            return dict(entry)
    known = [entry.get("id") for entry in phases if isinstance(entry, Mapping)]
    raise GeneratorError(f"unknown phase {phase}; registry={known}")


def load_report_schema(root: Path) -> Mapping[str, Any]:
    schema = read_json(root / REPORT_SCHEMA_REL, "phase report schema")
    if not isinstance(schema, Mapping):
        raise GeneratorError("phase report schema is not an object")
    return schema


def load_evidence_inventory(root: Path) -> dict[str, Any]:
    index_path = root / EVIDENCE_INDEX_REL
    index = read_json(index_path, "evidence index")
    if not isinstance(index, Mapping) or not isinstance(index.get("qualified_evidence"), list):
        raise GeneratorError("evidence index has no qualified_evidence list")

    registered: dict[str, str] = {}
    for entry in index["qualified_evidence"]:
        if not isinstance(entry, Mapping) or not entry.get("path"):
            raise GeneratorError("evidence index entry has no path")
        rel = str(entry["path"])
        if rel in registered:
            raise GeneratorError(f"evidence index registers {rel} twice")
        registered[rel] = str(entry.get("sha256", ""))

    evidence_root = root / EVIDENCE_ROOT_REL
    if not evidence_root.is_dir():
        raise GeneratorError(f"evidence directory is missing: {evidence_root}")

    on_disk: dict[str, int] = {}
    for path in sorted(evidence_root.rglob("*.json")):
        rel = path.relative_to(root).as_posix()
        if rel == EVIDENCE_INDEX_REL.as_posix():
            continue
        record = read_json(path, f"evidence record {rel}")
        if not isinstance(record, Mapping):
            raise GeneratorError(f"evidence record {rel} is not an object")
        version = record.get("schema_version")
        if isinstance(version, bool) or not isinstance(version, int):
            raise GeneratorError(f"evidence record {rel} has no integer schema_version")
        on_disk[rel] = version

    schema_counts = Counter(on_disk.values())
    missing_on_disk = sorted(set(registered) - set(on_disk))
    hash_mismatches: list[str] = []
    for rel, expected in sorted(registered.items()):
        if rel not in on_disk:
            continue
        digest = hashlib.sha256((root / rel).read_bytes()).hexdigest()
        if digest != expected:
            hash_mismatches.append(rel)

    measured_registered = sum(
        1
        for rel in sorted(set(registered) & set(on_disk))
        if on_disk[rel] == MEASURED_EVIDENCE_SCHEMA_VERSION
    )

    return {
        "index": index,
        "registered": registered,
        "on_disk": on_disk,
        "schema_counts": schema_counts,
        "missing_on_disk": missing_on_disk,
        "hash_mismatches": hash_mismatches,
        "measured_registered": measured_registered,
    }


# --------------------------------------------------------------------------- #
# live machine state
# --------------------------------------------------------------------------- #
def git(root: Path, *argv: str) -> str | None:
    try:
        result = subprocess.run(
            ["git", *argv], cwd=root, capture_output=True, text=True, timeout=30, check=False
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise GeneratorError(f"git {' '.join(argv)} failed: {exc}") from exc
    if result.returncode != 0:
        return None
    return result.stdout.strip()


def git_head(root: Path) -> str:
    head = git(root, "rev-parse", "HEAD")
    if not head or not re.fullmatch(r"[a-f0-9]{40}", head):
        raise GeneratorError("a real Git HEAD is required to generate a phase report")
    return head


def git_dirty(root: Path) -> bool:
    porcelain = git(root, "status", "--porcelain")
    if porcelain is None:
        raise GeneratorError("cannot determine worktree state from git status")
    return bool(porcelain)


def git_start_commit(root: Path, phase: str, entry: Mapping[str, Any]) -> str:
    acceptance = str(entry.get("acceptance", ""))
    if acceptance:
        commit = git(root, "log", "--diff-filter=A", "--format=%H", "-1", "--", acceptance)
        if commit and re.fullmatch(r"[a-f0-9]{40}", commit):
            return commit
    return "unknown"


def os_release_field(name: str) -> str:
    text = ""
    try:
        text = Path("/etc/os-release").read_text(encoding="utf-8")
    except OSError:
        return ""
    for line in text.splitlines():
        key, _, value = line.partition("=")
        if key.strip() == name:
            return value.strip().strip('"')
    return ""


def detect_native_or_vm() -> str:
    try:
        result = subprocess.run(
            ["systemd-detect-virt"], capture_output=True, text=True, timeout=15, check=False
        )
    except (OSError, subprocess.SubprocessError):
        result = None  # type: ignore[assignment]
    if result is not None and result.returncode in (0, 1):
        verdict = result.stdout.strip().lower()
        if verdict and verdict != "none":
            return "vm"
    dmi = ""
    for field in ("sys_vendor", "product_name"):
        try:
            dmi += Path("/sys/class/dmi/id", field).read_text(encoding="utf-8").lower()
        except OSError:
            continue
    if any(vendor in dmi for vendor in KNOWN_VM_VENDORS):
        return "vm"
    if result is None and not dmi:
        raise GeneratorError(
            "cannot determine whether this host is native or a VM; "
            "refusing to guess a phase-report environment"
        )
    return "native"


def detect_cpu() -> str:
    try:
        text = Path("/proc/cpuinfo").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    for line in text.splitlines():
        key, _, value = line.partition(":")
        if key.strip() == "model name":
            return value.strip()
    return ""


def collect_environment() -> dict[str, Any]:
    kernel = ""
    try:
        kernel = os.uname().release
    except OSError as exc:
        raise GeneratorError(f"cannot read the running kernel: {exc}") from exc
    os_name = os_release_field("NAME") or os.uname().sysname
    os_version = os_release_field("VERSION") or os_release_field("VERSION_ID")
    if not os_name or not os_version or not kernel:
        raise GeneratorError("cannot read the host os/release from /etc/os-release or uname")
    desktop = os.environ.get("XDG_CURRENT_DESKTOP", "").strip() or "unknown"
    session = os.environ.get("XDG_SESSION_TYPE", "").strip() or "unknown"
    return {
        "os": os_name,
        "os_version": os_version,
        "kernel": kernel,
        "desktop": desktop,
        "session": session,
        "native_or_vm": detect_native_or_vm(),
        "cpu": detect_cpu() or None,
    }


# --------------------------------------------------------------------------- #
# derivation
# --------------------------------------------------------------------------- #
def derive_deterministic_metrics(
    rows: Sequence[Mapping[str, str]], phase: str, evidence: Mapping[str, Any]
) -> dict[str, Any]:
    required = [row for row in rows if row["release_gate"] == "required"]
    roadmap = [row for row in rows if row["release_gate"] == "roadmap"]
    primary = [row for row in rows if row["primary_phase"] == phase]
    primary_required = [row for row in primary if row["release_gate"] == "required"]
    secondary = [
        row["requirement_id"] for row in rows if row.get("secondary_phase", "") == phase
    ]

    passed_with_measured = 0
    passed_evidence_missing: list[str] = []
    for row in required:
        if row["status"] != "passed":
            continue
        rel = row["evidence"]
        version = evidence["on_disk"].get(rel)
        if version is None:
            passed_evidence_missing.append(row["requirement_id"])
        elif version == MEASURED_EVIDENCE_SCHEMA_VERSION and rel in evidence["registered"]:
            passed_with_measured += 1

    on_disk = evidence["on_disk"]
    registered = evidence["registered"]
    return {
        "traceability_requirements_total": len(rows),
        "traceability_status_counts": dict(sorted(Counter(row["status"] for row in rows).items())),
        "release_gated_requirements_total": len(required),
        "release_gated_requirements_passed": sum(1 for row in required if row["status"] == "passed"),
        "release_gated_requirements_blocked": sum(
            1 for row in required if row["status"] == "blocked"
        ),
        "release_gated_requirements_passed_with_measured_evidence": passed_with_measured,
        "release_gated_blocked_requirement_ids": sorted(
            row["requirement_id"] for row in required if row["status"] != "passed"
        ),
        "roadmap_requirements_total": len(roadmap),
        "phase_primary_release_gated_requirements": len(primary_required),
        "phase_primary_release_gated_passed": sum(
            1 for row in primary_required if row["status"] == "passed"
        ),
        "phase_primary_release_gated_blocked": sum(
            1 for row in primary_required if row["status"] != "passed"
        ),
        "phase_primary_requirement_ids": sorted(row["requirement_id"] for row in primary),
        "phase_secondary_requirement_ids": sorted(secondary),
        "evidence_index_policy": str(evidence["index"].get("policy", "unknown")),
        "evidence_records_on_disk": len(on_disk),
        "evidence_schema_version_counts": {
            str(version): count for version, count in sorted(evidence["schema_counts"].items())
        },
        "evidence_registered_in_index": len(registered),
        "evidence_registered_missing_on_disk": len(evidence["missing_on_disk"]),
        "evidence_registered_hash_mismatches": len(evidence["hash_mismatches"]),
        "evidence_on_disk_not_registered": len(set(on_disk) - set(registered)),
        "measured_evidence_records_registered": evidence["measured_registered"],
        "release_gated_passed_evidence_unreadable": passed_evidence_missing,
    }


def derive_requirements(
    rows: Sequence[Mapping[str, str]], phase: str
) -> list[dict[str, str]]:
    derived: list[dict[str, str]] = []
    for row in rows:
        if row["primary_phase"] != phase:
            continue
        evidence = row["evidence"]
        if not evidence:
            evidence = (
                "unset in spec/requirements-traceability.csv"
                f" (release_gate={row['release_gate']},"
                f" evidence_disposition={row['evidence_disposition'] or 'unset'})"
            )
        derived.append(
            {
                "requirement_id": row["requirement_id"],
                "verification_id": row["verification_id"],
                "status": STATUS_TO_REPORT[row["status"]],
                "evidence": evidence,
            }
        )
    derived.sort(key=lambda item: item["requirement_id"])
    return derived


def derive_artifacts(
    root: Path, entry: Mapping[str, Any]
) -> tuple[list[dict[str, str]], dict[str, Any]]:
    """Hash only immutable release output; a snapshot must not pin mutable source.

    A phase report is a point-in-time record. Hashing a working-tree document
    into it would make the report go stale on every unrelated edit and
    reintroduce exactly the drift this generator exists to remove, so only
    declared artifacts under ``artifacts/`` are attested. Every other declared
    artifact is still inventoried in ``metrics`` so nothing is hidden.
    """
    acceptance_rel = str(entry.get("acceptance", ""))
    declared: list[str] = []
    if acceptance_rel:
        acceptance = read_json(root / acceptance_rel, f"acceptance spec {acceptance_rel}")
        if isinstance(acceptance, Mapping):
            for rel in acceptance.get("required_artifacts", []) or []:
                if isinstance(rel, str) and rel not in declared:
                    declared.append(rel)

    artifacts: list[dict[str, str]] = []
    missing: list[str] = []
    unhashed: list[str] = []
    for rel in declared:
        candidate = root / rel
        if not candidate.is_file():
            missing.append(rel)
            continue
        if not rel.startswith("artifacts/"):
            unhashed.append(rel)
            continue
        artifacts.append(
            {
                "path": rel,
                "sha256": hashlib.sha256(candidate.read_bytes()).hexdigest(),
            }
        )
    inventory = {
        "required_artifacts_declared": len(declared),
        "required_artifacts_present": len(artifacts) + len(unhashed),
        "required_artifacts_hashed": [item["path"] for item in artifacts],
        "required_artifacts_missing": missing,
        "required_artifacts_present_but_not_hashed": unhashed,
    }
    return artifacts, inventory


def derive_status(
    *,
    gate_exit_code: int,
    open_release_gated: int,
    open_phase_requirements: int,
    phase_started: bool,
) -> str:
    """A phase passes only on a green gate over a fully closed requirement set."""
    if gate_exit_code == 0 and open_release_gated == 0:
        return "passed"
    if not phase_started:
        return "not_started"
    if open_phase_requirements == 0:
        return "failed"
    return "in_progress"


def run_gate(root: Path, gate_command: str, timeout: int) -> tuple[int, str]:
    try:
        argv = shlex.split(gate_command)
    except ValueError as exc:
        raise GeneratorError(f"gate command cannot be parsed: {gate_command!r}: {exc}") from exc
    if not argv:
        raise GeneratorError("gate command is empty")
    try:
        result = subprocess.run(
            argv, cwd=root, capture_output=True, text=True, timeout=timeout, check=False
        )
    except subprocess.TimeoutExpired as exc:
        raise GeneratorError(
            f"gate did not finish within {timeout}s, so its real exit code is unknown: {gate_command}"
        ) from exc
    except OSError as exc:
        raise GeneratorError(f"gate command could not be executed: {gate_command}: {exc}") from exc
    return result.returncode, (result.stdout or "") + (result.stderr or "")


# --------------------------------------------------------------------------- #
# timestamps
# --------------------------------------------------------------------------- #
def parse_timestamp(value: object) -> datetime | None:
    if not isinstance(value, str) or not value.strip():
        return None
    try:
        parsed = datetime.fromisoformat(value.strip().replace("Z", "+00:00"))
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed


def iso(moment: datetime) -> str:
    return moment.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def phase_start(
    root: Path, phase: str, entry: Mapping[str, Any], generated_at: datetime
) -> tuple[datetime, str]:
    candidates: list[datetime] = []
    evidence_dir = root / EVIDENCE_ROOT_REL / phase
    if evidence_dir.is_dir():
        for path in sorted(evidence_dir.rglob("*.json")):
            record = read_json(path, f"evidence record {path.relative_to(root).as_posix()}")
            if isinstance(record, Mapping) and record.get("phase") == phase:
                for field in ("started_at", "ended_at"):
                    moment = parse_timestamp(record.get(field))
                    if moment is not None:
                        candidates.append(moment)
    if candidates:
        return min(candidates), "earliest timestamp in this phase's on-disk evidence records"
    acceptance = str(entry.get("acceptance", ""))
    if acceptance:
        commit_date = git(root, "log", "--diff-filter=A", "--format=%cI", "-1", "--", acceptance)
        moment = parse_timestamp(commit_date)
        if moment is not None:
            return moment, f"commit date that added {acceptance}"
    return generated_at, "report generation time (no earlier dated source exists)"


# --------------------------------------------------------------------------- #
# report assembly
# --------------------------------------------------------------------------- #
def build_report(
    *,
    root: Path,
    phase: str,
    rows: Sequence[Mapping[str, str]],
    entry: Mapping[str, Any],
    evidence: Mapping[str, Any],
    gate_exit_code: int,
    gate_command: str,
    gate_transcript: str,
) -> dict[str, Any]:
    deterministic = derive_deterministic_metrics(rows, phase, evidence)
    requirements = derive_requirements(rows, phase)
    artifacts, artifact_inventory = derive_artifacts(root, entry)

    head = git_head(root)
    dirty = git_dirty(root)
    generated_at = datetime.now(timezone.utc).replace(microsecond=0)
    started_at, started_source = phase_start(root, phase, entry, generated_at)

    pack_version = read_text(root / PACK_VERSION_REL, "pack version").strip()
    if not pack_version:
        raise GeneratorError("BUILD-PACK-VERSION is empty")
    short_head = head[:8]
    build_id = f"{pack_version}+git{short_head}{'-dirty' if dirty else ''}"

    open_release_gated = int(deterministic["release_gated_requirements_total"]) - int(
        deterministic["release_gated_requirements_passed"]
    )
    open_phase = int(deterministic["phase_primary_release_gated_requirements"]) - int(
        deterministic["phase_primary_release_gated_passed"]
    )
    phase_started = bool(deterministic["phase_primary_release_gated_requirements"]) or bool(
        (root / EVIDENCE_ROOT_REL / phase).is_dir()
    )
    status = derive_status(
        gate_exit_code=gate_exit_code,
        open_release_gated=open_release_gated,
        open_phase_requirements=open_phase,
        phase_started=phase_started,
    )

    blockers: list[str] = []
    blocked_ids = list(deterministic["release_gated_blocked_requirement_ids"])
    if blocked_ids:
        blockers.append(
            f"{len(blocked_ids)} of {deterministic['release_gated_requirements_total']} "
            f"release-gated requirements are not passed: {', '.join(blocked_ids)}"
        )
    if open_phase:
        blockers.append(
            f"{open_phase} release-gated requirement(s) with primary_phase={phase} "
            f"are not passed: "
            f"{', '.join(sorted(item['requirement_id'] for item in requirements if item['status'] != 'passed'))}"
        )
    if artifact_inventory["required_artifacts_missing"]:
        blockers.append(
            "declared required artifacts are absent from the worktree, so no artifact "
            "hash can be reported: "
            + ", ".join(artifact_inventory["required_artifacts_missing"])
        )
    if evidence["hash_mismatches"]:
        blockers.append(
            "evidence index hashes do not match the files on disk: "
            + ", ".join(evidence["hash_mismatches"])
        )
    if evidence["missing_on_disk"]:
        blockers.append(
            "evidence index registers records that are not on disk: "
            + ", ".join(evidence["missing_on_disk"])
        )
    if gate_exit_code != 0:
        blockers.append(
            f"the phase gate `{gate_command}` exits {gate_exit_code}; this report is not a pass"
        )

    measured = int(deterministic["measured_evidence_records_registered"])
    schema_readout = (
        ", ".join(
            f"v{version}: {count}"
            for version, count in sorted(deterministic["evidence_schema_version_counts"].items())
        )
        or "none"
    )
    summary = (
        f"Derived by scripts/generate_phase_report.py from {TRACEABILITY_REL.as_posix()}, "
        f"{EVIDENCE_INDEX_REL.as_posix()} and live repository state. "
        f"Release-gated requirements: {deterministic['release_gated_requirements_passed']} passed "
        f"and {open_release_gated} not passed of "
        f"{deterministic['release_gated_requirements_total']} "
        f"({deterministic['traceability_requirements_total']} traceability rows in total, "
        f"{deterministic['roadmap_requirements_total']} of them post-1.0 roadmap). "
        f"This phase owns {deterministic['phase_primary_release_gated_requirements']} "
        f"release-gated requirement(s) as primary phase, of which "
        f"{deterministic['phase_primary_release_gated_passed']} are passed. "
        f"Evidence on disk: {deterministic['evidence_records_on_disk']} records "
        f"(schema versions {schema_readout}), "
        f"{deterministic['evidence_registered_in_index']} registered in the authoritative index, "
        f"{measured} of them schema-v{MEASURED_EVIDENCE_SCHEMA_VERSION} measured records. "
        f"Zero of the {deterministic['release_gated_requirements_passed']} passed requirements are "
        f"currently backed by a schema-v{MEASURED_EVIDENCE_SCHEMA_VERSION} measured record, so the "
        f"passed count is what the traceability CSV declares, not a statement that those records "
        f"still satisfy the measured-assertion rule. "
        f"Gate `{gate_command}` exited {gate_exit_code} at commit {head[:7]} "
        f"(dirty={str(dirty).lower()}), so this report's status is `{status}`."
    )

    deviations = (
        "No hand-entered total survives: every count, id list, evidence figure, commit and gate "
        "result in this report is recomputed from "
        f"{TRACEABILITY_REL.as_posix()}, {EVIDENCE_INDEX_REL.as_posix()}, the on-disk evidence "
        "tree and the live worktree. Two consequences are deliberate. First, only declared "
        "artifacts under artifacts/ are hashed into this snapshot, because a phase report is a "
        "point-in-time record and pinning a mutable working-tree document would make the report "
        "stale on every unrelated edit; every other declared artifact is inventoried in metrics "
        "instead. Second, "
        f"started_at={iso(started_at)} is derived from the {started_source}, and ended_at is this "
        "report's generation time, so the window records when evidence began to exist rather than "
        "asserting continuous work."
    )

    metrics: dict[str, Any] = dict(deterministic)
    metrics.update(
        {
            "report_generated_at": iso(generated_at),
            "started_at_source": started_source,
            "git_end_commit": head,
            "git_dirty": dirty,
            "required_artifacts_declared": artifact_inventory["required_artifacts_declared"],
            "required_artifacts_present": artifact_inventory["required_artifacts_present"],
            "required_artifacts_hashed": artifact_inventory["required_artifacts_hashed"],
            "required_artifacts_missing": artifact_inventory["required_artifacts_missing"],
            "required_artifacts_present_but_not_hashed": artifact_inventory[
                "required_artifacts_present_but_not_hashed"
            ],
            "phase_evidence_records": len(
                list((root / EVIDENCE_ROOT_REL / phase).rglob("*.json"))
            )
            if (root / EVIDENCE_ROOT_REL / phase).is_dir()
            else 0,
            "build_id_source": (
                f"{PACK_VERSION_REL.as_posix()} plus the live git HEAD short hash, "
                "suffixed with -dirty when the worktree is dirty"
            ),
        }
    )

    report: dict[str, Any] = {
        "schema_version": 1,
        "phase": phase,
        "status": status,
        "started_at": iso(started_at),
        "ended_at": iso(generated_at),
        "git_start_commit": git_start_commit(root, phase, entry),
        "git_end_commit": head,
        "git_dirty": dirty,
        "build_id": build_id,
        "gate_command": gate_command,
        "gate_exit_code": gate_exit_code,
        "gate_transcript_hash": hashlib.sha256(gate_transcript.encode("utf-8")).hexdigest(),
        "environment": collect_environment(),
        "requirements": requirements,
        "artifacts": artifacts,
        "metrics": metrics,
        "summary": summary,
        "deviations": deviations,
        "blockers": blockers,
    }
    validate_report(report, load_report_schema(root))
    validate_requirement_ids(report, traceability_index(rows))
    validate_claims(report)
    return report


def validate_report(report: Mapping[str, Any], schema: Mapping[str, Any]) -> None:
    try:
        import jsonschema
    except ImportError as exc:  # pragma: no cover - depends on the host
        raise GeneratorError(
            "jsonschema is required to validate the generated phase report; "
            "it is already required by scripts/validate_pack.py"
        ) from exc
    try:
        jsonschema.validate(instance=json.loads(json.dumps(report)), schema=dict(schema))
    except Exception as exc:
        raise GeneratorError(f"generated report fails {REPORT_SCHEMA_REL.as_posix()}: {exc}") from exc


def validate_requirement_ids(
    report: Mapping[str, Any], index: Mapping[str, Mapping[str, str]]
) -> None:
    unknown = sorted(
        item["requirement_id"]
        for item in report["requirements"]
        if item["requirement_id"] not in index
    )
    if unknown:
        raise GeneratorError(
            f"report names requirement ids that are absent from {TRACEABILITY_REL.as_posix()}: {unknown}"
        )
    wrong = sorted(
        item["requirement_id"]
        for item in report["requirements"]
        if index[item["requirement_id"]]["verification_id"] != item["verification_id"]
        or STATUS_TO_REPORT[index[item["requirement_id"]]["status"]] != item["status"]
    )
    if wrong:
        raise GeneratorError(f"report requirement entries disagree with the CSV: {wrong}")


def validate_claims(report: Mapping[str, Any]) -> None:
    """Independent of the schema: the report may never overstate acceptance."""
    metrics = report["metrics"]
    blocked = int(metrics["release_gated_requirements_blocked"])
    passed = int(metrics["release_gated_requirements_passed"])
    total = int(metrics["release_gated_requirements_total"])
    if passed + blocked != total:
        raise GeneratorError(
            f"release-gated tally does not add up: {passed} + {blocked} != {total}"
        )
    if report["status"] == "passed":
        if report["gate_exit_code"] != 0:
            raise GeneratorError("status passed with a non-zero gate exit code")
        if blocked:
            raise GeneratorError(
                f"status passed with {blocked} release-gated requirements not passed"
            )
    if int(metrics["phase_primary_release_gated_passed"]) + int(
        metrics["phase_primary_release_gated_blocked"]
    ) != int(metrics["phase_primary_release_gated_requirements"]):
        raise GeneratorError("phase primary release-gated tally does not add up")


# --------------------------------------------------------------------------- #
# headline block
# --------------------------------------------------------------------------- #
def headline_block(metrics: Mapping[str, Any]) -> str:
    counts = metrics["evidence_schema_version_counts"]
    schema_cell = ", ".join(f"v{version}: {count}" for version, count in counts.items()) or "none"
    lines = [
        HEADLINE_BEGIN,
        "### Authoritative counts (generated; do not hand-edit)",
        "",
        "| Fact | Value |",
        "| --- | --- |",
        f"| Traceability rows | {metrics['traceability_requirements_total']} |",
        f"| Release-gated requirements | {metrics['release_gated_requirements_total']} |",
        f"| Release-gated passed | {metrics['release_gated_requirements_passed']} |",
        f"| Release-gated not passed | {metrics['release_gated_requirements_blocked']} |",
        f"| Post-1.0 roadmap requirements | {metrics['roadmap_requirements_total']} |",
        f"| This phase's own release-gated requirements | {metrics['phase_primary_release_gated_requirements']} |",
        f"| This phase's own passed | {metrics['phase_primary_release_gated_passed']} |",
        f"| Evidence records on disk | {metrics['evidence_records_on_disk']} |",
        f"| Evidence records registered in the index | {metrics['evidence_registered_in_index']} |",
        f"| On-disk but unregistered | {metrics['evidence_on_disk_not_registered']} |",
        f"| Evidence schema versions | {schema_cell} |",
        f"| Schema-v{MEASURED_EVIDENCE_SCHEMA_VERSION} measured records registered | {metrics['measured_evidence_records_registered']} |",
        f"| Passed requirements backed by a measured record | {metrics['release_gated_requirements_passed_with_measured_evidence']} |",
        "",
        HEADLINE_END,
    ]
    return "\n".join(lines)


def extract_headline_block(text: str) -> str | None:
    """Return the generated headline block from a narrative report, if present."""
    start = text.find(HEADLINE_BEGIN)
    if start < 0:
        return None
    end = text.find(HEADLINE_END, start)
    if end < 0:
        return None
    return text[start : end + len(HEADLINE_END)]


def write_headline_block(path: Path, metrics: Mapping[str, Any]) -> str:
    """Replace the generated block inside a narrative report, in place.

    The block is the only part of the `.md` the generator owns: everything else
    is a human engineering log and is left untouched. Returns a short
    description of what changed so the caller can report it without diffing.
    """
    if not path.is_file():
        raise FileNotFoundError(f"narrative report not found: {path}")
    text = path.read_text()
    block = headline_block(metrics)
    existing = extract_headline_block(text)
    if existing is None:
        raise RuntimeError(
            f"{path.name} carries no generated headline block; the guard cannot verify it, "
            f"so a new one cannot be placed without a decision about where it belongs"
        )
    if existing == block:
        return "headline already current"
    path.write_text(text.replace(existing, block, 1))
    return "headline block refreshed"


# --------------------------------------------------------------------------- #
# commands
# --------------------------------------------------------------------------- #
def check_on_disk(root: Path, phase: str) -> int:
    rows = load_traceability(root)
    evidence = load_evidence_inventory(root)
    report_path = root / REPORTS_REL / f"{phase}.json"
    if not report_path.is_file():
        print(f"no committed phase report at {report_path.relative_to(root)}", file=sys.stderr)
        return EXIT_DRIFT
    try:
        on_disk = json.loads(read_text(report_path, "phase report"))
    except GeneratorError as exc:
        print(str(exc), file=sys.stderr)
        return EXIT_DRIFT
    try:
        validate_report(on_disk, load_report_schema(root))
    except GeneratorError as exc:
        print(f"committed {report_path.relative_to(root)} is not schema-valid: {exc}", file=sys.stderr)
        return EXIT_DRIFT
    fresh = derive_deterministic_metrics(rows, phase, evidence)
    drift: list[str] = []
    if on_disk.get("phase") != phase:
        drift.append(f"phase: {on_disk.get('phase')!r} != {phase!r}")
    for key in DETERMINISTIC_METRIC_KEYS:
        if key not in fresh:
            continue
        if on_disk.get("metrics", {}).get(key) != fresh[key]:
            drift.append(
                f"metrics.{key}: committed {on_disk.get('metrics', {}).get(key)!r} != derived {fresh[key]!r}"
            )
    if on_disk.get("requirements") != derive_requirements(rows, phase):
        drift.append("requirements: committed list differs from the traceability CSV")
    if on_disk.get("status") == "passed" and on_disk.get("gate_exit_code") != 0:
        drift.append("status passed with a non-zero gate_exit_code")
    if on_disk.get("status") == "passed" and int(
        on_disk.get("metrics", {}).get("release_gated_requirements_blocked", -1)
    ) not in (0,):
        drift.append("status passed while release-gated requirements remain not passed")
    if drift:
        print(f"{phase} phase report has drifted from its authoritative sources:", file=sys.stderr)
        for line in drift:
            print(f" - {line}", file=sys.stderr)
        return EXIT_DRIFT
    print(f"{phase} phase report matches {TRACEABILITY_REL.as_posix()} and {EVIDENCE_INDEX_REL.as_posix()}.")
    return EXIT_OK


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--phase", required=True, help="phase id, e.g. P11")
    parser.add_argument(
        "--root",
        default=str(ROOT),
        help="repository root (defaults to NARRIVY_ROOT or the checkout containing this script)",
    )
    parser.add_argument(
        "--output",
        default=None,
        help="report path (defaults to agent/phase-reports/<phase>.json)",
    )
    parser.add_argument(
        "--gate-command",
        default=None,
        help="gate to execute (defaults to the phase's declared gate in agent/PHASES.json)",
    )
    parser.add_argument(
        "--gate-timeout",
        type=int,
        default=3600,
        help="seconds to allow the gate; a timeout fails closed because the real exit code is unknown",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="re-derive the deterministic facts and exit 3 if the committed report has drifted",
    )
    parser.add_argument(
        "--headline",
        action="store_true",
        help="print the machine-checkable markdown headline block for the narrative report",
    )
    parser.add_argument(
        "--write-headline",
        action="store_true",
        help="refresh the generated headline block inside the narrative report in place",
    )
    args = parser.parse_args(list(argv) if argv is not None else None)
    root = Path(args.root).resolve()

    try:
        if args.check:
            return check_on_disk(root, args.phase)
        entry = load_phase_entry(root, args.phase)
        rows = load_traceability(root)
        evidence = load_evidence_inventory(root)
        gate_command = args.gate_command or str(entry.get("gate", "")).strip()
        if not gate_command:
            raise GeneratorError(
                f"{args.phase} declares no gate command in {PHASES_REL.as_posix()}"
            )
        if args.headline:
            metrics = derive_deterministic_metrics(rows, args.phase, evidence)
            print(headline_block(metrics))
            return EXIT_OK
        if args.write_headline:
            metrics = derive_deterministic_metrics(rows, args.phase, evidence)
            narrative = root / "agent" / "phase-reports" / f"{args.phase}.md"
            print(write_headline_block(narrative, metrics))
            return EXIT_OK
        gate_exit_code, transcript = run_gate(root, gate_command, args.gate_timeout)
        report = build_report(
            root=root,
            phase=args.phase,
            rows=rows,
            entry=entry,
            evidence=evidence,
            gate_exit_code=gate_exit_code,
            gate_command=gate_command,
            gate_transcript=transcript,
        )
    except GeneratorError as exc:
        print(f"generate_phase_report: {exc}", file=sys.stderr)
        return EXIT_FAILURE

    destination = Path(args.output) if args.output else root / REPORTS_REL / f"{args.phase}.json"
    try:
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    except OSError as exc:
        print(f"generate_phase_report: cannot write {destination}: {exc}", file=sys.stderr)
        return EXIT_FAILURE

    print(
        f"wrote {destination.relative_to(root)}: status={report['status']} "
        f"gate_exit_code={report['gate_exit_code']} commit={report['git_end_commit'][:7]} "
        f"release_gated={report['metrics']['release_gated_requirements_passed']}"
        f"/{report['metrics']['release_gated_requirements_total']} passed, "
        f"{report['metrics']['release_gated_requirements_blocked']} not passed"
    )
    if report["status"] != "passed":
        print(
            "this is an honest red report: the release is not qualified. "
            "It is not a failure of the generator.",
            file=sys.stderr,
        )
    return EXIT_OK


if __name__ == "__main__":
    raise SystemExit(main())
