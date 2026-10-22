#!/usr/bin/env python3
"""Shared, non-mutating trust checks for qualification evidence.

Measurement contract and migration
---------------------------------
A procedure assertion used to be ``{"id": <label>, "result": "pass"}``: a label
that validated itself, because the only rule was that the reviewed contract's
``required_assertions`` id appeared with ``result == "pass"``. This module now
requires every assertion to carry a *measurement* - a pointer into the record's
own ``metrics``, a comparator, and the value it must satisfy - and requires the
reviewed procedure contract to pin that binding, so a recorder cannot swap in a
trivially-true metric.

Migration:

* ``schema_version: 3`` requires a measurement on every assertion and requires
  ``metrics`` to exist. The six recorders covered by this change emit v3.
* ``schema_version: 2`` stays *readable* - the old assertion shape still
  validates against the schema and historical material is never destroyed -
  but a v2 record can no longer qualify, because none of its 163 existing
  assertion labels is backed by a measurement. That is the intended, fail-closed
  consequence: all 50 v2 records must be re-recorded by a migrated recorder.
* A procedure whose ``required_assertions`` entry is still a bare string has no
  reviewed measurement. It remains in the contract so the claim is not silently
  dropped, and any record that depends on it fails closed with a named error.

Nothing here mutates a record, and no check has been relaxed: the v2 path can
only ever produce more errors than it did before.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
from collections.abc import Mapping
from pathlib import Path

EVIDENCE_TYPES = {
    "unit",
    "mock",
    "simulated",
    "integration",
    "native-platform",
    "package-artifact",
    "manual",
}
ARTIFACT_KINDS = {"executable", "package"}
SHA256_RE = re.compile(r"^[a-f0-9]{64}$")
INDEX_RELATIVE_PATH = Path("agent/evidence/INDEX.json")
INDEX_SCHEMA_RELATIVE_PATH = Path("spec/schemas/evidence-index.schema.json")
PROFILE_RELATIVE_PATH = Path("spec/evidence-verification-profiles.json")
PROFILE_SCHEMA_RELATIVE_PATH = Path("spec/schemas/evidence-verification-profiles.schema.json")
DISALLOWED_BEHAVIORAL_COMMANDS = {"true", "test", ":"}
SUBSTITUTION_TYPES = {"mock", "fixture", "proxy", "stub", "synthetic-audio"}

# An assertion is a claim; a measurement is the evidence for it. Schema-v3
# records must bind every assertion to a metric inside their own record, and
# the reviewed procedure contract pins which metric backs which claim so a
# recorder cannot substitute a trivially-true measurement.
RELEASED_EVIDENCE_SCHEMA_VERSIONS = {2, 3}
MEASURED_EVIDENCE_SCHEMA_VERSION = 3
MEASUREMENT_METRIC_PATTERN = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_-]*(\.[A-Za-z0-9_-]+)*$")
MEASUREMENT_FIELDS = frozenset({"metric", "comparator", "expected", "tolerance"})
MEASUREMENT_COMPARATORS = (
    "eq",
    "ne",
    "lt",
    "le",
    "gt",
    "ge",
    "in",
    "not_in",
    "contains",
    "len_eq",
    "len_ne",
    "len_lt",
    "len_le",
    "len_gt",
    "len_ge",
    "is_true",
    "is_false",
)
_ORDERED_COMPARATORS = frozenset({"lt", "le", "gt", "ge"})
_LENGTH_COMPARATORS = frozenset(
    {"len_eq", "len_ne", "len_lt", "len_le", "len_gt", "len_ge"}
)
_SEQUENCE_COMPARATORS = frozenset({"in", "not_in", "contains"})
_FLAG_COMPARATORS = frozenset({"is_true", "is_false"})
SCALAR_COMPARATORS = frozenset({"eq", "ne"})
_MISSING = object()


class MeasurementError(ValueError):
    """A measurement binding is unusable; it can never support a pass."""


def _is_number(value: object) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _value_kind(value: object) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, Mapping):
        return "object"
    if isinstance(value, (list, tuple)):
        return "array"
    return type(value).__name__


def resolve_measurement_metric(metrics: object, path: object) -> object:
    """Resolve a dotted metric path, or raise MeasurementError."""
    if not isinstance(path, str) or not MEASUREMENT_METRIC_PATTERN.fullmatch(path):
        raise MeasurementError(f"measurement metric path is malformed: {path!r}")
    if not isinstance(metrics, Mapping):
        raise MeasurementError("evidence has no metrics object to resolve a measurement against")
    current: object = metrics
    for segment in path.split("."):
        if not isinstance(current, Mapping) or segment not in current:
            raise MeasurementError(f"measurement metric does not resolve: {path}")
        current = current[segment]
    return current


def _compare_ordered(comparator: str, actual: object, expected: object) -> bool:
    if not _is_number(actual) or not _is_number(expected):
        raise MeasurementError(
            f"comparator {comparator!r} requires numeric operands, got "
            f"{_value_kind(actual)} and {_value_kind(expected)}"
        )
    if comparator == "lt":
        return actual < expected
    if comparator == "le":
        return actual <= expected
    if comparator == "gt":
        return actual > expected
    return actual >= expected


def _compare_length(comparator: str, actual: object, expected: object) -> bool:
    if not _is_number(expected):
        raise MeasurementError(f"comparator {comparator!r} requires a numeric expected length")
    if not isinstance(actual, (str, list, tuple, Mapping)):
        raise MeasurementError(
            f"comparator {comparator!r} requires a sized measurement, got {_value_kind(actual)}"
        )
    size = len(actual)
    if comparator == "len_eq":
        return size == expected
    if comparator == "len_ne":
        return size != expected
    if comparator == "len_lt":
        return size < expected
    if comparator == "len_le":
        return size <= expected
    if comparator == "len_gt":
        return size > expected
    return size >= expected


def _compare_sequence(comparator: str, actual: object, expected: object) -> bool:
    if not isinstance(expected, (list, tuple)) or not expected:
        raise MeasurementError(f"comparator {comparator!r} requires a non-empty expected array")
    if isinstance(actual, (str, list, tuple)):
        items = [actual] if isinstance(actual, str) else list(actual)
    elif isinstance(actual, Mapping):
        items = list(actual)
    else:
        raise MeasurementError(
            f"comparator {comparator!r} requires a string or collection measurement, got "
            f"{_value_kind(actual)}"
        )
    if comparator == "in":
        return any(_same_json_value(item, actual) for item in expected)
    if comparator == "not_in":
        return not any(_same_json_value(item, actual) for item in expected)
    return all(any(_same_json_value(item, candidate) for candidate in items) for item in expected)


def _same_json_value(left: object, right: object) -> bool:
    if _value_kind(left) != _value_kind(right):
        return False
    if isinstance(left, (list, tuple)):
        return len(left) == len(right) and all(
            _same_json_value(a, b) for a, b in zip(left, right, strict=True)
        )
    if isinstance(left, Mapping):
        return dict(left) == dict(right)
    return bool(left == right)


def _compare_scalar(comparator: str, actual: object, expected: object, tolerance: object) -> bool:
    if tolerance is not _MISSING and not isinstance(tolerance, (int, float)):
        raise MeasurementError("measurement tolerance must be numeric")
    if tolerance is not _MISSING and not _is_number(actual):
        raise MeasurementError("measurement tolerance is only defined for numeric measurements")
    if _is_number(actual) and _is_number(expected):
        slack = float(tolerance) if tolerance is not _MISSING else 0.0
        equal = abs(float(actual) - float(expected)) <= slack
    else:
        equal = _same_json_value(actual, expected)
    return equal if comparator == "eq" else not equal


def measurement_failure_reason(metrics: object, measurement: object) -> str | None:
    """Return None when the measurement resolves and is satisfied, else why not.

    Every failure mode is a hard error: an unusable binding never supports a pass.
    """
    satisfied, reason = _evaluate_measurement(metrics, measurement)
    return None if satisfied else reason


def _evaluate_measurement(metrics: object, measurement: object) -> tuple[bool, str | None]:
    if not isinstance(measurement, Mapping):
        return False, "measurement binding is malformed"
    unknown = set(measurement) - MEASUREMENT_FIELDS
    if unknown:
        return False, "measurement binding has unknown fields: " + ", ".join(sorted(unknown))
    comparator = measurement.get("comparator")
    if not isinstance(comparator, str) or comparator not in MEASUREMENT_COMPARATORS:
        return False, f"measurement comparator is not evaluable: {comparator!r}"
    try:
        actual = resolve_measurement_metric(metrics, measurement.get("metric"))
    except MeasurementError as exc:
        return False, str(exc)
    expected = measurement.get("expected", _MISSING)
    tolerance = measurement.get("tolerance", _MISSING)
    try:
        if comparator in _FLAG_COMPARATORS:
            if expected is not _MISSING:
                raise MeasurementError(
                    f"comparator {comparator!r} does not take an expected value"
                )
            if not isinstance(actual, bool):
                raise MeasurementError(
                    f"comparator {comparator!r} requires a boolean measurement, got "
                    f"{_value_kind(actual)}"
                )
            satisfied = actual if comparator == "is_true" else not actual
        elif comparator in _ORDERED_COMPARATORS:
            if expected is _MISSING:
                raise MeasurementError(f"comparator {comparator!r} requires an expected value")
            satisfied = _compare_ordered(comparator, actual, expected)
        elif comparator in _LENGTH_COMPARATORS:
            if expected is _MISSING:
                raise MeasurementError(f"comparator {comparator!r} requires an expected value")
            satisfied = _compare_length(comparator, actual, expected)
        elif comparator in _SEQUENCE_COMPARATORS:
            if expected is _MISSING:
                raise MeasurementError(f"comparator {comparator!r} requires an expected value")
            satisfied = _compare_sequence(comparator, actual, expected)
        elif comparator in SCALAR_COMPARATORS:
            if expected is _MISSING:
                raise MeasurementError(f"comparator {comparator!r} requires an expected value")
            satisfied = _compare_scalar(comparator, actual, expected, tolerance)
        else:  # pragma: no cover - guarded by the membership check above
            return False, f"measurement comparator is not evaluable: {comparator!r}"
    except MeasurementError as exc:
        return False, str(exc)
    if satisfied:
        return True, None
    return (
        False,
        f"measurement {measurement.get('metric')!r} {comparator} {_render(expected)} is not "
        f"satisfied by observed {_render(actual)}",
    )


def _render(value: object) -> str:
    if value is _MISSING:
        return "<absent>"
    if isinstance(value, (list, tuple)):
        return "[" + ", ".join(_render(item) for item in value) + "]"
    if isinstance(value, str):
        return repr(value)
    return repr(value)


def assert_measurements(
    data: Mapping[str, object], assertion_id: str, measurement: object = _MISSING
) -> list[str]:
    """Fail-closed check for one assertion's measurement binding.

    A declared ``pass`` that the measurement contradicts is reported as a
    contradiction, not as a generic failure: the label overstates the record.
    """
    if measurement is _MISSING:
        return [
            f"assertion declares no measurement binding: {assertion_id} "
            f"(schema-v{MEASURED_EVIDENCE_SCHEMA_VERSION} measured assertions are required)"
        ]
    satisfied, reason = _evaluate_measurement(data.get("metrics"), measurement)
    if satisfied:
        return []
    return [f"assertion result contradicts its measurement: {assertion_id}: {reason}"]


def assessment_errors_for_record(data: Mapping[str, object], *, assertion_id: str) -> list[str]:
    """Check one declared assertion against the measurements in its own record.

    This is the public, single-assertion form of the rule the reviewed procedure
    gate applies, exposed so the rule can be exercised directly as well as
    through the release validators.
    """
    procedure = data.get("procedure")
    assertions = procedure.get("assertions") if isinstance(procedure, Mapping) else None
    if not isinstance(assertions, list):
        return [f"evidence procedure assertions are missing: {assertion_id}"]
    for assertion in assertions:
        if isinstance(assertion, Mapping) and assertion.get("id") == assertion_id:
            return assert_measurements(
                data, assertion_id, assertion.get("measurement", _MISSING)
            )
    return [f"assertion is not declared by this record: {assertion_id}"]


def measurement_shape_error(measurement: object) -> str | None:
    """Structural check of a measurement binding, without resolving a metric.

    A contract must not pin a binding that could never be evaluated, otherwise
    the procedure is unqualifiable by construction rather than by measurement.
    """
    if not isinstance(measurement, Mapping):
        return "measurement binding is malformed"
    unknown = set(measurement) - MEASUREMENT_FIELDS
    if unknown:
        return "measurement binding has unknown fields: " + ", ".join(sorted(unknown))
    metric = measurement.get("metric")
    if not isinstance(metric, str) or not MEASUREMENT_METRIC_PATTERN.fullmatch(metric):
        return f"measurement metric path is malformed: {metric!r}"
    comparator = measurement.get("comparator")
    if not isinstance(comparator, str) or comparator not in MEASUREMENT_COMPARATORS:
        return f"measurement comparator is not evaluable: {comparator!r}"
    expected = measurement.get("expected", _MISSING)
    if comparator in _FLAG_COMPARATORS:
        if expected is not _MISSING:
            return f"comparator {comparator!r} does not take an expected value"
    elif comparator in _ORDERED_COMPARATORS or comparator in _LENGTH_COMPARATORS:
        if expected is _MISSING:
            return f"comparator {comparator!r} requires an expected value"
        if not _is_number(expected):
            return f"comparator {comparator!r} requires a numeric expected value"
    elif comparator in _SEQUENCE_COMPARATORS:
        if expected is _MISSING:
            return f"comparator {comparator!r} requires an expected value"
        if not isinstance(expected, (list, tuple)) or not expected:
            return f"comparator {comparator!r} requires a non-empty expected array"
    elif comparator in SCALAR_COMPARATORS and expected is _MISSING:
        return f"comparator {comparator!r} requires an expected value"
    tolerance = measurement.get("tolerance", _MISSING)
    if tolerance is not _MISSING and not _is_number(tolerance):
        return "measurement tolerance must be numeric"
    if tolerance is not _MISSING and comparator not in SCALAR_COMPARATORS:
        return f"comparator {comparator!r} does not take a tolerance"
    return None


def _reviewed_measurement(entry: object) -> object | None:
    if isinstance(entry, Mapping) and "measurement" in entry:
        return entry.get("measurement")
    return None


def _reviewed_assertion_id(entry: object) -> str | None:
    if isinstance(entry, str):
        return entry
    if isinstance(entry, Mapping) and isinstance(entry.get("id"), str):
        return entry["id"]
    return None



def _safe_relative_path(value: object) -> str | None:
    if not isinstance(value, str) or not value:
        return None
    path = Path(value)
    if path.is_absolute() or ".." in path.parts:
        return None
    return path.as_posix()


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _artifact_binding_errors(
    data: Mapping[str, object], *, required_artifact_hash: str | None
) -> list[str]:
    errors: list[str] = []
    binding = data.get("artifact_under_test")
    if not isinstance(binding, Mapping):
        return ["evidence lacks an artifact_under_test binding required by its verification profile"]
    kind = binding.get("kind")
    path = _safe_relative_path(binding.get("path"))
    artifact_sha = binding.get("sha256")
    if kind not in ARTIFACT_KINDS:
        errors.append(f"artifact_under_test has invalid kind: {kind!r}")
    if path is None:
        errors.append("artifact_under_test path is missing or unsafe")
    if not isinstance(artifact_sha, str) or not SHA256_RE.fullmatch(artifact_sha):
        errors.append("artifact_under_test sha256 is missing or invalid")

    evidence_type = data.get("evidence_type")
    if evidence_type == "package-artifact" and kind != "package":
        errors.append("package-artifact evidence must bind a package artifact")
    if evidence_type == "native-platform" and kind not in {"executable", "package"}:
        errors.append("native-platform evidence must bind the exercised executable or package")

    artifact_hashes = data.get("artifact_hashes")
    if not isinstance(artifact_hashes, Mapping):
        errors.append("artifact_hashes is missing or invalid")
    elif path and isinstance(artifact_sha, str) and artifact_hashes.get(path) != artifact_sha:
        errors.append("artifact_under_test is absent from artifact_hashes or has a different hash")
    if required_artifact_hash and artifact_sha != required_artifact_hash:
        errors.append("artifact_under_test is not bound to the expected release artifact hash")
    return errors


def assess_evidence(
    data: Mapping[str, object],
    *,
    current_commit: str,
    expected_lane: str | None = None,
    **_unused: object,
) -> list[str]:
    """Validate facts common to every released evidence type; never mutate a record."""
    errors: list[str] = []
    schema_version = data.get("schema_version")
    if schema_version not in RELEASED_EVIDENCE_SCHEMA_VERSIONS:
        if schema_version == 1:
            return ["schema-v1 evidence is historical-quarantined; release-evidence-v2-required"]
        return [f"unsupported evidence schema_version: {schema_version!r}"]

    if data.get("result") != "pass":
        errors.append(f"evidence result is not pass: {data.get('result')!r}")
    if data.get("pass") is not True:
        errors.append(f"evidence pass flag is not exactly true: {data.get('pass')!r}")
    exit_code = data.get("exit_code")
    if isinstance(exit_code, bool) or not isinstance(exit_code, int) or exit_code != 0:
        errors.append(f"evidence exit_code is not 0: {exit_code!r}")
    if data.get("git_dirty") is not False:
        errors.append("evidence was captured from a dirty worktree")
    if current_commit != "unknown" and data.get("git_commit") != current_commit:
        errors.append("evidence commit does not match the candidate commit")

    evidence_type = data.get("evidence_type")
    if evidence_type not in EVIDENCE_TYPES:
        errors.append(f"unknown evidence_type: {evidence_type!r}")
    environment = data.get("environment")
    if not isinstance(environment, Mapping):
        errors.append("evidence environment is missing")
    else:
        if environment.get("lane_available") is not True:
            errors.append("evidence records an unavailable qualification lane")
        if expected_lane and environment.get("qualification_lane") != expected_lane:
            errors.append(
                f"evidence lane {environment.get('qualification_lane')!r} != expected {expected_lane!r}"
            )
    execution = data.get("execution")
    if not isinstance(execution, Mapping):
        errors.append("evidence execution trust declaration is missing")
    else:
        if execution.get("lane_available") is not True:
            errors.append("execution declares an unavailable lane")
        if execution.get("child_failures_ignored") is not False:
            errors.append("execution ignored a child failure")
        if not isinstance(execution.get("substitutions"), list):
            errors.append("execution substitutions declaration is missing")
    if not isinstance(data.get("artifact_hashes"), Mapping):
        errors.append("artifact_hashes is missing or invalid")
    return errors


def verify_artifact_under_test(data: Mapping[str, object], *, root: Path) -> list[str]:
    """Verify a declared package/executable identity, not behavioral execution."""
    binding = data.get("artifact_under_test")
    if not isinstance(binding, Mapping):
        return ["artifact_under_test binding is missing"]
    kind = binding.get("kind")
    rel = _safe_relative_path(binding.get("path"))
    expected = binding.get("sha256")
    if kind not in ARTIFACT_KINDS or rel is None or not isinstance(expected, str):
        return ["artifact_under_test binding is malformed"]
    candidate = (root / rel).resolve()
    if root.resolve() not in candidate.parents or not candidate.is_file():
        return [f"artifact_under_test path missing or unsafe: {rel}"]
    if _sha256(candidate) != expected:
        return [f"artifact_under_test hash mismatch: {rel}"]
    command = data.get("command")
    if not isinstance(command, str):
        return ["command does not name the artifact_under_test"]
    try:
        argv = shlex.split(command, comments=True)
    except ValueError as exc:
        return [f"command cannot be parsed for artifact binding: {exc}"]
    named_paths = {rel, str(candidate)}
    if kind == "executable" and (not argv or argv[0] not in named_paths):
        return ["command does not invoke the executable artifact_under_test"]
    if kind == "package" and not any(argument in named_paths for argument in argv):
        return ["command does not name the package artifact_under_test as an argument"]
    if kind == "executable" and not os.access(candidate, os.X_OK):
        return [f"artifact_under_test is not executable: {rel}"]
    if kind == "package":
        parts = candidate.relative_to(root).parts
        if candidate.suffix != ".deb" or parts[:2] != ("artifacts", "releases"):
            return ["package artifact_under_test must be a release .deb under artifacts/releases"]
    return []


def verify_artifact_hashes(data: Mapping[str, object], *, root: Path) -> list[str]:
    """Verify all declared provenance hashes, including non-artifact fixtures."""
    errors: list[str] = []
    artifact_hashes = data.get("artifact_hashes")
    if not isinstance(artifact_hashes, Mapping):
        return ["artifact_hashes is missing or invalid"]
    for raw_path, expected in artifact_hashes.items():
        rel = _safe_relative_path(raw_path)
        if rel is None or not isinstance(expected, str) or not SHA256_RE.fullmatch(expected):
            errors.append("malformed artifact_hashes entry")
            continue
        candidate = (root / rel).resolve()
        if root.resolve() not in candidate.parents or not candidate.is_file():
            errors.append(f"artifact hash path missing/unsafe: {raw_path}")
        elif _sha256(candidate) != expected:
            errors.append(f"artifact hash mismatch: {raw_path}")
    return errors


def load_evidence_index(root: Path) -> tuple[dict[str, object] | None, list[str]]:
    """Load and schema-check the authoritative qualified-evidence registry."""
    index_path = root / INDEX_RELATIVE_PATH
    schema_path = root / INDEX_SCHEMA_RELATIVE_PATH
    try:
        import jsonschema
    except ImportError:
        return None, ["jsonschema is required to validate the evidence index"]
    try:
        index = json.loads(index_path.read_text())
        schema = json.loads(schema_path.read_text())
        jsonschema.validate(instance=index, schema=schema)
    except Exception as exc:
        return None, [f"evidence index invalid: {exc}"]
    seen: set[str] = set()
    for entry in index["qualified_evidence"]:
        path = entry["path"]
        if path in seen:
            return None, [f"evidence index contains duplicate path: {path}"]
        seen.add(path)
    return index, []


def verify_evidence_registration(
    evidence_path: Path, *, root: Path, index: Mapping[str, object] | None
) -> list[str]:
    """Require every schema-v2 record to be registered with its exact file hash."""
    try:
        relative = evidence_path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return ["evidence path is outside the repository"]
    if index is None:
        return ["authoritative evidence index is unavailable"]
    entries = index.get("qualified_evidence")
    if not isinstance(entries, list):
        return ["authoritative evidence index has no qualified_evidence list"]
    matches = [entry for entry in entries if isinstance(entry, Mapping) and entry.get("path") == relative]
    if len(matches) != 1:
        return ["schema-v2 evidence is not registered in the authoritative evidence index"]
    indexed_hash = matches[0].get("sha256")
    actual_hash = _sha256(evidence_path)
    if indexed_hash != actual_hash:
        return ["authoritative evidence index hash does not match the evidence file"]
    return []


def load_verification_contract(root: Path) -> tuple[dict[str, object] | None, list[str]]:
    """Load the reviewed layer/profile/procedure contract fail-closed."""
    try:
        import jsonschema
    except ImportError:
        return None, ["jsonschema is required to validate evidence verification profiles"]
    try:
        contract = json.loads((root / PROFILE_RELATIVE_PATH).read_text())
        schema = json.loads((root / PROFILE_SCHEMA_RELATIVE_PATH).read_text())
        jsonschema.validate(instance=contract, schema=schema)
    except Exception as exc:
        return None, [f"evidence verification profile contract invalid: {exc}"]

    profile_ids: set[str] = set()
    layers: set[str] = set()
    for profile in contract["profiles"]:
        profile_id = profile["id"]
        if profile_id in profile_ids:
            return None, [f"verification profile contract contains duplicate profile: {profile_id}"]
        profile_ids.add(profile_id)
        for layer in profile["verification_layers"]:
            if layer in layers:
                return None, [f"verification profile contract maps a layer more than once: {layer}"]
            layers.add(layer)
    procedures: set[str] = set()
    for procedure in contract["procedures"]:
        procedure_id = procedure["id"]
        if procedure_id in procedures:
            return None, [f"verification profile contract contains duplicate procedure: {procedure_id}"]
        procedures.add(procedure_id)
        if procedure["profile_id"] not in profile_ids:
            return None, [f"procedure {procedure_id} references unknown profile {procedure['profile_id']}"]
        required = procedure.get("required_assertions")
        if not isinstance(required, list) or not required:
            return None, [f"procedure {procedure_id} has no reviewed required_assertions"]
        seen: set[str] = set()
        for entry in required:
            assertion_id = _reviewed_assertion_id(entry)
            if assertion_id is None:
                return None, [f"procedure {procedure_id} has a malformed required assertion entry"]
            if assertion_id in seen:
                return None, [
                    f"procedure {procedure_id} requires assertion {assertion_id} more than once"
                ]
            seen.add(assertion_id)
            pinned = _reviewed_measurement(entry)
            if pinned is not None:
                # The pinned binding must itself be structurally evaluable,
                # otherwise the contract would approve an unusable measurement.
                shape_error = measurement_shape_error(pinned)
                if shape_error is not None:
                    return None, [
                        f"procedure {procedure_id} pins an unusable measurement for "
                        f"{assertion_id}: {shape_error}"
                    ]
    return contract, []


def _profile_for_layer(contract: Mapping[str, object], layer: str) -> Mapping[str, object] | None:
    profiles = contract.get("profiles")
    if not isinstance(profiles, list):
        return None
    for profile in profiles:
        if isinstance(profile, Mapping) and layer in profile.get("verification_layers", []):
            return profile
    return None


def _verify_reviewed_procedure(
    data: Mapping[str, object],
    *,
    root: Path,
    contract: Mapping[str, object],
    profile: Mapping[str, object],
    verification_id: str,
) -> list[str]:
    errors: list[str] = []
    evidence_procedure = data.get("procedure")
    if not isinstance(evidence_procedure, Mapping):
        return ["evidence lacks a reviewed procedure declaration"]
    procedure_id = evidence_procedure.get("id")
    if not isinstance(procedure_id, str):
        return ["evidence procedure id is missing"]
    procedures = contract.get("procedures")
    procedure = next(
        (
            item
            for item in procedures if isinstance(item, Mapping) and item.get("id") == procedure_id
        ),
        None,
    ) if isinstance(procedures, list) else None
    if procedure is None:
        return ["evidence procedure is not approved in the verification profile contract"]
    if procedure.get("verification_id") != verification_id:
        errors.append("reviewed procedure is not approved for this verification_id")
    if procedure.get("profile_id") != profile.get("id"):
        errors.append("reviewed procedure profile does not match the requirement verification profile")
    if evidence_procedure.get("profile_id") != profile.get("id"):
        errors.append("evidence procedure profile does not match the requirement verification profile")

    # A declaration is not an allowance.  A substitution may be used only when
    # both the reviewed profile and its reviewed procedure explicitly allow it.
    # Behavioural profiles have a stronger invariant: their schema enumerates
    # every substitution kind, and none may stand in for the exercised
    # native/package artifact.
    execution = data.get("execution")
    substitutions = execution.get("substitutions") if isinstance(execution, Mapping) else None
    if not isinstance(substitutions, list):
        errors.append("execution substitutions declaration is missing or invalid")
    elif any(not isinstance(item, str) or item not in SUBSTITUTION_TYPES for item in substitutions):
        errors.append("execution substitutions declaration contains an unknown substitution")
    else:
        profile_allowed = profile.get("permitted_substitutions")
        procedure_allowed = procedure.get("permitted_substitutions")
        if not isinstance(profile_allowed, list):
            errors.append("verification profile substitution policy is missing or invalid")
            profile_allowed = []
        if not isinstance(procedure_allowed, list):
            errors.append("reviewed procedure substitution policy is missing or invalid")
            procedure_allowed = []
        declared = set(substitutions)
        if profile.get("behavioral_runner_required") is True and declared:
            errors.append(
                "behavioral verification profiles forbid substitutions: "
                + ", ".join(sorted(declared))
            )
        undeclared_by_profile = declared.difference(profile_allowed)
        if undeclared_by_profile:
            errors.append(
                "evidence substitutions are not permitted by verification profile: "
                + ", ".join(sorted(undeclared_by_profile))
            )
        undeclared_by_procedure = declared.difference(procedure_allowed)
        if undeclared_by_procedure:
            errors.append(
                "evidence substitutions are not permitted by reviewed procedure: "
                + ", ".join(sorted(undeclared_by_procedure))
            )
    command = data.get("command")
    try:
        argv = shlex.split(command, comments=True) if isinstance(command, str) else []
    except ValueError as exc:
        errors.append(f"command cannot be parsed for reviewed procedure: {exc}")
        argv = []
    expected_argv = procedure.get("command_argv")
    if not isinstance(expected_argv, list) or argv != expected_argv:
        errors.append("evidence command does not match the reviewed verification procedure")

    assertions = evidence_procedure.get("assertions")
    assertion_entries: dict[str, object] = {}
    if isinstance(assertions, list):
        for assertion in assertions:
            if not isinstance(assertion, Mapping) or assertion.get("id") in assertion_entries:
                errors.append("evidence procedure assertions are malformed or duplicate")
                continue
            assertion_entries[assertion.get("id")] = assertion
    else:
        errors.append("evidence procedure assertions are missing")

    # A label must not be able to assert itself. Every assertion a record
    # claims - required or not - must carry a measurement that resolves in this
    # record's own metrics and satisfies its comparator.
    for assertion_id, assertion in assertion_entries.items():
        errors.extend(
            assert_measurements(
                data, assertion_id, assertion.get("measurement", _MISSING)
            )
        )

    reviewed = procedure.get("required_assertions", [])
    if not isinstance(reviewed, list):
        errors.append("reviewed procedure required_assertions are missing or invalid")
        reviewed = []
    for entry in reviewed:
        assertion_id = _reviewed_assertion_id(entry)
        if assertion_id is None:
            errors.append("reviewed procedure assertion entry is malformed")
            continue
        assertion = assertion_entries.get(assertion_id)
        if not isinstance(assertion, Mapping) or assertion.get("result") != "pass":
            errors.append(f"reviewed procedure assertion did not pass: {assertion_id}")
        pinned = _reviewed_measurement(entry)
        if pinned is None:
            errors.append(
                f"reviewed procedure assertion has no reviewed measurement binding: {assertion_id} "
                "(the procedure contract must pin the metric that backs this claim)"
            )
            continue
        declared = assertion.get("measurement", _MISSING) if isinstance(assertion, Mapping) else _MISSING
        if declared is _MISSING:
            errors.extend(assert_measurements(data, assertion_id))
        elif not _same_json_value(declared, pinned):
            errors.append(
                f"assertion measurement does not match the reviewed binding: {assertion_id} "
                f"(reviewed={_render(pinned)} declared={_render(declared)})"
            )

    if profile.get("behavioral_runner_required") is True:
        runner = procedure.get("runner")
        if not isinstance(runner, Mapping):
            errors.append("behavioral verification procedure lacks a reviewed runner")
            return errors
        runner_rel = _safe_relative_path(runner.get("path"))
        runner_hash = runner.get("sha256")
        runner_index = runner.get("argv_index")
        if runner_rel is None or not isinstance(runner_hash, str) or not isinstance(runner_index, int):
            errors.append("reviewed behavioral runner is malformed")
            return errors
        runner_path = (root / runner_rel).resolve()
        if root.resolve() not in runner_path.parents or not runner_path.is_file():
            errors.append("reviewed behavioral runner path is missing or unsafe")
            return errors
        if _sha256(runner_path) != runner_hash:
            errors.append("reviewed behavioral runner hash does not match")
        if runner_index < 0 or runner_index >= len(argv) or argv[runner_index] not in {runner_rel, str(runner_path)}:
            errors.append("evidence command does not invoke the reviewed behavioral runner")
        if argv and argv[0] in DISALLOWED_BEHAVIORAL_COMMANDS:
            errors.append("placeholder command cannot qualify behavioral execution")
    return errors


def validate_requirement_evidence(
    data: Mapping[str, object],
    *,
    evidence_path: Path,
    root: Path,
    verification_id: str,
    verification_layer: str,
    current_commit: str,
    expected_lane: str | None = None,
    required_artifact_hash: str | None = None,
) -> list[str]:
    """Apply the reviewed verification profile to one requirement/evidence claim."""
    errors = assess_evidence(data, current_commit=current_commit, expected_lane=expected_lane)
    contract, contract_errors = load_verification_contract(root)
    errors.extend(contract_errors)
    if contract is None:
        return errors
    profile = _profile_for_layer(contract, verification_layer)
    if profile is None:
        return errors + [f"no reviewed verification profile maps layer {verification_layer!r}"]
    if profile.get("qualifiable") is not True:
        errors.append(f"verification layer {verification_layer!r} is not qualifiable")
    if data.get("verification_id") != verification_id:
        errors.append("evidence verification_id does not match the requirement")
    if data.get("evidence_type") not in profile.get("permitted_evidence_types", []):
        errors.append(
            f"evidence_type {data.get('evidence_type')!r} is not permitted by profile {profile.get('id')!r}"
        )
    execution = data.get("execution")
    if not isinstance(execution, Mapping) or execution.get("environment_class") != profile.get("execution_environment"):
        errors.append("evidence execution environment does not match the verification profile")
    environment = data.get("environment")
    if profile.get("native_platform_required") is True:
        if not isinstance(environment, Mapping) or environment.get("native_or_vm") != "native":
            errors.append("verification profile requires native platform evidence")
    elif profile.get("execution_environment") == "controlled":
        if not isinstance(environment, Mapping) or environment.get("native_or_vm") != "controlled":
            errors.append("controlled verification profile requires a controlled environment declaration")

    artifact_policy = profile.get("artifact_binding")
    if artifact_policy == "required":
        errors.extend(_artifact_binding_errors(data, required_artifact_hash=required_artifact_hash))
        errors.extend(verify_artifact_under_test(data, root=root))
    elif artifact_policy == "forbidden":
        if data.get("artifact_under_test") is not None:
            errors.append("verification profile forbids artifact_under_test binding")
        if required_artifact_hash is not None:
            errors.append("verification profile cannot satisfy a required release artifact hash")
    else:
        errors.append("verification profile has an invalid artifact binding policy")

    errors.extend(
        _verify_reviewed_procedure(
            data,
            root=root,
            contract=contract,
            profile=profile,
            verification_id=verification_id,
        )
    )
    index, index_errors = load_evidence_index(root)
    errors.extend(index_errors)
    errors.extend(verify_evidence_registration(evidence_path, root=root, index=index))
    errors.extend(verify_artifact_hashes(data, root=root))
    return errors
