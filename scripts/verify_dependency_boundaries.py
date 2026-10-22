#!/usr/bin/env python3
"""Fail-closed direct dependency validation for portable Narrivy crates."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, Iterable, Mapping

try:
    from boundary_policy import (
        FORBIDDEN_WORKSPACE_LAYERS,
        LIBRARY_LEAF_ALLOWLIST,
        PROTECTED_PACKAGES,
        forbidden_implementation_family,
    )
except ModuleNotFoundError:  # Imported by the fixture suite.
    from scripts.boundary_policy import (
        FORBIDDEN_WORKSPACE_LAYERS,
        LIBRARY_LEAF_ALLOWLIST,
        PROTECTED_PACKAGES,
        forbidden_implementation_family,
    )


ROOT = Path(__file__).resolve().parents[1]
METADATA_COMMAND: tuple[str, ...] = (
    "cargo",
    "metadata",
    "--format-version",
    "1",
    "--locked",
    "--no-deps",
)


class MetadataLoadError(RuntimeError):
    """Cargo metadata could not be obtained without weakening the gate."""


def load_cargo_metadata(root: Path) -> dict[str, Any]:
    """Load declared workspace dependencies without resolving transitives."""

    if not shutil.which("cargo"):
        raise MetadataLoadError(
            "cargo is required; run doctor/bootstrap/toolchain setup"
        )
    result = subprocess.run(
        list(METADATA_COMMAND),
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip() or "no Cargo diagnostic"
        raise MetadataLoadError(
            f"cargo metadata exited {result.returncode}: {detail}"
        )
    try:
        metadata = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise MetadataLoadError(f"cargo metadata emitted invalid JSON: {error}") from error
    if not isinstance(metadata, dict):
        raise MetadataLoadError("cargo metadata root is not an object")
    return metadata


def _dependency_kind(raw_kind: object) -> str | None:
    if raw_kind is None or raw_kind == "normal":
        return "normal"
    if raw_kind in {"build", "dev"}:
        return str(raw_kind)
    return None


def _workspace_layer(root: Path, dependency_path: str) -> tuple[str | None, str | None]:
    try:
        path = Path(dependency_path).resolve()
        relative = path.relative_to(root.resolve())
    except (OSError, ValueError) as error:
        return None, f"local dependency path {dependency_path!r} is outside the workspace: {error}"
    layer = relative.parts[0] if relative.parts else None
    return layer, None


def validate_dependency_metadata(
    metadata: dict[str, Any],
    root: Path,
    protected_packages: Iterable[str] = PROTECTED_PACKAGES,
) -> list[str]:
    """Validate direct production dependencies from Cargo metadata.

    Normal and build dependencies are checked for every declared target
    predicate. Dev-only dependencies do not establish a production boundary.
    The actual Cargo package identity (``name``) is checked, never its local
    import alias (``rename``).
    """

    errors: list[str] = []
    packages = metadata.get("packages")
    workspace_members = metadata.get("workspace_members")
    if not isinstance(packages, list):
        return ["cargo metadata field 'packages' is missing or not a list"]
    if not isinstance(workspace_members, list) or not all(
        isinstance(member, str) for member in workspace_members
    ):
        return ["cargo metadata field 'workspace_members' is missing or malformed"]
    member_ids = set(workspace_members)

    protected = tuple(protected_packages)
    by_name: dict[str, list[dict[str, Any]]] = {name: [] for name in protected}
    for package in packages:
        if not isinstance(package, dict):
            errors.append("cargo metadata contains a non-object package entry")
            continue
        name = package.get("name")
        package_id = package.get("id")
        if name in by_name and isinstance(package_id, str) and package_id in member_ids:
            by_name[name].append(package)

    for package_name in protected:
        candidates = by_name[package_name]
        if len(candidates) != 1:
            errors.append(
                f"protected package {package_name} has {len(candidates)} workspace metadata entries"
            )
            continue
        package = candidates[0]
        dependencies = package.get("dependencies")
        if not isinstance(dependencies, list):
            errors.append(f"{package_name} dependencies metadata is missing or malformed")
            continue

        for dependency in dependencies:
            if not isinstance(dependency, dict):
                errors.append(f"{package_name} contains a non-object dependency entry")
                continue
            actual_name = dependency.get("name")
            if not isinstance(actual_name, str) or not actual_name.strip():
                errors.append(f"{package_name} dependency has no valid package identity")
                continue
            rename = dependency.get("rename")
            if rename is not None and not isinstance(rename, str):
                errors.append(
                    f"{package_name} dependency {actual_name} has malformed rename metadata"
                )
                continue
            target = dependency.get("target")
            if target is not None and not isinstance(target, str):
                errors.append(
                    f"{package_name} dependency {actual_name} has malformed target metadata"
                )
                continue
            kind = _dependency_kind(dependency.get("kind"))
            if kind is None:
                errors.append(
                    f"{package_name} dependency {actual_name} has unknown kind "
                    f"{dependency.get('kind')!r}"
                )
                continue
            if kind == "dev":
                continue

            target_detail = f" for {target}" if target else ""
            alias_detail = f" (renamed {rename})" if rename else ""
            family = forbidden_implementation_family(actual_name)
            if family is not None:
                errors.append(
                    f"{package_name} directly {kind}-depends on forbidden implementation "
                    f"package {actual_name}{alias_detail}{target_detail} (family {family})"
                )

            dependency_path = dependency.get("path")
            if dependency_path is not None:
                if not isinstance(dependency_path, str) or not dependency_path:
                    errors.append(
                        f"{package_name} dependency {actual_name} has malformed path metadata"
                    )
                    continue
                layer, path_error = _workspace_layer(root, dependency_path)
                if path_error:
                    errors.append(f"{package_name} {kind} dependency {actual_name}: {path_error}")
                elif layer in FORBIDDEN_WORKSPACE_LAYERS:
                    errors.append(
                        f"{package_name} directly {kind}-depends on forbidden workspace "
                        f"layer {layer}: {actual_name}{target_detail}"
                    )
    return sorted(set(errors))


def _declares_binary(package: Mapping[str, Any], name: str, errors: list[str]) -> bool:
    """Report whether a package exposes a binary target.

    A ``bin`` target is a release-graph entry point: packaging reaches it
    without any Cargo dependency edge, so it can never be an orphan.
    """

    targets = package.get("targets")
    if not isinstance(targets, list):
        errors.append(f"{name} targets metadata is missing or malformed")
        return True
    for target in targets:
        if not isinstance(target, dict):
            errors.append(f"{name} contains a non-object target entry")
            return True
        kinds = target.get("kind")
        if not isinstance(kinds, list) or not all(isinstance(kind, str) for kind in kinds):
            errors.append(f"{name} target metadata has a malformed kind list")
            return True
        if "bin" in kinds:
            return True
    return False


def validate_workspace_composition(
    metadata: dict[str, Any],
    root: Path,
    library_leaf_allowlist: Mapping[str, str] = LIBRARY_LEAF_ALLOWLIST,
) -> list[str]:
    """Fail on workspace members that no production code path can reach.

    A member with no production reverse dependency is either dead code or a
    missing composition, and both are release-relevant: an orphan crate can
    ship with its own passing ``cargo test`` while nothing in the daemon,
    CLI, or desktop app ever calls it, which lets a requirement cite that test
    as proof. Dev-dependency edges do not count as production composition,
    matching the rest of this validator.
    """

    errors: list[str] = []
    packages = metadata.get("packages")
    workspace_members = metadata.get("workspace_members")
    if not isinstance(packages, list):
        return ["cargo metadata field 'packages' is missing or not a list"]
    if not isinstance(workspace_members, list) or not all(
        isinstance(member, str) for member in workspace_members
    ):
        return ["cargo metadata field 'workspace_members' is missing or malformed"]
    member_ids = set(workspace_members)

    members: dict[str, dict[str, Any]] = {}
    member_dirs: dict[str, Path] = {}
    for package in packages:
        if not isinstance(package, dict):
            errors.append("cargo metadata contains a non-object package entry")
            continue
        name = package.get("name")
        package_id = package.get("id")
        manifest_path = package.get("manifest_path")
        if not isinstance(name, str) or not isinstance(package_id, str):
            errors.append("cargo metadata package entry lacks a name/id identity")
            continue
        if package_id not in member_ids:
            continue
        if name in members:
            errors.append(f"workspace member {name} appears more than once in cargo metadata")
            continue
        members[name] = package
        if isinstance(manifest_path, str) and manifest_path:
            try:
                member_dirs[name] = Path(manifest_path).resolve().parent
            except OSError as error:
                errors.append(f"{name} manifest path cannot be resolved: {error}")

    if not members:
        return sorted(set([*errors, "cargo metadata reports no workspace member packages"]))

    produced_by: dict[str, set[str]] = {name: set() for name in members}
    for name in sorted(members):
        dependencies = members[name].get("dependencies")
        if not isinstance(dependencies, list):
            errors.append(f"{name} dependencies metadata is missing or malformed")
            continue
        for dependency in dependencies:
            if not isinstance(dependency, dict):
                errors.append(f"{name} contains a non-object dependency entry")
                continue
            # Kind is validated before the path check so malformed kind metadata
            # on any dependency fails closed, not just the path-based ones.
            kind = _dependency_kind(dependency.get("kind"))
            if kind is None:
                errors.append(
                    f"{name} dependency {dependency.get('name')!r} has unknown kind "
                    f"{dependency.get('kind')!r}"
                )
                continue
            if kind == "dev":
                continue
            dependency_path = dependency.get("path")
            if dependency_path is None:
                continue
            if not isinstance(dependency_path, str) or not dependency_path:
                errors.append(f"{name} has a dependency with malformed path metadata")
                continue
            try:
                resolved = Path(dependency_path).resolve()
            except OSError as error:
                errors.append(f"{name} local dependency path {dependency_path!r} is unusable: {error}")
                continue
            for target, target_dir in member_dirs.items():
                if target != name and resolved == target_dir:
                    produced_by[target].add(name)

    for name in sorted(members):
        if produced_by[name]:
            continue
        if _declares_binary(members[name], name, errors):
            continue
        if name not in library_leaf_allowlist:
            errors.append(
                f"workspace member {name} is unreachable: no production reverse dependency, "
                f"no bin target, and no library-leaf allowlist entry"
            )
            continue
        if not library_leaf_allowlist[name].strip():
            errors.append(
                f"library-leaf allowlist entry {name} has no written justification"
            )

    for name in sorted(library_leaf_allowlist):
        if name not in members:
            errors.append(
                f"library-leaf allowlist entry {name} is not a workspace member; remove the stale entry"
            )
        elif produced_by[name] and not _declares_binary(members[name], name, errors):
            errors.append(
                f"library-leaf allowlist entry {name} now has production consumers; remove the stale entry"
            )
    return sorted(set(errors))


def main() -> int:
    try:
        metadata = load_cargo_metadata(ROOT)
    except MetadataLoadError as error:
        print(f"ERROR dependency-boundary metadata unavailable: {error}", file=sys.stderr)
        return 2
    errors = validate_dependency_metadata(metadata, ROOT)
    errors.extend(validate_workspace_composition(metadata, ROOT))
    if errors:
        print("Dependency boundary validation FAILED")
        for error in sorted(set(errors)):
            print(f" - {error}")
        return 1
    print(
        f"Dependency boundaries OK for {len(PROTECTED_PACKAGES)} portable/core packages; "
        "every workspace member is a bin target, composed, or allowlisted."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
