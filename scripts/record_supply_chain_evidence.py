#!/usr/bin/env python3
"""Verify an existing candidate supply-chain bundle and record VER-OPS-004."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shlex
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import jsonschema

import build_candidate_supply_chain as supply
import qualify_candidate_known_phrase as package_identity
import recorder_safety

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = "agent/evidence/P10/VER-OPS-004.json"
INDEX = "agent/evidence/INDEX.json"
PROFILE_ID = "package-artifact-v1"
PROCEDURE_ID = "exact-package-supply-chain-v1"
# The reviewed claim list; the per-claim measurement bindings live with the
# procedure below and are mirrored by spec/evidence-verification-profiles.json.
ASSERTIONS = [
    "exact-clean-package-bound",
    "package-checksum-manifest-matched",
    "spdx-dependency-license-inventory-present",
    "spdx-package-checksums-verified",
    "in-toto-subject-matched",
    "immutable-build-identity-matched",
    "signing-state-truthful",
]
MAX_BUNDLE_FILE_BYTES = 16 * 1024 * 1024


def measurement(metric: str, comparator: str, expected: object = None) -> dict[str, object]:
    binding: dict[str, object] = {"metric": metric, "comparator": comparator}
    if comparator not in {"is_true", "is_false"}:
        binding["expected"] = expected
    return binding


def measured(assertion: str, binding: dict[str, object]) -> dict[str, object]:
    return {"id": assertion, "result": "pass", "measurement": binding}

class VerificationError(RuntimeError):
    pass

def _sha(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()

def _json_file(path: Path, maximum: int = MAX_BUNDLE_FILE_BYTES) -> tuple[bytes, dict[str, Any]]:
    try:
        payload = supply._read_regular_file(path, maximum)
        value = json.loads(payload.decode("utf-8"))
    except (supply.SupplyChainError, UnicodeDecodeError, json.JSONDecodeError, OSError) as exc:
        raise VerificationError(f"invalid bundle file: {path.name}") from exc
    if not isinstance(value, dict):
        raise VerificationError(f"invalid bundle file: {path.name}")
    return payload, value

def _artifact(manifest: dict[str, Any], kind: str) -> dict[str, Any]:
    matches = [item for item in manifest["artifacts"] if item.get("kind") == kind]
    if len(matches) != 1:
        raise VerificationError(f"release manifest must contain one {kind} artifact")
    return matches[0]

UNASSERTED_LICENSES = frozenset({"", "NOASSERTION", "NONE"})


def _verify_inventory(
    packages: list[Any], lock_packages: list[supply.LockPackage], product_name: str
) -> dict[str, int]:
    """Every shipped component has a real licence and a Cargo.lock-matched checksum.

    ``NOASSERTION`` (or ``NONE``) is not a licence: it fails. Each dependency
    must be in the source commit's Cargo.lock at the same version, and when
    Cargo.lock pins a checksum the SBOM must carry exactly that checksum.
    """
    locked = {(item.name, item.version): item for item in lock_packages}
    unasserted = 0
    mismatched = 0
    unknown = 0
    verified = 0
    for item in packages:
        if not isinstance(item, dict):
            raise VerificationError("SPDX dependency or license inventory is malformed")
        for field in ("licenseDeclared", "licenseConcluded"):
            if str(item.get(field, "")).strip().upper() in UNASSERTED_LICENSES:
                unasserted += 1
        if item.get("name") == product_name:
            continue
        lock = locked.get((item.get("name"), item.get("versionInfo")))
        if lock is None:
            unknown += 1
            continue
        observed = [
            checksum.get("checksumValue")
            for checksum in item.get("checksums", []) or []
            if isinstance(checksum, dict) and checksum.get("algorithm") == "SHA256"
        ]
        if lock.checksum is None:
            if observed:
                mismatched += 1
            continue
        if observed != [lock.checksum]:
            mismatched += 1
        else:
            verified += 1
    if unasserted:
        raise VerificationError(
            f"SPDX inventory has {unasserted} NOASSERTION/NONE licence fields for shipped components"
        )
    if unknown:
        raise VerificationError(f"SPDX inventory has {unknown} components absent from Cargo.lock")
    if mismatched:
        raise VerificationError(f"SPDX inventory has {mismatched} checksums that do not match Cargo.lock")
    if verified < 1:
        raise VerificationError("SPDX inventory verified no checksum against Cargo.lock")
    return {
        "unasserted_license_fields": unasserted,
        "components_absent_from_lockfile": unknown,
        "lockfile_checksum_mismatches": mismatched,
        "lockfile_checksums_verified": verified,
    }


def _verify_bundle(package: Path, bundle: Path, source_root: Path) -> dict[str, Any]:
    try:
        clean_root, head = package_identity._require_clean_source(
            source_root, package_identity.HostOperations(), 5.0
        )
        if clean_root != source_root.resolve():
            raise VerificationError("source root mismatch")
        package_payload = package_identity._read_regular_file(
            package, package_identity.MAX_CANDIDATE_BYTES, "candidate"
        )
        identity, _ = package_identity._prepare_package(package_payload, head)
    except (package_identity.InputError, OSError) as exc:
        raise VerificationError(str(exc)) from exc
    package_hash = _sha(package_payload)
    package_size = len(package_payload)

    if not bundle.is_dir() or bundle.is_symlink():
        raise VerificationError("bundle must be a directory")
    release_payload, manifest = _json_file(bundle / supply.RELEASE_ARTIFACTS_NAME)
    spdx_payload, spdx = _json_file(bundle / supply.SPDX_NAME)
    provenance_payload, provenance = _json_file(bundle / supply.PROVENANCE_NAME)
    result_payload, summary = _json_file(bundle / supply.RESULT_NAME)
    try:
        schema = json.loads((source_root / "spec/schemas/release-artifacts.schema.json").read_text())
        jsonschema.validate(instance=manifest, schema=schema)
    except (OSError, json.JSONDecodeError, jsonschema.ValidationError) as exc:
        raise VerificationError("release manifest validation failed") from exc
    signing = manifest["signing"]
    if signing != {
        "signed": False,
        "status": "not-supported",
        "key_id": None,
        "signature_path": None,
    }:
        raise VerificationError("signing state is inconsistent")
    try:
        supply._validate_release_artifacts(manifest)
    except supply.SupplyChainError as exc:
        raise VerificationError("release manifest validation failed") from exc

    if manifest["git_commit"] != head or manifest["git_dirty"] is not False:
        raise VerificationError("release manifest source identity mismatch")
    if manifest["version"] != identity.version:
        raise VerificationError("release manifest package version mismatch")
    if manifest["build_identity"] != f"narrivy-{identity.version}+{head[:7]}":
        raise VerificationError("immutable build identity mismatch")

    deb = _artifact(manifest, "deb")
    sbom = _artifact(manifest, "sbom")
    attestation = _artifact(manifest, "provenance")
    try:
        package_rel = package.relative_to(source_root).as_posix()
        bundle_rel = bundle.relative_to(source_root).as_posix()
    except ValueError as exc:
        raise VerificationError("package or bundle is outside the source root") from exc
    if (
        deb["path"] != package_rel
        or deb["sha256"] != package_hash
        or deb["size_bytes"] != package_size
    ):
        raise VerificationError("package checksum manifest mismatch")
    expected_files = {
        supply.SPDX_NAME: (spdx_payload, sbom, manifest["sbom_path"]),
        supply.PROVENANCE_NAME: (provenance_payload, attestation, manifest["provenance_path"]),
    }
    for name, (payload, entry, declared_path) in expected_files.items():
        expected_path = f"{bundle_rel}/{name}"
        if entry["path"] != expected_path or declared_path != expected_path:
            raise VerificationError(f"{name} manifest path mismatch")
        if entry["sha256"] != _sha(payload) or entry["size_bytes"] != len(payload):
            raise VerificationError(f"{name} manifest checksum mismatch")

    packages = spdx.get("packages")
    relationships = spdx.get("relationships")
    if spdx.get("spdxVersion") != "SPDX-2.3" or not isinstance(packages, list) or len(packages) < 2:
        raise VerificationError("SPDX dependency inventory is missing")
    if not isinstance(relationships, list) or not any(x.get("relationshipType") == "DEPENDS_ON" for x in relationships if isinstance(x, dict)):
        raise VerificationError("SPDX dependency relationships are missing")
    products = [
        item
        for item in packages
        if isinstance(item, dict)
        and item.get("name") == supply.PACKAGE_NAME
        and item.get("versionInfo") == identity.version
    ]
    if len(products) != 1:
        raise VerificationError("SPDX product identity is missing")
    checksummed = 0
    for item in packages:
        if not isinstance(item, dict) or not all(isinstance(item.get(k), str) and item[k] for k in ("name", "versionInfo", "licenseDeclared", "licenseConcluded")):
            raise VerificationError("SPDX dependency or license inventory is malformed")
        checksums = item.get("checksums", [])
        if checksums:
            if not isinstance(checksums, list) or any(
                not isinstance(checksum, dict)
                or checksum.get("algorithm") != "SHA256"
                or not isinstance(checksum.get("checksumValue"), str)
                or supply.SHA256_RE.fullmatch(checksum["checksumValue"]) is None
                for checksum in checksums
            ):
                raise VerificationError("SPDX package checksum is malformed")
            checksummed += 1
    if checksummed < 1:
        raise VerificationError("SPDX package checksum inventory is missing")
    try:
        lock_packages = supply._parse_lockfile(
            supply._read_lockfile(source_root, head, git=supply.GIT, timeout=supply.GIT_TIMEOUT_SECONDS)
        )
    except supply.SupplyChainError as exc:
        raise VerificationError(f"Cargo.lock at the source commit is unavailable: {exc}") from exc
    inventory = _verify_inventory(packages, lock_packages, supply.PACKAGE_NAME)

    subject = provenance.get("subject")
    predicate = provenance.get("predicate")
    if (
        provenance.get("_type") != "https://in-toto.io/Statement/v1"
        or not isinstance(subject, list) or len(subject) != 1
        or subject[0].get("name") != deb["path"]
        or subject[0].get("digest") != {"sha256": package_hash}
        or not isinstance(predicate, dict)
        or predicate.get("classification") != supply.PROVENANCE_CLASSIFICATION
        or predicate.get("package") != supply.PACKAGE_NAME
        or predicate.get("source_commit") != head
        or predicate.get("version") != identity.version
        or predicate.get("embedded") != identity.embedded
        or predicate.get("cargo_lock_hash")
        != manifest["builder_identity"]["cargo_lock_hash"]
        or predicate.get("builder")
        != {
            key: manifest["builder_identity"][key]
            for key in ("image", "image_digest", "os", "toolchain")
        }
    ):
        raise VerificationError("in-toto package subject or predicate mismatch")

    files = summary.get("files")
    if (
        summary.get("classification") != supply.BUNDLE_CLASSIFICATION
        or summary.get("candidate_sha256") != package_hash
        or summary.get("candidate_size_bytes") != package_size
        or summary.get("git_commit") != head
        or summary.get("version") != identity.version
        or summary.get("release_qualified") is not False
        or summary.get("requirement_evidence") is not False
        or not isinstance(files, dict)
    ):
        raise VerificationError("bundle summary identity mismatch")
    for name, payload in ((supply.SPDX_NAME, spdx_payload), (supply.PROVENANCE_NAME, provenance_payload), (supply.RELEASE_ARTIFACTS_NAME, release_payload)):
        if files.get(name) != {"sha256": _sha(payload), "size_bytes": len(payload)}:
            raise VerificationError(f"bundle summary checksum mismatch: {name}")

    # Every claim below is recorded as the comparison that was actually made,
    # not as a bare "pass" label. The raises above guarantee a record is only
    # produced when every one of these is true.
    return {
        "head": head, "identity": identity, "package_hash": package_hash,
        "package_size": package_size, "manifest": manifest, "spdx": spdx,
        "spdx_hash": _sha(spdx_payload), "provenance_hash": _sha(provenance_payload),
        "release_hash": _sha(release_payload), "summary_hash": _sha(result_payload),
        "dependency_count": len(packages) - 1, "checksummed_dependency_count": checksummed,
        "signing": signing,
        "inventory": inventory,
        "checks": {
            "manifest_checksum_match": (
                deb["sha256"] == package_hash and deb["size_bytes"] == package_size
            ),
            "source_commit_match": manifest["git_commit"] == head,
            "package_checksum_manifest_matched": deb["path"] == package_rel,
            "spdx_dependency_license_inventory_present": (
                spdx.get("spdxVersion") == "SPDX-2.3"
                and len(packages) >= 2
                and any(
                    isinstance(x, dict) and x.get("relationshipType") == "DEPENDS_ON"
                    for x in relationships
                )
            ),
            "spdx_package_checksums_verified": checksummed >= 1
            and inventory["lockfile_checksum_mismatches"] == 0
            and inventory["lockfile_checksums_verified"] >= 1,
            "spdx_licenses_asserted": inventory["unasserted_license_fields"] == 0,
            "in_toto_subject_matched": subject[0].get("digest") == {"sha256": package_hash},
            "immutable_build_identity_matched": (
                manifest["build_identity"] == f"narrivy-{identity.version}+{head[:7]}"
            ),
            "signing_state_truthful": signing["signed"] is False
            and signing["status"] == "not-supported",
        },
    }

def _atomic_json(path: Path, value: object) -> bytes:
    payload = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(payload); stream.flush(); os.fsync(stream.fileno())
        os.chmod(temporary, 0o644); os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)
    return payload

def record(package: Path, bundle: Path, source_root: Path, evidence_path: Path, index_path: Path, command: list[str]) -> dict[str, Any]:
    index = json.loads(index_path.read_text())
    if not recorder_safety.replaceable(evidence_path, index, source_root):
        raise VerificationError("evidence output is current, unindexed v2, or does not match its INDEX hash")
    started = datetime.now(timezone.utc)
    verified = _verify_bundle(package, bundle, source_root)
    ended = datetime.now(timezone.utc)
    package_rel = package.relative_to(source_root).as_posix()
    bundle_rel = bundle.relative_to(source_root).as_posix()
    evidence = {
        "schema_version": 3, "verification_id": "VER-OPS-004", "requirement_ids": ["OPS-004"],
        "phase": "P10", "scenario_id": PROCEDURE_ID, "git_commit": verified["head"],
        "git_dirty": False, "build_id": verified["manifest"]["build_identity"],
        "package_sha256": verified["package_hash"],
        "environment": {"id": "supply-chain-verification", "native_or_vm": "controlled",
            "os": platform.system(), "os_version": platform.release(), "kernel": platform.release(),
            "desktop": "none", "session": "noninteractive", "cpu": platform.machine(),
            "gpu": None, "npu": None, "device_details": "content verification only",
            "microphone": None, "input_backend": None, "display_scale": None,
            "qualification_lane": "exact-package-supply-chain", "lane_available": True},
        "command": shlex.join(command), "scenario_version": "1",
        "scenario_hash": _sha(Path(__file__).read_bytes()),
        "started_at": started.isoformat(), "ended_at": ended.isoformat(), "exit_code": 0,
        "result": "pass", "pass": True,
        "metrics": {"dependency_count": verified["dependency_count"],
            "checksummed_dependency_count": verified["checksummed_dependency_count"],
            "signing_status": verified["signing"]["status"], "signed": verified["signing"]["signed"],
            "package_sha256": verified["package_hash"],
            "build_identity": verified["manifest"]["build_identity"],
            "spdx_relationships": sum(
                1 for x in verified["spdx"].get("relationships", [])
                if isinstance(x, dict) and x.get("relationshipType") == "DEPENDS_ON"
            ),
            "inventory": verified["inventory"],
            "checks": verified["checks"]},
        "artifact_hashes": {package_rel: verified["package_hash"],
            f"{bundle_rel}/{supply.SPDX_NAME}": verified["spdx_hash"],
            f"{bundle_rel}/{supply.PROVENANCE_NAME}": verified["provenance_hash"],
            f"{bundle_rel}/{supply.RELEASE_ARTIFACTS_NAME}": verified["release_hash"],
            f"{bundle_rel}/{supply.RESULT_NAME}": verified["summary_hash"],
            "scripts/record_supply_chain_evidence.py": _sha(Path(__file__).read_bytes())},
        "artifact_under_test": {"kind": "package", "path": package_rel, "sha256": verified["package_hash"]},
        "procedure": {"id": PROCEDURE_ID, "profile_id": PROFILE_ID,
            "assertions": [
                measured("exact-clean-package-bound", measurement("checks.source_commit_match", "is_true")),
                measured("package-checksum-manifest-matched", measurement("checks.manifest_checksum_match", "is_true")),
                measured("spdx-dependency-license-inventory-present", measurement("inventory.unasserted_license_fields", "eq", 0)),
                measured("spdx-package-checksums-verified", measurement("inventory.lockfile_checksum_mismatches", "eq", 0)),
                measured("in-toto-subject-matched", measurement("checks.in_toto_subject_matched", "is_true")),
                measured("immutable-build-identity-matched", measurement("checks.immutable_build_identity_matched", "is_true")),
                measured("signing-state-truthful", measurement("signing_status", "eq", "not-supported")),
            ]},
        "notes": "Verified the exact package, schema-v3 checksum manifest, SPDX dependency/license fields, in-toto subject, embedded clean-source build identity, and declared signing state. This does not claim native runtime qualification.",
        "evidence_type": "package-artifact",
        "execution": {"lane_available": True, "child_failures_ignored": False,
            "environment_class": "release-package", "substitutions": []},
    }
    jsonschema.validate(instance=evidence, schema=json.loads((source_root / "spec/schemas/evidence.schema.json").read_text()))
    payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode()
    index_schema = json.loads((source_root / "spec/schemas/evidence-index.schema.json").read_text())
    try:
        recorder_safety.commit_records(
            source_root, index_path, index, [(evidence_path, payload)],
            validate_index=lambda value: jsonschema.validate(instance=value, schema=index_schema),
        )
    except recorder_safety.RecorderSafetyError as exc:
        raise VerificationError(str(exc)) from exc
    return evidence

def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("package", type=Path)
    parser.add_argument("bundle", type=Path)
    args = parser.parse_args(argv)
    command = ["python3", "scripts/record_supply_chain_evidence.py", args.package.as_posix(), args.bundle.as_posix()]
    try:
        evidence = record((ROOT / args.package).resolve(), (ROOT / args.bundle).resolve(), ROOT, ROOT / EVIDENCE, ROOT / INDEX, command)
    except (VerificationError, OSError, ValueError, json.JSONDecodeError, jsonschema.ValidationError) as exc:
        print(f"record_supply_chain_evidence: {exc}", file=sys.stderr); return 1
    print(json.dumps({"evidence": EVIDENCE, "package_sha256": evidence["package_sha256"], "result": "pass"}, sort_keys=True))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
