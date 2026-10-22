#!/usr/bin/env python3
"""Bind an exact candidate package to an unqualified SPDX/provenance bundle.

The entrypoint accepts explicit absolute paths for an existing candidate ``.deb``,
the source Git toplevel, and a new output directory, plus the expected package
SHA-256 and caller-supplied builder identity strings. It never infers
reproducibility, signing, qualification, or release status.

The bundle is classified ``candidate-supply-chain-unqualified``. It is
bookkeeping for one exact package, not native qualification, signing,
immutable-builder proof, or release-gate evidence.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import io
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Mapping

PACKAGE_NAME = "narrivy"
BUILD_INFO_MEMBER = "usr/share/narrivy/build-info.json"
CONTROL_MEMBER = "control"
CLEAN_CLASSIFICATION = "clean-unqualified-release-candidate"
BUNDLE_CLASSIFICATION = "candidate-supply-chain-unqualified"
PROVENANCE_CLASSIFICATION = "unqualified-candidate-provenance"
SPDX_NAME = "spdx.json"
PROVENANCE_NAME = "provenance.json"
RELEASE_ARTIFACTS_NAME = "release-artifacts.json"
RESULT_NAME = "result.json"
GIT = "/usr/bin/git"
GIT_TIMEOUT_SECONDS = 5
GIT_ENV = {
    "PATH": "/usr/bin:/bin",
    "LC_ALL": "C",
    "TZ": "UTC",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_OPTIONAL_LOCKS": "0",
    "GIT_TERMINAL_PROMPT": "0",
}
AR_MAGIC = b"!<arch>\n"
AR_HEADER_SIZE = 60
MAX_CANDIDATE_BYTES = 512 * 1024 * 1024
MAX_BUILD_INFO_BYTES = 64 * 1024
MAX_CONTROL_BYTES = 16 * 1024
MAX_LOCKFILE_BYTES = 8 * 1024 * 1024
MAX_GIT_TEXT_BYTES = 4096
MAX_AR_MEMBERS = 8
MAX_TAR_MEMBERS = 4096
MAX_TAR_DECLARED_BYTES = MAX_CANDIDATE_BYTES
MAX_SOURCE_DATE_EPOCH = 253_402_300_799
SHA256_RE = re.compile(r"[0-9a-f]{64}")
COMMIT_RE = re.compile(r"[0-9a-f]{40}")
SPDX_TOKEN_RE = re.compile(r"[^A-Za-z0-9.-]+")
SCHEMA_PATH_RE = re.compile(r"^(?!/)(?!.*(?:^|/)\.\.(?:/|$)).+$")
EMBEDDED_KEYS = (
    "narrivyd",
    "narrivy-cli",
    "narrivy-desktop",
    "resident-worker",
    "bundled-model",
)


class SupplyChainError(RuntimeError):
    """Content-free candidate supply-chain failure."""


@dataclass(frozen=True)
class Inputs:
    candidate: Path
    source_root: Path
    output: Path
    expected_sha256: str
    builder_os: str
    builder_image: str
    builder_image_digest: str
    toolchain: str
    git: str = GIT
    timeout: float = GIT_TIMEOUT_SECONDS


@dataclass(frozen=True)
class EmbeddedIdentity:
    package: str
    version: str
    source_head: str
    source_date_epoch: int
    embedded: dict[str, dict[str, object]]


@dataclass(frozen=True)
class LockPackage:
    name: str
    version: str
    source: str | None
    checksum: str | None


def _canonical_json(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def _digest_of(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _require_identity_text(value: str) -> None:
    if (
        not isinstance(value, str)
        or not value
        or value.strip() != value
        or any(ord(character) < 32 for character in value)
    ):
        raise SupplyChainError("builder identity is invalid")


def _require_sha256(value: str) -> str:
    if not isinstance(value, str) or not SHA256_RE.fullmatch(value):
        raise SupplyChainError("expected SHA-256 must be 64 lowercase hex characters")
    return value


def _run_git(
    source_root: Path,
    arguments: tuple[str, ...],
    *,
    git: str,
    timeout: float,
    maximum: int,
) -> bytes:
    try:
        result = subprocess.run(
            [git, "-C", os.fspath(source_root), *arguments],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env=GIT_ENV,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise SupplyChainError("source git command timed out") from error
    except OSError as error:
        raise SupplyChainError("source git command failed") from error
    if result.returncode != 0 or len(result.stdout) > maximum:
        raise SupplyChainError("source git command failed")
    return result.stdout


def _git_text(
    source_root: Path,
    arguments: tuple[str, ...],
    *,
    git: str,
    timeout: float,
    maximum: int = MAX_GIT_TEXT_BYTES,
) -> str:
    payload = _run_git(
        source_root, arguments, git=git, timeout=timeout, maximum=maximum
    )
    try:
        text = payload.decode("utf-8")
    except UnicodeDecodeError as error:
        raise SupplyChainError("source git command failed") from error
    return text.rstrip("\n")


def _require_source_root(source_root: Path, *, git: str, timeout: float) -> Path:
    if not source_root.is_absolute():
        raise SupplyChainError("source root must be an existing absolute directory")
    try:
        resolved = source_root.resolve(strict=True)
    except OSError as error:
        raise SupplyChainError("source root must be an existing absolute directory") from error
    if not resolved.is_dir():
        raise SupplyChainError("source root must be an existing absolute directory")
    toplevel = _git_text(
        source_root,
        ("rev-parse", "--show-toplevel"),
        git=git,
        timeout=timeout,
    )
    try:
        resolved_toplevel = Path(toplevel).resolve(strict=True)
    except OSError as error:
        raise SupplyChainError("source root is not the git toplevel") from error
    if resolved_toplevel != resolved:
        raise SupplyChainError("source root is not the git toplevel")
    return resolved


def _inside_repository(path: Path, source_root: Path) -> bool:
    try:
        path.relative_to(source_root)
    except ValueError:
        return False
    return True


def _posix_relative(path: Path, source_root: Path) -> str:
    try:
        relative = path.relative_to(source_root)
    except ValueError as error:
        raise SupplyChainError("path is outside the repository") from error
    text = relative.as_posix()
    if text in ("", ".") or not SCHEMA_PATH_RE.fullmatch(text):
        raise SupplyChainError("path is outside the repository")
    return text


def _read_regular_file(path: Path, maximum: int) -> bytes:
    try:
        observed = path.lstat()
    except OSError as error:
        raise SupplyChainError("candidate is unavailable") from error
    if stat.S_ISLNK(observed.st_mode) or not stat.S_ISREG(observed.st_mode):
        raise SupplyChainError("candidate must be a regular non-symlink file")
    if observed.st_size <= 0 or observed.st_size > maximum:
        raise SupplyChainError("candidate size is invalid")
    descriptor = -1
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        opened = os.fstat(descriptor)
        if (
            opened.st_dev,
            opened.st_ino,
            opened.st_mode,
            opened.st_size,
        ) != (
            observed.st_dev,
            observed.st_ino,
            observed.st_mode,
            observed.st_size,
        ):
            raise SupplyChainError("candidate identity changed")
        chunks: list[bytes] = []
        total = 0
        while True:
            chunk = os.read(descriptor, min(64 * 1024, maximum + 1 - total))
            if not chunk:
                break
            chunks.append(chunk)
            total += len(chunk)
            if total > maximum:
                raise SupplyChainError("candidate size is invalid")
        if total != opened.st_size:
            raise SupplyChainError("candidate identity changed")
        return b"".join(chunks)
    except OSError as error:
        raise SupplyChainError("candidate is unavailable") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def _require_candidate(
    candidate: Path, expected_sha256: str, source_root: Path
) -> tuple[Path, bytes, str]:
    if not candidate.is_absolute() or candidate.name in ("", ".", ".."):
        raise SupplyChainError("candidate path must be an absolute file path")
    try:
        parent = candidate.parent.resolve(strict=True)
    except OSError as error:
        raise SupplyChainError("candidate is unavailable") from error
    resolved = parent / candidate.name
    if not _inside_repository(resolved, source_root):
        raise SupplyChainError("candidate path is outside the repository")
    payload = _read_regular_file(candidate, MAX_CANDIDATE_BYTES)
    digest = _digest_of(payload)
    if digest != expected_sha256:
        raise SupplyChainError("candidate SHA-256 does not match the expected value")
    return resolved, payload, digest


def _require_new_output(output: Path, source_root: Path) -> Path:
    if not output.is_absolute() or output.name in ("", ".", ".."):
        raise SupplyChainError("output path must be an absolute directory path")
    if os.path.lexists(output):
        raise SupplyChainError("output directory already exists")
    try:
        parent = output.parent.resolve(strict=True)
    except OSError as error:
        raise SupplyChainError("output parent directory does not exist") from error
    if not parent.is_dir():
        raise SupplyChainError("output parent directory does not exist")
    destination = parent / output.name
    if not _inside_repository(destination, source_root):
        raise SupplyChainError("output path is outside the repository")
    if os.path.lexists(destination):
        raise SupplyChainError("output directory already exists")
    return destination


def _parse_debian_archive(payload: bytes) -> dict[str, bytes]:
    if not payload.startswith(AR_MAGIC):
        raise SupplyChainError("candidate is not a Debian archive")
    offset = len(AR_MAGIC)
    members: dict[str, bytes] = {}
    while offset < len(payload):
        if len(members) >= MAX_AR_MEMBERS:
            raise SupplyChainError("Debian archive has too many members")
        if offset + AR_HEADER_SIZE > len(payload):
            raise SupplyChainError("candidate is not a Debian archive")
        header = payload[offset : offset + AR_HEADER_SIZE]
        if header[58:60] != b"`\n":
            raise SupplyChainError("candidate is not a Debian archive")
        try:
            name = header[0:16].decode("ascii").strip()
            size_text = header[48:58].decode("ascii").strip()
            size = int(size_text, 10)
        except (UnicodeDecodeError, ValueError) as error:
            raise SupplyChainError("candidate is not a Debian archive") from error
        if name.endswith("/"):
            name = name[:-1]
        if (
            not name
            or name in (".", "..")
            or "/" in name
            or size < 0
            or offset + AR_HEADER_SIZE + size > len(payload)
        ):
            raise SupplyChainError("candidate is not a Debian archive")
        offset += AR_HEADER_SIZE
        data = payload[offset : offset + size]
        offset += size
        if size % 2 == 1 and offset < len(payload):
            offset += 1
        if name in members:
            raise SupplyChainError("Debian archive contains a duplicate member")
        members[name] = data
    debian_binary = members.get("debian-binary")
    if debian_binary not in (b"2.0\n", b"2.0"):
        raise SupplyChainError("candidate is not a Debian archive")
    return members


def _archive_payload(members: Mapping[str, bytes], stem: str) -> tuple[str, bytes]:
    matches = [
        name
        for name in members
        if name == stem or name.startswith(stem + ".")
    ]
    if len(matches) != 1:
        raise SupplyChainError("Debian archive membership is invalid")
    name = matches[0]
    return name, members[name]


def _open_tar(payload: bytes, archive_name: str) -> tarfile.TarFile:
    if archive_name.endswith(".tar.gz"):
        mode = "r:gz"
    elif archive_name.endswith(".tar.xz"):
        mode = "r:xz"
    elif archive_name.endswith(".tar"):
        mode = "r:"
    else:
        raise SupplyChainError("Debian archive compression is unsupported")
    try:
        return tarfile.open(fileobj=io.BytesIO(payload), mode=mode)
    except (tarfile.TarError, OSError, EOFError) as error:
        raise SupplyChainError("Debian archive is invalid") from error


def _normalize_tar_name(name: str) -> str:
    normalized = name.removeprefix("./") or "."
    path = PurePosixPath(normalized)
    if path.is_absolute() or ".." in path.parts:
        raise SupplyChainError("candidate archive path is unsafe")
    return normalized


def _required_tar_regular_file(
    payload: bytes,
    archive_name: str,
    member_name: str,
    maximum: int,
    label: str,
) -> bytes:
    found: bytes | None = None
    member_count = 0
    declared_bytes = 0
    try:
        archive = _open_tar(payload, archive_name)
    except SupplyChainError:
        raise
    try:
        for member in archive:
            member_count += 1
            declared_bytes += member.size
            if (
                member_count > MAX_TAR_MEMBERS
                or member.size < 0
                or declared_bytes > MAX_TAR_DECLARED_BYTES
            ):
                raise SupplyChainError("Debian archive exceeds bounded tar limits")
            normalized = _normalize_tar_name(member.name)
            if normalized != member_name:
                continue
            if found is not None:
                raise SupplyChainError(f"{label} member is duplicated")
            if not member.isfile() or member.issym() or member.islnk():
                raise SupplyChainError(f"{label} member is not a regular file")
            if member.size > maximum:
                raise SupplyChainError(f"{label} member is oversized")
            if member.size < 1:
                raise SupplyChainError(f"{label} member is invalid")
            extracted = archive.extractfile(member)
            if extracted is None:
                raise SupplyChainError(f"{label} member is not a regular file")
            data = extracted.read(maximum + 1)
            if len(data) != member.size:
                raise SupplyChainError(f"{label} member is invalid")
            found = data
    except (tarfile.TarError, OSError, EOFError) as error:
        raise SupplyChainError("Debian archive is invalid") from error
    finally:
        archive.close()
    if found is None:
        raise SupplyChainError(f"{label} member is missing")
    return found


def _digest_size(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise SupplyChainError("build identity is not a clean unqualified candidate")
    digest = value.get("sha256")
    size = value.get("size_bytes")
    if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
        raise SupplyChainError("build identity is not a clean unqualified candidate")
    if isinstance(size, bool) or not isinstance(size, int) or size <= 0:
        raise SupplyChainError("build identity is not a clean unqualified candidate")
    return {"sha256": digest, "size_bytes": size}


def _parse_build_info(payload: bytes) -> EmbeddedIdentity:
    try:
        document = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SupplyChainError("build identity is not a clean unqualified candidate") from error
    if not isinstance(document, dict):
        raise SupplyChainError("build identity is not a clean unqualified candidate")
    inputs = document.get("inputs")
    if (
        document.get("schema_version") != 2
        or document.get("classification") != CLEAN_CLASSIFICATION
        or document.get("git_dirty") is not False
        or document.get("package") != PACKAGE_NAME
        or not isinstance(document.get("version"), str)
        or not document["version"]
        or not isinstance(document.get("source_head"), str)
        or not COMMIT_RE.fullmatch(document["source_head"])
        or isinstance(document.get("source_date_epoch"), bool)
        or not isinstance(document.get("source_date_epoch"), int)
        or document["source_date_epoch"] <= 0
        or document["source_date_epoch"] > MAX_SOURCE_DATE_EPOCH
        or not isinstance(inputs, dict)
    ):
        raise SupplyChainError("build identity is not a clean unqualified candidate")
    embedded = {key: _digest_size(inputs.get(key)) for key in EMBEDDED_KEYS}
    return EmbeddedIdentity(
        package=PACKAGE_NAME,
        version=document["version"],
        source_head=document["source_head"],
        source_date_epoch=document["source_date_epoch"],
        embedded=embedded,
    )


def _parse_control(payload: bytes) -> tuple[str, str]:
    try:
        text = payload.decode("utf-8")
    except UnicodeDecodeError as error:
        raise SupplyChainError("Debian control is invalid") from error
    if "\0" in text:
        raise SupplyChainError("Debian control is invalid")
    package = ""
    version = ""
    for line in text.splitlines():
        if line.startswith("Package:"):
            package = line[len("Package:") :].strip()
        elif line.startswith("Version:"):
            version = line[len("Version:") :].strip()
    if not package or not version:
        raise SupplyChainError("Debian control identity is missing")
    return package, version


def _require_commit(source_root: Path, commit: str, *, git: str, timeout: float) -> None:
    try:
        kind = _git_text(
            source_root,
            ("cat-file", "-t", commit),
            git=git,
            timeout=timeout,
        )
    except SupplyChainError as error:
        if str(error) == "source git command failed":
            raise SupplyChainError("source commit is not in the repository") from error
        raise
    if kind != "commit":
        raise SupplyChainError("source commit is not in the repository")


def _read_lockfile(
    source_root: Path, commit: str, *, git: str, timeout: float
) -> bytes:
    return _run_git(
        source_root,
        ("cat-file", "blob", f"{commit}:Cargo.lock"),
        git=git,
        timeout=timeout,
        maximum=MAX_LOCKFILE_BYTES,
    )


def _parse_lockfile(payload: bytes) -> list[LockPackage]:
    try:
        document = tomllib.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise SupplyChainError("Cargo.lock is malformed") from error
    packages = document.get("package")
    if not isinstance(packages, list) or not packages:
        raise SupplyChainError("Cargo.lock is malformed")
    unique: dict[tuple[str, str], LockPackage] = {}
    for item in packages:
        if not isinstance(item, dict):
            raise SupplyChainError("Cargo.lock is malformed")
        name = item.get("name")
        version = item.get("version")
        source = item.get("source")
        checksum = item.get("checksum")
        if not isinstance(name, str) or not name or not isinstance(version, str) or not version:
            raise SupplyChainError("Cargo.lock is malformed")
        if source is not None and (not isinstance(source, str) or not source):
            raise SupplyChainError("Cargo.lock is malformed")
        if checksum is not None and (
            not isinstance(checksum, str) or not SHA256_RE.fullmatch(checksum)
        ):
            raise SupplyChainError("Cargo.lock is malformed")
        parsed = LockPackage(name, version, source, checksum)
        key = (name, version)
        existing = unique.get(key)
        if existing is not None and existing != parsed:
            raise SupplyChainError("Cargo.lock is malformed")
        unique[key] = parsed
    return sorted(unique.values(), key=lambda item: (item.name.lower(), item.version))


def _spdx_token(value: str) -> str:
    token = SPDX_TOKEN_RE.sub("-", value).strip("-")
    return token or "package"


def _allocate_spdx_id(used: set[str], token: str) -> str:
    candidate = f"SPDXRef-Package-{token}"
    index = 2
    while candidate in used:
        candidate = f"SPDXRef-Package-{token}-{index}"
        index += 1
    used.add(candidate)
    return candidate


def _spdx_lock_package(item: LockPackage, used: set[str]) -> dict[str, object]:
    document: dict[str, object] = {
        "SPDXID": _allocate_spdx_id(used, _spdx_token(f"{item.name}-{item.version}")),
        "copyrightText": "NOASSERTION",
        "downloadLocation": item.source if item.source else "NOASSERTION",
        "filesAnalyzed": False,
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": "NOASSERTION",
        "name": item.name,
        "versionInfo": item.version,
    }
    if item.checksum is not None:
        document["checksums"] = [
            {"algorithm": "SHA256", "checksumValue": item.checksum}
        ]
    return document


def _iso8601(epoch: int) -> str:
    return datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )


def _spdx_document(
    identity: EmbeddedIdentity, lock_packages: list[LockPackage]
) -> dict[str, object]:
    used: set[str] = {"SPDXRef-DOCUMENT"}
    product_id = _allocate_spdx_id(used, "narrivy")
    product = {
        "SPDXID": product_id,
        "copyrightText": "NOASSERTION",
        "downloadLocation": "NOASSERTION",
        "filesAnalyzed": False,
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": "NOASSERTION",
        "name": PACKAGE_NAME,
        "versionInfo": identity.version,
    }
    packages = [product]
    relationships = [
        {
            "relatedSpdxElement": product_id,
            "relationshipType": "DESCRIBES",
            "spdxElementId": "SPDXRef-DOCUMENT",
        }
    ]
    for item in lock_packages:
        package = _spdx_lock_package(item, used)
        packages.append(package)
        relationships.append(
            {
                "relatedSpdxElement": package["SPDXID"],
                "relationshipType": "DEPENDS_ON",
                "spdxElementId": product_id,
            }
        )
    return {
        "SPDXID": "SPDXRef-DOCUMENT",
        "creationInfo": {
            "created": _iso8601(identity.source_date_epoch),
            "creators": ["Tool: scripts/build_candidate_supply_chain.py"],
        },
        "dataLicense": "CC0-1.0",
        "documentNamespace": (
            f"https://narrivy.local/spdx/{PACKAGE_NAME}-{identity.version}-"
            f"{identity.source_head}"
        ),
        "name": f"{PACKAGE_NAME}-{identity.version}",
        "packages": packages,
        "relationships": relationships,
        "spdxVersion": "SPDX-2.3",
    }


def _provenance_document(
    identity: EmbeddedIdentity,
    candidate_rel: str,
    candidate_digest: str,
    lock_hash: str,
    inputs: Inputs,
) -> dict[str, object]:
    return {
        "_type": "https://in-toto.io/Statement/v1",
        "predicate": {
            "builder": {
                "image": inputs.builder_image,
                "image_digest": inputs.builder_image_digest,
                "os": inputs.builder_os,
                "toolchain": inputs.toolchain,
            },
            "cargo_lock_hash": lock_hash,
            "classification": PROVENANCE_CLASSIFICATION,
            "embedded": identity.embedded,
            "package": identity.package,
            "source_commit": identity.source_head,
            "source_date_epoch": identity.source_date_epoch,
            "version": identity.version,
        },
        "predicateType": "https://narrivy.local/provenance/unqualified-candidate/v1",
        "subject": [
            {
                "digest": {"sha256": candidate_digest},
                "name": candidate_rel,
            }
        ],
    }


def _artifact(kind: str, path: str, payload: bytes) -> dict[str, object]:
    return {
        "kind": kind,
        "path": path,
        "sha256": _digest_of(payload),
        "size_bytes": len(payload),
    }


def _release_artifacts(
    identity: EmbeddedIdentity,
    inputs: Inputs,
    lock_hash: str,
    candidate_rel: str,
    candidate_digest: str,
    candidate_size: int,
    spdx_rel: str,
    spdx_payload: bytes,
    provenance_rel: str,
    provenance_payload: bytes,
) -> dict[str, object]:
    return {
        "artifacts": [
            {
                "kind": "deb",
                "path": candidate_rel,
                "sha256": candidate_digest,
                "size_bytes": candidate_size,
            },
            _artifact("sbom", spdx_rel, spdx_payload),
            _artifact("provenance", provenance_rel, provenance_payload),
        ],
        "build_identity": f"{PACKAGE_NAME}-{identity.version}+{identity.source_head[:7]}",
        "builder_identity": {
            "cargo_lock_hash": lock_hash,
            "image": inputs.builder_image,
            "image_digest": inputs.builder_image_digest,
            "os": inputs.builder_os,
            "toolchain": inputs.toolchain,
        },
        "git_commit": identity.source_head,
        "git_dirty": False,
        "provenance_path": provenance_rel,
        "sbom_path": spdx_rel,
        "schema_version": 3,
        "signing": {
            "key_id": None,
            "signature_path": None,
            "signed": False,
            "status": "not-supported",
        },
        "version": identity.version,
    }


def _validate_release_artifacts(document: Mapping[str, object]) -> None:
    artifacts = document.get("artifacts")
    builder = document.get("builder_identity")
    signing = document.get("signing")
    if (
        document.get("schema_version") != 3
        or not isinstance(document.get("version"), str)
        or not document["version"]
        or not isinstance(document.get("build_identity"), str)
        or len(document["build_identity"]) < 7
        or not isinstance(document.get("git_commit"), str)
        or not COMMIT_RE.fullmatch(document["git_commit"])
        or document.get("git_dirty") is not False
        or not isinstance(builder, dict)
        or set(builder) != {"os", "image", "image_digest", "toolchain", "cargo_lock_hash"}
        or any(
            not isinstance(builder[key], str) or not builder[key]
            for key in ("os", "image", "image_digest", "toolchain")
        )
        or not isinstance(builder.get("cargo_lock_hash"), str)
        or not SHA256_RE.fullmatch(builder["cargo_lock_hash"])
        or not isinstance(artifacts, list)
        or len(artifacts) < 3
        or not isinstance(document.get("sbom_path"), str)
        or not isinstance(document.get("provenance_path"), str)
        or not isinstance(signing, dict)
        or signing.get("signed") is not False
        or signing.get("status") != "not-supported"
        or signing.get("key_id") is not None
        or signing.get("signature_path") is not None
    ):
        raise SupplyChainError("release artifacts metadata is invalid")
    kinds: set[str] = set()
    for artifact in artifacts:
        if (
            not isinstance(artifact, dict)
            or set(artifact) != {"kind", "path", "sha256", "size_bytes"}
            or artifact.get("kind")
            not in {
                "deb",
                "sbom",
                "provenance",
                "dependency-inventory",
                "license-inventory",
                "other",
            }
            or not isinstance(artifact.get("path"), str)
            or not SCHEMA_PATH_RE.fullmatch(artifact["path"])
            or not isinstance(artifact.get("sha256"), str)
            or not SHA256_RE.fullmatch(artifact["sha256"])
            or isinstance(artifact.get("size_bytes"), bool)
            or not isinstance(artifact.get("size_bytes"), int)
            or artifact["size_bytes"] < 1
        ):
            raise SupplyChainError("release artifacts metadata is invalid")
        kinds.add(artifact["kind"])
    if not {"deb", "sbom", "provenance"}.issubset(kinds):
        raise SupplyChainError("release artifacts metadata is invalid")
    if not any(
        artifact.get("kind") == "sbom" and artifact.get("path") == document["sbom_path"]
        for artifact in artifacts
    ) or not any(
        artifact.get("kind") == "provenance"
        and artifact.get("path") == document["provenance_path"]
        for artifact in artifacts
    ):
        raise SupplyChainError("release artifacts metadata is invalid")


def _write_exclusive(path: Path, payload: bytes) -> None:
    descriptor = os.open(
        path,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
        0o644,
    )
    try:
        view = memoryview(payload)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise SupplyChainError("supply-chain publication write failed")
            view = view[written:]
        os.fchmod(descriptor, 0o644)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _publish(destination: Path, files: Mapping[str, bytes]) -> None:
    if os.path.lexists(destination):
        raise SupplyChainError("output directory already exists")
    temporary: Path | None = None
    published = False
    try:
        temporary = Path(
            tempfile.mkdtemp(
                prefix=".narrivy-supply-chain-", dir=os.fspath(destination.parent)
            )
        )
        for name, payload in files.items():
            if name != Path(name).name or name in ("", ".", ".."):
                raise SupplyChainError("supply-chain output name is invalid")
            _write_exclusive(temporary / name, payload)
        directory = os.open(
            temporary, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW
        )
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
        os.chmod(temporary, 0o755)
        os.rename(temporary, destination)
        published = True
    except FileExistsError as error:
        raise SupplyChainError("output directory already exists") from error
    except OSError as error:
        raise SupplyChainError("supply-chain publication failed") from error
    finally:
        if temporary is not None and not published:
            shutil.rmtree(temporary, ignore_errors=True)


def _summary(
    identity: EmbeddedIdentity,
    destination: Path,
    candidate_digest: str,
    candidate_size: int,
    lock_hash: str,
    spdx_payload: bytes,
    provenance_payload: bytes,
    release_payload: bytes,
) -> dict[str, object]:
    return {
        "candidate_sha256": candidate_digest,
        "candidate_size_bytes": candidate_size,
        "cargo_lock_hash": lock_hash,
        "classification": BUNDLE_CLASSIFICATION,
        "files": {
            PROVENANCE_NAME: {
                "sha256": _digest_of(provenance_payload),
                "size_bytes": len(provenance_payload),
            },
            RELEASE_ARTIFACTS_NAME: {
                "sha256": _digest_of(release_payload),
                "size_bytes": len(release_payload),
            },
            SPDX_NAME: {
                "sha256": _digest_of(spdx_payload),
                "size_bytes": len(spdx_payload),
            },
        },
        "git_commit": identity.source_head,
        "native_hardware_qualified": False,
        "output": str(destination),
        "package": identity.package,
        "release_qualified": False,
        "requirement_evidence": False,
        "source_date_epoch": identity.source_date_epoch,
        "version": identity.version,
    }


def build(inputs: Inputs) -> dict[str, object]:
    expected = _require_sha256(inputs.expected_sha256)
    for value in (
        inputs.builder_os,
        inputs.builder_image,
        inputs.builder_image_digest,
        inputs.toolchain,
    ):
        _require_identity_text(value)
    source_root = _require_source_root(
        inputs.source_root, git=inputs.git, timeout=inputs.timeout
    )
    candidate_path, candidate_bytes, candidate_digest = _require_candidate(
        inputs.candidate, expected, source_root
    )
    destination = _require_new_output(inputs.output, source_root)
    members = _parse_debian_archive(candidate_bytes)
    control_name, control_payload = _archive_payload(members, "control.tar")
    data_name, data_payload = _archive_payload(members, "data.tar")
    control = _required_tar_regular_file(
        control_payload,
        control_name,
        CONTROL_MEMBER,
        MAX_CONTROL_BYTES,
        "Debian control",
    )
    build_info_payload = _required_tar_regular_file(
        data_payload,
        data_name,
        BUILD_INFO_MEMBER,
        MAX_BUILD_INFO_BYTES,
        "build-info",
    )
    identity = _parse_build_info(build_info_payload)
    package_name, package_version = _parse_control(control)
    if package_name != identity.package or package_version != identity.version:
        raise SupplyChainError("package version does not match")
    _require_commit(
        source_root,
        identity.source_head,
        git=inputs.git,
        timeout=inputs.timeout,
    )
    lock_bytes = _read_lockfile(
        source_root,
        identity.source_head,
        git=inputs.git,
        timeout=inputs.timeout,
    )
    lock_packages = _parse_lockfile(lock_bytes)
    lock_hash = _digest_of(lock_bytes)
    candidate_rel = _posix_relative(candidate_path, source_root)
    output_rel = _posix_relative(destination, source_root)
    spdx_rel = f"{output_rel}/{SPDX_NAME}"
    provenance_rel = f"{output_rel}/{PROVENANCE_NAME}"
    spdx_payload = _canonical_json(_spdx_document(identity, lock_packages))
    provenance_payload = _canonical_json(
        _provenance_document(
            identity, candidate_rel, candidate_digest, lock_hash, inputs
        )
    )
    release_document = _release_artifacts(
        identity,
        inputs,
        lock_hash,
        candidate_rel,
        candidate_digest,
        len(candidate_bytes),
        spdx_rel,
        spdx_payload,
        provenance_rel,
        provenance_payload,
    )
    _validate_release_artifacts(release_document)
    release_payload = _canonical_json(release_document)
    summary = _summary(
        identity,
        destination,
        candidate_digest,
        len(candidate_bytes),
        lock_hash,
        spdx_payload,
        provenance_payload,
        release_payload,
    )
    _publish(
        destination,
        {
            SPDX_NAME: spdx_payload,
            PROVENANCE_NAME: provenance_payload,
            RELEASE_ARTIFACTS_NAME: release_payload,
            RESULT_NAME: _canonical_json(summary),
        },
    )
    return summary


def _required_text(value: str) -> str:
    if not value or value.strip() != value or any(ord(character) < 32 for character in value):
        raise argparse.ArgumentTypeError("must be a nonempty single-line string")
    return value


def _sha256_text(value: str) -> str:
    if not SHA256_RE.fullmatch(value):
        raise argparse.ArgumentTypeError("must be 64 lowercase hex characters")
    return value


def _arguments(argv: list[str] | None) -> Inputs:
    parser = argparse.ArgumentParser(
        description=(
            "Generate an unqualified candidate SPDX/provenance bundle bound to "
            "one existing Debian package."
        )
    )
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--source-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--expected-sha256", required=True, type=_sha256_text)
    parser.add_argument("--builder-os", required=True, type=_required_text)
    parser.add_argument("--builder-image", required=True, type=_required_text)
    parser.add_argument("--builder-image-digest", required=True, type=_required_text)
    parser.add_argument("--toolchain", required=True, type=_required_text)
    values = parser.parse_args(argv)
    return Inputs(
        candidate=values.candidate,
        source_root=values.source_root,
        output=values.output,
        expected_sha256=values.expected_sha256,
        builder_os=values.builder_os,
        builder_image=values.builder_image,
        builder_image_digest=values.builder_image_digest,
        toolchain=values.toolchain,
    )


def main(argv: list[str] | None = None) -> int:
    try:
        summary = build(_arguments(argv))
    except (SupplyChainError, OSError, subprocess.SubprocessError) as error:
        print(f"build_candidate_supply_chain: {error}", file=sys.stderr)
        return 1
    sys.stdout.write(_canonical_json(summary).decode("ascii"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
