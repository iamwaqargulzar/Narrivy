#!/usr/bin/env python3
"""Deterministic positive and negative fixtures for architecture validators."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Iterable, Mapping
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts import boundary_policy, verify_dependency_boundaries as dependency_validator


def _load_source_validator():
    spec = importlib.util.spec_from_file_location(
        "narrivy_verify_boundaries", SCRIPTS / "verify-boundaries.py"
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load source boundary validator")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


source_validator = _load_source_validator()


class SourceBoundaryTests(unittest.TestCase):
    package = "narrivy-core"

    def make_package(self, root: Path, source: str = "pub fn portable() {}\n") -> Path:
        base = root / "crates" / self.package
        (base / "src").mkdir(parents=True)
        (base / "Cargo.toml").write_text(
            '[package]\nname = "narrivy-core"\nversion = "0.0.0"\n',
            encoding="utf-8",
        )
        (base / "src" / "lib.rs").write_text(source, encoding="utf-8")
        return base

    def validate(self, root: Path) -> list[str]:
        return source_validator.validate_source_boundaries(root, (self.package,))

    def test_portable_source_and_non_code_platform_words_pass(self) -> None:
        source = r'''
//! Wayland, PipeWire, std::os::unix, and extern "C" are documentation only.
/* nested comment: tauri::Builder /* libc::flock */ */
const NORMAL: &str = "pipewire::MainLoop and UnixStream";
const RAW: &str = r###"std::os::unix and windows_sys::Win32"###;
const BYTE: &[u8] = b"libc::geteuid";
pub fn portable<'a>(value: &'a str) -> &'a str { value }
'''
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_package(root, source)
            self.assertEqual(self.validate(root), [])

    def test_std_os_and_unix_types_fail(self) -> None:
        source = "use std::os::unix::net::UnixStream;\npub fn use_it(_: UnixStream) {}\n"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_package(root, source)
            errors = self.validate(root)
        self.assertTrue(any("standard-library OS module" in error for error in errors))
        self.assertTrue(any("Unix socket type" in error for error in errors))

    def test_fd_filesystem_extensions_fail(self) -> None:
        source = "use portable_alias::{AsRawFd, PermissionsExt};\n"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_package(root, source)
            errors = self.validate(root)
        self.assertTrue(
            any("platform file-descriptor/filesystem extension" in error for error in errors)
        )

    def test_libc_extern_abi_and_platform_family_fail(self) -> None:
        source = '''
unsafe extern "C" { fn native(); }
pub fn call() {
    let _ = libc::geteuid();
    tauri::Builder::default();
    windows::Win32::Foundation::CloseHandle();
}
'''
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_package(root, source)
            errors = self.validate(root)
        self.assertTrue(any("native extern ABI" in error for error in errors))
        self.assertTrue(any("libc FFI" in error for error in errors))
        self.assertTrue(any("platform/framework/runtime" in error for error in errors))

    def test_build_script_is_production_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base = self.make_package(root)
            (base / "build.rs").write_text(
                "use std::os::unix::ffi::OsStrExt;\n", encoding="utf-8"
            )
            errors = self.validate(root)
        self.assertTrue(any("build.rs:1" in error for error in errors))

    def test_top_level_integration_test_is_not_a_production_target(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base = self.make_package(root)
            (base / "tests").mkdir()
            (base / "tests" / "native.rs").write_text(
                "use std::os::unix::net::UnixStream;\n", encoding="utf-8"
            )
            self.assertEqual(self.validate(root), [])

    def test_inline_test_remains_inside_portable_source_policy(self) -> None:
        source = "#[cfg(test)] mod tests { use std::os::unix::net::UnixStream; }\n"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_package(root, source)
            errors = self.validate(root)
        self.assertTrue(any("standard-library OS module" in error for error in errors))

    def test_missing_package_and_invalid_utf8_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            missing_errors = self.validate(root)
            base = self.make_package(root)
            (base / "src" / "lib.rs").write_bytes(b"\xff\xfe")
            encoding_errors = self.validate(root)
        self.assertTrue(any("is missing" in error for error in missing_errors))
        self.assertTrue(any("cannot be read as UTF-8" in error for error in encoding_errors))


class DependencyBoundaryTests(unittest.TestCase):
    package = "narrivy-core"

    @staticmethod
    def dependency(
        name: str,
        *,
        kind: str | None = None,
        target: str | None = None,
        rename: str | None = None,
        path: str | None = None,
    ) -> dict[str, object]:
        return {
            "name": name,
            "kind": kind,
            "target": target,
            "rename": rename,
            "path": path,
            "source": None if path else "registry+https://example.invalid/index",
        }

    def metadata(self, root: Path, dependencies: list[dict[str, object]]) -> dict[str, object]:
        package_id = f"path+file://{root}/crates/{self.package}#0.0.0"
        return {
            "packages": [
                {
                    "id": package_id,
                    "name": self.package,
                    "manifest_path": str(root / "crates" / self.package / "Cargo.toml"),
                    "dependencies": dependencies,
                }
            ],
            "workspace_members": [package_id],
        }

    def validate(
        self, root: Path, dependencies: list[dict[str, object]]
    ) -> list[str]:
        return dependency_validator.validate_dependency_metadata(
            self.metadata(root, dependencies), root, (self.package,)
        )

    def test_neutral_normal_and_forbidden_dev_dependencies_pass(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            errors = self.validate(
                root,
                [
                    self.dependency("serde"),
                    self.dependency("tempfile"),
                    self.dependency("windows-sys", kind="dev"),
                    self.dependency(
                        "narrivy-platform-null",
                        kind="dev",
                        path=str(root / "adapters" / "platform-null"),
                    ),
                ],
            )
        self.assertEqual(errors, [])

    def test_direct_normal_implementation_dependency_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            errors = self.validate(root, [self.dependency("pipewire")])
        self.assertTrue(any("directly normal-depends" in error for error in errors))

    def test_build_and_target_specific_implementation_dependencies_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            errors = self.validate(
                root,
                [
                    self.dependency("libc", kind="build"),
                    self.dependency(
                        "windows-sys", target='cfg(target_os = "windows")'
                    ),
                ],
            )
        self.assertTrue(any("directly build-depends" in error for error in errors))
        self.assertTrue(any('cfg(target_os = "windows")' in error for error in errors))

    def test_renamed_dependency_uses_actual_package_identity(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            errors = self.validate(
                root, [self.dependency("libc", rename="portable_helper")]
            )
        self.assertTrue(any("libc (renamed portable_helper)" in error for error in errors))

    def test_production_workspace_adapter_edge_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            errors = self.validate(
                root,
                [
                    self.dependency(
                        "harmless-name", path=str(root / "adapters" / "platform-linux")
                    )
                ],
            )
        self.assertTrue(any("forbidden workspace layer adapters" in error for error in errors))

    def test_external_local_path_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary, tempfile.TemporaryDirectory() as outside:
            root = Path(temporary)
            errors = self.validate(
                root,
                [self.dependency("local-helper", path=str(Path(outside) / "helper"))],
            )
        self.assertTrue(any("outside the workspace" in error for error in errors))

    def test_missing_package_unknown_kind_and_malformed_metadata_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            missing = dependency_validator.validate_dependency_metadata(
                {"packages": [], "workspace_members": []}, root, (self.package,)
            )
            unknown = self.validate(
                root, [self.dependency("serde", kind="future-kind")]
            )
            malformed = dependency_validator.validate_dependency_metadata(
                {"packages": "not-a-list", "workspace_members": []},
                root,
                (self.package,),
            )
        self.assertTrue(any("0 workspace metadata entries" in error for error in missing))
        self.assertTrue(any("unknown kind" in error for error in unknown))
        self.assertTrue(any("not a list" in error for error in malformed))

    def test_metadata_loader_uses_locked_direct_dependency_command(self) -> None:
        completed = mock.Mock(returncode=0, stdout=json.dumps({"packages": []}), stderr="")
        with mock.patch.object(
            dependency_validator.shutil, "which", return_value="/usr/bin/cargo"
        ), mock.patch.object(
            dependency_validator.subprocess, "run", return_value=completed
        ) as run:
            dependency_validator.load_cargo_metadata(Path("/workspace"))
        self.assertEqual(
            run.call_args.args[0],
            ["cargo", "metadata", "--format-version", "1", "--locked", "--no-deps"],
        )
        self.assertEqual(run.call_args.kwargs["cwd"], Path("/workspace"))

    def test_metadata_loader_fails_closed_on_tool_json_and_command_errors(self) -> None:
        with mock.patch.object(dependency_validator.shutil, "which", return_value=None):
            with self.assertRaises(dependency_validator.MetadataLoadError):
                dependency_validator.load_cargo_metadata(Path("/workspace"))
        invalid_json = mock.Mock(returncode=0, stdout="not json", stderr="")
        with mock.patch.object(
            dependency_validator.shutil, "which", return_value="cargo"
        ), mock.patch.object(
            dependency_validator.subprocess, "run", return_value=invalid_json
        ):
            with self.assertRaises(dependency_validator.MetadataLoadError):
                dependency_validator.load_cargo_metadata(Path("/workspace"))
        failed = mock.Mock(returncode=101, stdout="", stderr="locked file mismatch")
        with mock.patch.object(
            dependency_validator.shutil, "which", return_value="cargo"
        ), mock.patch.object(dependency_validator.subprocess, "run", return_value=failed):
            with self.assertRaises(dependency_validator.MetadataLoadError):
                dependency_validator.load_cargo_metadata(Path("/workspace"))


class WorkspaceCompositionTests(unittest.TestCase):
    """A workspace member nothing can reach is dead code or missing wiring."""

    @staticmethod
    def member(
        root: Path,
        name: str,
        *,
        depends_on: Iterable[str] = (),
        dev_depends_on: Iterable[str] = (),
        build_depends_on: Iterable[str] = (),
        kinds: Iterable[str] = ("lib",),
    ) -> dict[str, object]:
        directory = root / "members" / name
        manifest = directory / "Cargo.toml"
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_text(
            f'[package]\nname = "{name}"\nversion = "0.0.0"\n', encoding="utf-8"
        )
        dependencies: list[dict[str, object]] = []
        for kind, targets in (
            (None, depends_on),
            ("build", build_depends_on),
            ("dev", dev_depends_on),
        ):
            dependencies.extend(
                {
                    "name": target,
                    "kind": kind,
                    "target": None,
                    "rename": None,
                    "path": str(root / "members" / target),
                    "source": None,
                }
                for target in targets
            )
        return {
            "id": f"path+file://{directory}#0.0.0",
            "name": name,
            "manifest_path": str(manifest),
            "dependencies": dependencies,
            "targets": [{"kind": [target_kind]} for target_kind in kinds],
        }

    def metadata(self, root: Path, members: Iterable[object]) -> dict[str, object]:
        packages = list(members)
        return {
            "packages": packages,
            "workspace_members": [
                str(package["id"])
                for package in packages
                if isinstance(package, dict) and isinstance(package.get("id"), str)
            ],
        }

    def validate(
        self,
        root: Path,
        members: Iterable[object],
        allowlist: Mapping[str, str] | None = None,
    ) -> list[str]:
        return dependency_validator.validate_workspace_composition(
            self.metadata(root, members), root, {} if allowlist is None else allowlist
        )

    def test_composed_member_and_binary_entry_points_pass(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            errors = self.validate(
                root,
                [
                    self.member(root, "leaf"),
                    self.member(root, "adapter"),
                    self.member(
                        root,
                        "daemon",
                        depends_on=["leaf", "adapter"],
                        kinds=("bin", "lib"),
                    ),
                    self.member(root, "cli", depends_on=["leaf"], kinds=("bin",)),
                ],
            )
        self.assertEqual(errors, [])

    def test_library_member_with_no_reverse_dependency_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            errors = self.validate(
                root,
                [self.member(root, "orphan"), self.member(root, "daemon", kinds=("bin",))],
            )
        self.assertEqual(
            errors,
            [
                "workspace member orphan is unreachable: no production reverse dependency, "
                "no bin target, and no library-leaf allowlist entry"
            ],
        )

    def test_dev_dependency_edge_is_not_production_composition(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            errors = self.validate(
                root,
                [
                    self.member(root, "test-only-leaf"),
                    self.member(root, "daemon", dev_depends_on=["test-only-leaf"], kinds=("bin",)),
                ],
            )
        self.assertTrue(
            any("workspace member test-only-leaf is unreachable" in error for error in errors),
            errors,
        )

    def test_build_dependency_edge_is_production_composition(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            errors = self.validate(
                root,
                [
                    self.member(root, "build-helper"),
                    self.member(root, "leaf", build_depends_on=["build-helper"]),
                ],
            )
        # The build-helper is composed by a build edge; only the top of the
        # chain is unreachable, which proves the build edge was counted.
        self.assertEqual(
            errors,
            [
                "workspace member leaf is unreachable: no production reverse dependency, "
                "no bin target, and no library-leaf allowlist entry"
            ],
        )

    def test_allowlisted_library_leaf_passes_only_with_a_justification(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            members = [self.member(root, "null-backend"), self.member(root, "daemon", kinds=("bin",))]
            allowed = self.validate(
                root, members, {"null-backend": "Null implementation of the platform contract."}
            )
            unjustified = self.validate(root, members, {"null-backend": "   "})
        self.assertEqual(allowed, [])
        self.assertTrue(
            any("has no written justification" in error for error in unjustified), unjustified
        )

    def test_stale_or_satisfied_allowlist_entries_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            missing = self.validate(
                root,
                [self.member(root, "daemon", kinds=("bin",))],
                {"gone-crate": "Reviewed earlier."},
            )
            satisfied = self.validate(
                root,
                [
                    self.member(root, "leaf"),
                    self.member(root, "daemon", depends_on=["leaf"], kinds=("bin",)),
                ],
                {"leaf": "Reviewed earlier."},
            )
        self.assertTrue(
            any("is not a workspace member" in error for error in missing), missing
        )
        self.assertTrue(
            any("now has production consumers" in error for error in satisfied), satisfied
        )

    def test_malformed_metadata_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            no_packages = dependency_validator.validate_workspace_composition(
                {"packages": [], "workspace_members": []}, root, {}
            )
            bad_list = dependency_validator.validate_workspace_composition(
                {"packages": "nope", "workspace_members": []}, root, {}
            )
            bad_members = dependency_validator.validate_workspace_composition(
                {"packages": [], "workspace_members": [1]}, root, {}
            )
        self.assertTrue(any("no workspace member packages" in error for error in no_packages), no_packages)
        self.assertTrue(any("'packages' is missing or not a list" in error for error in bad_list), bad_list)
        self.assertTrue(
            any("'workspace_members' is missing or malformed" in error for error in bad_members),
            bad_members,
        )

    def test_malformed_targets_and_dependency_entries_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            no_targets = self.member(root, "leaf")
            no_targets["targets"] = "not-a-list"
            bad_kind = self.member(root, "other")
            bad_kind["dependencies"] = [{"name": "thing", "kind": "future-kind", "path": None}]
            no_dependencies = self.member(root, "third")
            no_dependencies["dependencies"] = "not-a-list"
            duplicate = self.member(root, "fourth")
            errors = self.validate(
                root,
                [
                    no_targets,
                    bad_kind,
                    no_dependencies,
                    duplicate,
                    dict(duplicate),
                    "not-a-package",
                ],
            )
        self.assertTrue(any("targets metadata is missing" in error for error in errors), errors)
        self.assertTrue(any("unknown kind" in error for error in errors), errors)
        self.assertTrue(
            any("third dependencies metadata is missing" in error for error in errors), errors
        )
        self.assertTrue(any("appears more than once" in error for error in errors), errors)
        self.assertTrue(any("non-object package entry" in error for error in errors), errors)

    # The crates the defect report named. Each one is asserted to be flagged
    # exactly while it still has no production consumer, so composing any of
    # them turns this into a no-op instead of a stale failure.
    REPORTED_ORPHANS = ("narrivy-history", "narrivy-diagnostics", "narrivy-compute")

    @staticmethod
    def independent_orphans(metadata: dict[str, object], root: Path) -> set[str]:
        """Recompute the unreachable members without reusing validator code."""

        members = {
            package["name"]: package
            for package in metadata["packages"]  # type: ignore[index]
            if package["id"] in set(metadata["workspace_members"])  # type: ignore[index]
        }
        directories = {
            str(Path(package["manifest_path"]).resolve().parent): name
            for name, package in members.items()
        }
        consumed: set[str] = set()
        for package in members.values():
            for dependency in package["dependencies"]:
                if dependency.get("kind") == "dev" or not dependency.get("path"):
                    continue
                consumed.add(directories.get(str(Path(dependency["path"]).resolve()), ""))
        return {
            name
            for name, package in members.items()
            if name not in consumed
            and not any("bin" in target.get("kind", []) for target in package["targets"])
        }

    def test_flagged_members_are_exactly_the_unreachable_ones(self) -> None:
        """The shipped check, on the shipped tree, matches a second derivation."""

        metadata = dependency_validator.load_cargo_metadata(ROOT)
        errors = dependency_validator.validate_workspace_composition(metadata, ROOT)
        flagged = {
            error.split(" ")[2] for error in errors if " is unreachable" in error
        }
        allowlist = set(boundary_policy.LIBRARY_LEAF_ALLOWLIST)
        self.assertEqual(
            flagged,
            self.independent_orphans(metadata, ROOT) - allowlist,
            "the orphan rule disagrees with an independent derivation",
        )
        # No error may be anything other than a genuine orphan finding.
        self.assertEqual(len(errors), len(flagged), errors)

    def test_reported_orphans_are_flagged_exactly_while_uncomposed(self) -> None:
        metadata = dependency_validator.load_cargo_metadata(ROOT)
        errors = dependency_validator.validate_workspace_composition(metadata, ROOT)
        orphaned = self.independent_orphans(metadata, ROOT)
        for name in self.REPORTED_ORPHANS:
            with self.subTest(crate=name):
                flagged = any(f"workspace member {name} is unreachable" in error for error in errors)
                self.assertEqual(flagged, name in orphaned)

    def test_shipped_allowlist_entries_are_justified_and_still_orphans(self) -> None:
        for name, justification in boundary_policy.LIBRARY_LEAF_ALLOWLIST.items():
            with self.subTest(crate=name):
                self.assertGreater(
                    len(justification.split()),
                    15,
                    f"{name} needs a written justification a reviewer can check",
                )
        metadata = dependency_validator.load_cargo_metadata(ROOT)
        orphaned = self.independent_orphans(metadata, ROOT)
        for name in boundary_policy.LIBRARY_LEAF_ALLOWLIST:
            with self.subTest(crate=name):
                self.assertIn(
                    name,
                    orphaned,
                    f"{name} now has a production consumer; remove the stale allowlist entry",
                )

    def test_protected_set_covers_every_portable_crate_named_in_the_report(self) -> None:
        for name in (
            "narrivy-history",
            "narrivy-diagnostics",
            "narrivy-compute",
            "narrivy-model-manager",
        ):
            with self.subTest(crate=name):
                self.assertIn(name, boundary_policy.PROTECTED_PACKAGES)

    def test_protected_packages_all_exist_as_workspace_members(self) -> None:
        metadata = dependency_validator.load_cargo_metadata(ROOT)
        names = {
            package["name"]
            for package in metadata["packages"]  # type: ignore[index]
            if package["id"] in set(metadata["workspace_members"])  # type: ignore[index]
        }
        for name in boundary_policy.PROTECTED_PACKAGES:
            with self.subTest(crate=name):
                self.assertIn(name, names)


if __name__ == "__main__":
    unittest.main()
