#!/usr/bin/env python3
"""Build and inspect a retained, installable Narrivy Debian candidate.

This builder reuses the fail-closed staging and archive inspection primitives
from ``build_deb_candidate.py``. Default output is an installable unqualified
candidate. ``--clean-source`` with ``--source-root`` proves a clean Git tree
and embeds a clean-unqualified-release-candidate identity. Neither mode is
release-qualified.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

import build_deb_candidate as safe
import stage_installed_runtime as runtime_staging


PACKAGE_NAME = "narrivy"
# D3: semantic versions in Debian form, `1.0.0~rcN` then `1.0.0`; `~` sorts
# a release candidate before its release.
VERSION_RE = re.compile(r"(0|[1-9][0-9]{0,3})(\.(0|[1-9][0-9]{0,3})){2}(~rc[1-9][0-9]{0,3})?")
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
DEFAULT_CLASSIFICATION = "installable-unqualified-candidate"
CLEAN_CLASSIFICATION = "clean-unqualified-release-candidate"
# The language-model worker (refine and the AI assistant, D48), bound by its
# own fixed-root manifest of the transcribe manifest's shape.
LLM_WORKER_RELATIVE = "usr/lib/narrivy/narrivy-openvino-llm-worker"
LLM_MANIFEST_RELATIVE = "usr/share/narrivy/llm-worker-manifest.json"
LLM_RUNTIME_ID = "openvino-llm"
# The OpenVINO speech worker (the OpenVINO add-on's models, D25), bound the
# same way. It loads the runtime pack the Store installs.
OPENVINO_WORKER_RELATIVE = "usr/lib/narrivy/narrivy-openvino-worker"
OPENVINO_MANIFEST_RELATIVE = "usr/share/narrivy/openvino-worker-manifest.json"
OPENVINO_RUNTIME_ID = "openvino"
# Voice mode's worker (listening and speaking, D34), bound the same way, and
# the Silero voice detector it reads (MIT), checked against its digest.
VOICE_WORKER_RELATIVE = "usr/lib/narrivy/narrivy-voice-worker"
VOICE_MANIFEST_RELATIVE = "usr/share/narrivy/voice-worker-manifest.json"
VOICE_RUNTIME_ID = "voice"
VOICE_DETECTOR_RELATIVE = "usr/share/narrivy/voice/silero_vad.onnx"
VOICE_DETECTOR_SHA256 = "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6"
# Voice mode's Urdu speaker (GPL-3.0-or-later, D51), a separate program the
# voice worker starts, bound the same way; its licence goes beside it.
URDU_SPEAKER_RELATIVE = "usr/lib/narrivy/narrivy-urdu-speaker"
URDU_MANIFEST_RELATIVE = "usr/share/narrivy/urdu-speaker-manifest.json"
URDU_RUNTIME_ID = "urdu-speaker"
URDU_LICENSE_RELATIVE = "usr/share/doc/narrivy/narrivy-urdu-speaker/COPYING"
URDU_SOURCE_RELATIVE = "usr/share/doc/narrivy/narrivy-urdu-speaker/SOURCE"
# The writing assistant's native-messaging host and the unpacked extension
# the user loads in Chrome or Brave (D26, D48). The app registers the host
# per user; the package writes nothing under /etc.
BROWSER_HOST_RELATIVE = "usr/lib/narrivy/narrivy-browser-host"
BROWSER_EXTENSION_RELATIVE = "usr/share/narrivy/browser-extension"
BROWSER_EXTENSION_FILES = (
    "manifest.json",
    "background.js",
    "content.js",
    "settings.js",
    "popup.html",
    "popup.js",
    "options.html",
    "options.js",
    "ui.css",
    "highlights.css",
    "icons/icon-16.png",
    "icons/icon-32.png",
    "icons/icon-48.png",
    "icons/icon-128.png",
)
DEFAULT_BUILD_INFO_RELATIVE = "usr/share/narrivy/candidate-build-info.json"
CLEAN_BUILD_INFO_RELATIVE = "usr/share/narrivy/build-info.json"


class InstallableError(RuntimeError):
    """Content-free installable candidate construction failure."""


@dataclass(frozen=True)
class Inputs:
    narrivyd: Path
    cli: Path
    desktop: Path
    worker: Path
    llm_worker: Path
    openvino_worker: Path
    voice_worker: Path
    voice_detector: Path
    urdu_speaker: Path
    urdu_license: Path
    browser_host: Path
    browser_extension: Path
    service: Path
    desktop_entry: Path
    postinst: Path
    prerm: Path
    postrm: Path
    icon_32: Path
    icon_128: Path
    icon_256: Path
    icon_512: Path
    license_file: Path
    notices: Path
    output: Path
    version: str
    source_head: str
    source_date_epoch: int
    source_root: Path | None = None


@dataclass(frozen=True)
class PackageIdentity:
    classification: str
    git_dirty: bool
    schema_version: int
    build_info_relative: str


def _control(version: str) -> bytes:
    return (
        f"Package: {PACKAGE_NAME}\n"
        f"Version: {version}\n"
        "Section: utils\n"
        "Priority: optional\n"
        "Architecture: amd64\n"
        "Maintainer: Waqar Gulzar <iamwaqargulzar@gmail.com>\n"
        "Depends: libwebkit2gtk-4.1-0 (>= 2.44), libgtk-3-0t64 (>= 3.24) | libgtk-3-0 (>= 3.24), "
        "libgtk-layer-shell0 (>= 0.8), "
        "libopenblas0, libvulkan1, curl, pipewire, systemd, xdg-utils\n"
        "Recommends: pipewire-pulse, mesa-vulkan-drivers, wtype, libglib2.0-bin\n"
        "Homepage: https://github.com/iamwaqargulzar/narrivy\n"
        "Description: Narrivy local-first voice dictation for Linux\n"
        " Private dictation with local audio capture and offline speech recognition.\n"
    ).encode()


def _require_resident_daemon(narrivyd: Path, timeout: float = 5) -> None:
    """F-M14: the packaged unit runs the installed resident engine, so a
    daemon built without `asr-transcribe-resident` must never be packaged.
    The binary reports its own features; nothing else runs."""
    try:
        result = subprocess.run(
            [os.fspath(narrivyd), "--build-features"],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            env={},
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise InstallableError("narrivyd could not report its build features") from error
    features = result.stdout.decode("utf-8", "replace").split()
    if result.returncode != 0 or "asr-transcribe-resident" not in features:
        raise InstallableError("narrivyd was not built with asr-transcribe-resident")


def _validate_script(payload: bytes) -> None:
    if not payload.startswith(b"#!/bin/sh\n") or b"\x00" in payload:
        raise InstallableError("maintainer script is invalid")
    result = subprocess.run(
        ["/bin/sh", "-n"],
        input=payload,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=5,
        check=False,
    )
    if result.returncode != 0:
        raise InstallableError("maintainer script syntax is invalid")


def _inspect(
    archive_descriptor: int,
    expected_files: dict[str, safe.ExpectedFile],
    control_files: dict[str, tuple[int, bytes]],
    epoch: int,
) -> None:
    descriptor_path = f"/proc/self/fd/{archive_descriptor}"
    directories = {"."}
    for name in expected_files:
        directories.update(
            str(parent) for parent in PurePosixPath(name).parents if str(parent) != "."
        )
    entries = safe._tar_members(
        [str(safe.DPKG_DEB), "--fsys-tarfile", descriptor_path],
        archive_descriptor,
        directories,
        {name: value.size_bytes for name, value in expected_files.items()},
        set(),
    )
    if set(entries) != directories | set(expected_files):
        raise InstallableError("installable candidate data membership is not exact")
    for name in directories:
        entry = entries[name]
        expected_mode = 0o755
        if (entry.kind, entry.mode, entry.uid, entry.gid, entry.mtime) != (
            "directory",
            expected_mode,
            0,
            0,
            epoch,
        ):
            raise InstallableError("installable candidate directory identity is wrong")
    for name, expected in expected_files.items():
        entry = entries[name]
        if (
            entry.kind != "file"
            or entry.mode != expected.mode
            or entry.uid != 0
            or entry.gid != 0
            or entry.mtime != epoch
            or entry.size_bytes != expected.size_bytes
            or entry.sha256 != expected.sha256
        ):
            raise InstallableError("installable candidate file identity is wrong")

    controls = safe._tar_members(
        [str(safe.DPKG_DEB), "--ctrl-tarfile", descriptor_path],
        archive_descriptor,
        {"."},
        {name: len(payload) for name, (_, payload) in control_files.items()},
        set(control_files),
    )
    if set(controls) != {"."} | set(control_files):
        raise InstallableError("installable candidate control membership is not exact")
    root = controls["."]
    if (root.kind, root.mode, root.uid, root.gid, root.mtime) != (
        "directory",
        0o755,
        0,
        0,
        epoch,
    ):
        raise InstallableError("installable candidate control root is wrong")
    for name, (mode, payload) in control_files.items():
        entry = controls[name]
        if (
            entry.kind != "file"
            or entry.mode != mode
            or entry.uid != 0
            or entry.gid != 0
            or entry.mtime != epoch
            or entry.payload != payload
        ):
            raise InstallableError("installable candidate control identity is wrong")


def _publish(snapshot: int, destination: Path) -> None:
    if not destination.is_absolute() or destination.name in ("", ".", ".."):
        raise InstallableError("output path must be an absolute file path")
    destination.parent.mkdir(parents=True, exist_ok=True, mode=0o755)
    resolved_parent = destination.parent.resolve(strict=True)
    if resolved_parent != destination.parent:
        raise InstallableError("output ancestry may not contain symlinks")
    parent_metadata = resolved_parent.stat()
    if not stat.S_ISDIR(parent_metadata.st_mode) or parent_metadata.st_uid != os.geteuid():
        raise InstallableError("output directory ownership is unsafe")
    output = -1
    try:
        output = os.open(
            destination,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
            0o644,
        )
        os.lseek(snapshot, 0, os.SEEK_SET)
        while True:
            chunk = os.read(snapshot, 64 * 1024)
            if not chunk:
                break
            view = memoryview(chunk)
            while view:
                written = os.write(output, view)
                if written <= 0:
                    raise InstallableError("candidate publication write failed")
                view = view[written:]
        os.fchmod(output, 0o644)
        os.fsync(output)
    except Exception:
        if output >= 0:
            destination.unlink(missing_ok=True)
        raise
    finally:
        if output >= 0:
            os.close(output)


def _package_identity(source_root: Path | None) -> PackageIdentity:
    if source_root is None:
        return PackageIdentity(
            classification=DEFAULT_CLASSIFICATION,
            git_dirty=True,
            schema_version=1,
            build_info_relative=DEFAULT_BUILD_INFO_RELATIVE,
        )
    return PackageIdentity(
        classification=CLEAN_CLASSIFICATION,
        git_dirty=False,
        schema_version=2,
        build_info_relative=CLEAN_BUILD_INFO_RELATIVE,
    )


def _build_info_payload(
    identity: PackageIdentity,
    *,
    version: str,
    source_head: str,
    source_date_epoch: int,
    binaries: dict[str, object],
    worker: object,
    llm_worker: object,
    openvino_worker: object,
    voice_worker: object,
    urdu_speaker: object,
    browser_host: object,
) -> bytes:
    return safe._canonical_json(
        {
            "classification": identity.classification,
            "git_dirty": identity.git_dirty,
            "inputs": {
                **binaries,
                "resident-worker": worker,
                "llm-worker": llm_worker,
                "openvino-worker": openvino_worker,
                "voice-worker": voice_worker,
                "urdu-speaker": urdu_speaker,
                "browser-host": browser_host,
            },
            "package": PACKAGE_NAME,
            "schema_version": identity.schema_version,
            "source_date_epoch": source_date_epoch,
            "source_head": source_head,
            "version": version,
        }
    )


def _run_git(
    source_root: Path,
    arguments: tuple[str, ...],
    *,
    git: str = GIT,
    timeout: float = GIT_TIMEOUT_SECONDS,
) -> str:
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
        raise InstallableError("source git command timed out") from error
    except OSError as error:
        raise InstallableError("source git command failed") from error
    if result.returncode != 0:
        raise InstallableError("source git command failed")
    try:
        return result.stdout.decode("utf-8")
    except UnicodeDecodeError as error:
        raise InstallableError("source git command failed") from error


def _require_clean_source(
    source_root: Path,
    source_head: str,
    *,
    git: str = GIT,
    timeout: float = GIT_TIMEOUT_SECONDS,
) -> None:
    if not source_root.is_absolute():
        raise InstallableError("source root must be an existing absolute directory")
    try:
        resolved_root = source_root.resolve(strict=True)
    except OSError as error:
        raise InstallableError("source root must be an existing absolute directory") from error
    if not resolved_root.is_dir():
        raise InstallableError("source root must be an existing absolute directory")
    toplevel = _run_git(
        source_root,
        ("rev-parse", "--show-toplevel"),
        git=git,
        timeout=timeout,
    ).rstrip("\n")
    try:
        resolved_toplevel = Path(toplevel).resolve(strict=True)
    except OSError as error:
        raise InstallableError("source root is not the git toplevel") from error
    if resolved_toplevel != resolved_root:
        raise InstallableError("source root is not the git toplevel")
    observed_head = _run_git(
        source_root,
        ("rev-parse", "--verify", "HEAD"),
        git=git,
        timeout=timeout,
    ).rstrip("\n")
    if not re.fullmatch(r"[0-9a-f]{40}", observed_head) or observed_head != source_head:
        raise InstallableError("source HEAD does not match the repository")
    status = _run_git(
        source_root,
        ("status", "--porcelain=v1", "--untracked-files=all"),
        git=git,
        timeout=timeout,
    )
    if status != "":
        raise InstallableError("source tree is not clean")


def build(inputs: Inputs) -> dict[str, object]:
    if not VERSION_RE.fullmatch(inputs.version):
        raise InstallableError("package version is invalid")
    if not re.fullmatch(r"[0-9a-f]{40}", inputs.source_head):
        raise InstallableError("source HEAD must be lowercase 40-character hex")
    if inputs.source_date_epoch <= 0:
        raise InstallableError("SOURCE_DATE_EPOCH must be positive")
    if inputs.source_root is not None:
        _require_clean_source(inputs.source_root, inputs.source_head)
    identity = _package_identity(inputs.source_root)
    safe._validate_tool(safe.DPKG_DEB)

    service = safe._read_asset(inputs.service)
    desktop_entry = safe._read_asset(inputs.desktop_entry)
    safe._validate_service(service)
    safe._validate_desktop_entry(desktop_entry)
    maintainer = {
        "postinst": (0o755, safe._read_asset(inputs.postinst)),
        "prerm": (0o755, safe._read_asset(inputs.prerm)),
        "postrm": (0o755, safe._read_asset(inputs.postrm)),
    }
    for _, payload in maintainer.values():
        _validate_script(payload)
    license_payload = safe._read_asset(inputs.license_file)
    notices = safe._read_asset(inputs.notices)
    safe._validate_legal_text(license_payload, notices)
    icons: dict[str, bytes] = {}
    for dimension, path in (
        (32, inputs.icon_32),
        (128, inputs.icon_128),
        (256, inputs.icon_256),
        (512, inputs.icon_512),
    ):
        payload = safe._read_asset(path)
        safe._validate_png(payload, dimension)
        icons[f"usr/share/icons/hicolor/{dimension}x{dimension}/apps/narrivy.png"] = payload
    if not inputs.browser_extension.is_absolute():
        raise InstallableError("browser extension input must be absolute")
    extension = {
        f"{BROWSER_EXTENSION_RELATIVE}/{name}": safe._read_asset(inputs.browser_extension / name)
        for name in BROWSER_EXTENSION_FILES
    }
    try:
        extension_manifest = json.loads(extension[f"{BROWSER_EXTENSION_RELATIVE}/manifest.json"])
    except ValueError as error:
        raise InstallableError("browser extension manifest is invalid") from error
    if extension_manifest.get("manifest_version") != 3 or not extension_manifest.get("key"):
        raise InstallableError("browser extension manifest is invalid")

    with tempfile.TemporaryDirectory(prefix="narrivy-installable-") as temporary:
        root = Path(temporary) / "root"
        runtime_manifest = runtime_staging.stage_installed_runtime(root, inputs.worker)
        root_observed = root.lstat()
        root_descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        archive_snapshot = -1
        try:
            if safe._metadata_identity(root_observed) != safe._metadata_identity(
                os.fstat(root_descriptor)
            ):
                raise InstallableError("staging root identity changed")
            _require_resident_daemon(inputs.narrivyd)
            binaries = {
                "narrivyd": safe._copy_release_elf(
                    inputs.narrivyd, root_descriptor, PurePosixPath("usr/bin/narrivyd")
                ),
                "narrivy-cli": safe._copy_release_elf(
                    inputs.cli, root_descriptor, PurePosixPath("usr/bin/narrivy-cli")
                ),
                "narrivy-desktop": safe._copy_release_elf(
                    inputs.desktop, root_descriptor, PurePosixPath("usr/bin/narrivy-desktop")
                ),
            }
            llm_worker = safe._copy_release_elf(
                inputs.llm_worker, root_descriptor, PurePosixPath(LLM_WORKER_RELATIVE)
            )
            openvino_worker = safe._copy_release_elf(
                inputs.openvino_worker, root_descriptor, PurePosixPath(OPENVINO_WORKER_RELATIVE)
            )
            voice_worker = safe._copy_release_elf(
                inputs.voice_worker, root_descriptor, PurePosixPath(VOICE_WORKER_RELATIVE)
            )
            urdu_speaker = safe._copy_release_elf(
                inputs.urdu_speaker, root_descriptor, PurePosixPath(URDU_SPEAKER_RELATIVE)
            )
            browser_host = safe._copy_release_elf(
                inputs.browser_host, root_descriptor, PurePosixPath(BROWSER_HOST_RELATIVE)
            )
            llm_manifest = safe._canonical_json(
                {"runtime_id": LLM_RUNTIME_ID, "schema_version": 1, "worker": llm_worker}
            )
            openvino_manifest = safe._canonical_json(
                {"runtime_id": OPENVINO_RUNTIME_ID, "schema_version": 1, "worker": openvino_worker}
            )
            voice_manifest = safe._canonical_json(
                {"runtime_id": VOICE_RUNTIME_ID, "schema_version": 1, "worker": voice_worker}
            )
            urdu_manifest = safe._canonical_json(
                {"runtime_id": URDU_RUNTIME_ID, "schema_version": 1, "worker": urdu_speaker}
            )
            urdu_license = safe._read_asset(inputs.urdu_license, maximum=64 * 1024)
            # The source note sits beside the licence in the repository.
            urdu_source = safe._read_asset(inputs.urdu_license.with_name("SOURCE"), maximum=4096)
            if b"GNU GENERAL PUBLIC LICENSE" not in urdu_license or b"Version 3" not in urdu_license:
                raise InstallableError("the Urdu speaker's licence is not the GPL version 3")
            voice_detector = safe._read_asset(inputs.voice_detector, maximum=1024 * 1024)
            if hashlib.sha256(voice_detector).hexdigest() != VOICE_DETECTOR_SHA256:
                raise InstallableError("voice detector does not match its digest")
            manifest_payload = safe._canonical_json(runtime_manifest)
            build_info = _build_info_payload(
                identity,
                version=inputs.version,
                source_head=inputs.source_head,
                source_date_epoch=inputs.source_date_epoch,
                binaries=binaries,
                worker=runtime_manifest["worker"],
                llm_worker=llm_worker,
                openvino_worker=openvino_worker,
                voice_worker=voice_worker,
                urdu_speaker=urdu_speaker,
                browser_host=browser_host,
            )
            payloads = {
                "usr/lib/systemd/user/narrivy-daemon.service": (0o644, service),
                "usr/share/applications/narrivy.desktop": (0o644, desktop_entry),
                identity.build_info_relative: (0o644, build_info),
                LLM_MANIFEST_RELATIVE: (0o644, llm_manifest),
                OPENVINO_MANIFEST_RELATIVE: (0o644, openvino_manifest),
                VOICE_MANIFEST_RELATIVE: (0o644, voice_manifest),
                VOICE_DETECTOR_RELATIVE: (0o644, voice_detector),
                URDU_MANIFEST_RELATIVE: (0o644, urdu_manifest),
                URDU_LICENSE_RELATIVE: (0o644, urdu_license),
                URDU_SOURCE_RELATIVE: (0o644, urdu_source),
                **{name: (0o644, payload) for name, payload in extension.items()},
                "usr/share/doc/narrivy/copyright": (0o644, license_payload),
                "usr/share/doc/narrivy/THIRD_PARTY_NOTICES.md": (0o644, notices),
                **{name: (0o644, payload) for name, payload in icons.items()},
            }
            for name, (mode, payload) in payloads.items():
                safe._write_file_at(root_descriptor, PurePosixPath(name), payload, mode)
            expected = {
                "usr/bin/narrivyd": safe.ExpectedFile(0o755, **binaries["narrivyd"]),
                "usr/bin/narrivy-cli": safe.ExpectedFile(0o755, **binaries["narrivy-cli"]),
                "usr/bin/narrivy-desktop": safe.ExpectedFile(0o755, **binaries["narrivy-desktop"]),
                runtime_staging.WORKER_RELATIVE.as_posix(): safe.ExpectedFile(
                    0o755, **runtime_manifest["worker"]
                ),
                LLM_WORKER_RELATIVE: safe.ExpectedFile(0o755, **llm_worker),
                OPENVINO_WORKER_RELATIVE: safe.ExpectedFile(0o755, **openvino_worker),
                VOICE_WORKER_RELATIVE: safe.ExpectedFile(0o755, **voice_worker),
                URDU_SPEAKER_RELATIVE: safe.ExpectedFile(0o755, **urdu_speaker),
                BROWSER_HOST_RELATIVE: safe.ExpectedFile(0o755, **browser_host),
                runtime_staging.MANIFEST_RELATIVE.as_posix(): safe._expected_file(
                    0o644, manifest_payload
                ),
                **{
                    name: safe._expected_file(mode, payload)
                    for name, (mode, payload) in payloads.items()
                },
            }
            control_files = {"control": (0o644, _control(inputs.version)), **maintainer}
            for name, (mode, payload) in control_files.items():
                safe._write_file_at(
                    root_descriptor, PurePosixPath(f"DEBIAN/{name}"), payload, mode
                )
            normalized = {
                **expected,
                **{
                    f"DEBIAN/{name}": safe._expected_file(mode, payload)
                    for name, (mode, payload) in control_files.items()
                },
            }
            safe._normalize_anchored_tree(root_descriptor, normalized, inputs.source_date_epoch)
            # Debian's data archive root maps to the installed filesystem root;
            # it must never narrow `/` permissions during installation.
            os.fchmod(root_descriptor, 0o755)
            os.utime(root_descriptor, (inputs.source_date_epoch, inputs.source_date_epoch))
            archive = Path(temporary) / "candidate.deb"
            archive_descriptor = os.open(
                archive,
                os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
                0o600,
            )
            try:
                result = subprocess.run(
                    [
                        str(safe.DPKG_DEB),
                        "-Zxz",
                        "-z9",
                        "--root-owner-group",
                        "--build",
                        str(root),
                        f"/proc/self/fd/{archive_descriptor}",
                    ],
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    env={
                        "PATH": "/usr/bin:/bin",
                        "LC_ALL": "C",
                        "TZ": "UTC",
                        "SOURCE_DATE_EPOCH": str(inputs.source_date_epoch),
                    },
                    pass_fds=(archive_descriptor,),
                    timeout=120,
                    check=False,
                )
                if result.returncode != 0:
                    raise InstallableError("dpkg-deb construction failed")
                archive_snapshot, digest, size = safe._snapshot_archive_descriptor(
                    archive_descriptor
                )
            finally:
                os.close(archive_descriptor)
            _inspect(
                archive_snapshot,
                expected,
                control_files,
                inputs.source_date_epoch,
            )
            _publish(archive_snapshot, inputs.output)
            return {
                "archive_sha256": digest,
                "archive_size_bytes": size,
                "classification": identity.classification,
                "data_file_count": len(expected),
                "git_dirty": identity.git_dirty,
                "install_blocked": False,
                "output": str(inputs.output),
                "package": PACKAGE_NAME,
                "version": inputs.version,
            }
        finally:
            if archive_snapshot >= 0:
                os.close(archive_snapshot)
            os.close(root_descriptor)


def _arguments(argv: list[str] | None) -> Inputs:
    parser = argparse.ArgumentParser()
    for name in (
        "narrivyd",
        "cli",
        "desktop",
        "worker",
        "llm-worker",
        "openvino-worker",
        "voice-worker",
        "voice-detector",
        "urdu-speaker",
        "urdu-license",
        "browser-host",
        "browser-extension",
        "service",
        "desktop-entry",
        "postinst",
        "prerm",
        "postrm",
        "icon-32",
        "icon-128",
        "icon-256",
        "icon-512",
        "license-file",
        "notices",
        "output",
    ):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-head", required=True)
    parser.add_argument("--source-date-epoch", required=True, type=int)
    parser.add_argument(
        "--clean-source",
        action="store_true",
        help="prove a clean Git tree and embed a clean-unqualified-release-candidate",
    )
    parser.add_argument(
        "--source-root",
        type=Path,
        help="absolute Git toplevel required by --clean-source",
    )
    values = parser.parse_args(argv)
    if bool(values.clean_source) != (values.source_root is not None):
        parser.error("--clean-source and --source-root must be used together")
    return Inputs(
        narrivyd=values.narrivyd,
        cli=values.cli,
        desktop=values.desktop,
        worker=values.worker,
        llm_worker=values.llm_worker,
        openvino_worker=values.openvino_worker,
        voice_worker=values.voice_worker,
        voice_detector=values.voice_detector,
        urdu_speaker=values.urdu_speaker,
        urdu_license=values.urdu_license,
        browser_host=values.browser_host,
        browser_extension=values.browser_extension,
        service=values.service,
        desktop_entry=values.desktop_entry,
        postinst=values.postinst,
        prerm=values.prerm,
        postrm=values.postrm,
        icon_32=values.icon_32,
        icon_128=values.icon_128,
        icon_256=values.icon_256,
        icon_512=values.icon_512,
        license_file=values.license_file,
        notices=values.notices,
        output=values.output,
        version=values.version,
        source_head=values.source_head,
        source_date_epoch=values.source_date_epoch,
        source_root=values.source_root,
    )


def main(argv: list[str] | None = None) -> int:
    try:
        summary = build(_arguments(argv))
    except (InstallableError, safe.CandidateError, OSError, subprocess.SubprocessError) as error:
        print(f"build_deb_installable: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
