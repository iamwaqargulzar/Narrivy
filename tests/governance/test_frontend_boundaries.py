#!/usr/bin/env python3
"""Deterministic positive and negative fixtures for the frontend source guard.

The frontend must render capabilities reported by the daemon. Every rule here
exists because a user-visible OS, device, vendor-runtime or delivery-method
name is a claim the daemon never made, and because OS-name branching in the UI
re-creates the probing that the capability contract replaced.

Each guard is proven non-vacuous twice: once against a synthetic tree, and
once against the real ``apps/narrivy-desktop/src`` tree by planting a
temporary violating file, running the real script, and deleting it again.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts import boundary_policy, verify_frontend_boundaries as frontend_validator


CAPABILITY_CATALOG = """{
  "version": 1,
  "states": [
    {"capability": "x11.hotkey", "state": "trigger-x11-fallback"},
    {"capability": "portal.globalshortcuts", "state": "trigger-portal"}
  ]
}
"""


def make_source_tree(root: Path, sources: dict[str, str], catalog: str = CAPABILITY_CATALOG) -> Path:
    """Build a minimal frontend source tree plus its capability catalog."""

    base = root / "apps" / "narrivy-desktop" / "src"
    base.mkdir(parents=True)
    (root / "spec" / "catalogs").mkdir(parents=True)
    (root / "spec" / "catalogs" / "capability-states.json").write_text(
        catalog, encoding="utf-8"
    )
    for relative, text in sources.items():
        path = base / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return base


def scan(root: Path, sources: dict[str, str], **kwargs) -> list[str]:
    make_source_tree(root, sources)
    kwargs.setdefault("min_files", 1)
    return frontend_validator.validate_frontend_sources(
        root, allowlist=(), patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS, **kwargs
    )


class CommentMaskingTests(unittest.TestCase):
    """Only code is scanned: a comment documenting a denylist is not a claim."""

    @staticmethod
    def visible(text: str) -> str:
        return frontend_validator.mask_typescript_comments(text)

    def test_mask_preserves_offsets_and_newlines(self) -> None:
        source = "const a = 1; // note\n/* block */\nconst b = 2;\n"
        masked = self.visible(source)
        self.assertEqual(len(masked), len(source))
        self.assertEqual(masked.count("\n"), source.count("\n"))
        self.assertEqual(masked, "const a = 1;        \n           \nconst b = 2;\n")

    def test_line_and_block_comments_are_masked(self) -> None:
        self.assertNotIn("Intel NPU", self.visible("// Intel NPU\n"))
        self.assertNotIn("OpenVINO", self.visible("/* OpenVINO\n   uinput */\n"))

    def test_literal_comment_marker_does_not_open_a_comment(self) -> None:
        masked = self.visible("const u = 'https://host.invalid//path';\nconst b = 'Intel NPU';\n")
        self.assertIn("Intel NPU", masked)

    def test_template_url_cannot_hide_a_later_literal(self) -> None:
        masked = self.visible("const t = `https://host.invalid//path NVIDIA`;\n")
        self.assertIn("NVIDIA", masked)

    def test_comment_inside_interpolation_is_still_a_comment(self) -> None:
        masked = self.visible("const t = `x ${ y /* Intel NPU */ + z } ydotool`;\n")
        self.assertNotIn("Intel NPU", masked)
        self.assertIn("ydotool", masked)

    def test_nested_object_literal_in_interpolation_is_scanned(self) -> None:
        masked = self.visible("const t = `${JSON.stringify({a: 1})} libei`;\n")
        self.assertIn("libei", masked)

    def test_unterminated_comment_masks_to_end_of_file(self) -> None:
        self.assertEqual(self.visible("/* Intel NPU\nconst b = 'ydotool';\n").strip(), "")


class ForbiddenLiteralTests(unittest.TestCase):
    """One fixture per rule, using the shipped policy patterns."""

    RULES = {
        "accelerator vendor or model name": [
            "const a = 'Intel NPU';",
            "const b = 'Intel Arc A770';",
            "const c = 'NVIDIA RTX 4090';",
            "const d = 'AMD Ryzen 9';",
            "const e = 'Radeon';",
            "const f = 'M1 Max';",
            "const g = 'Intel iGPU';",
        ],
        "engine runtime or model name": [
            "const a = 'OpenVINO';",
            "const b = 'openvino';",
            "const c = 'Whisper Large-v3 INT8';",
        ],
        "virtual-input delivery method": ["const a = 'uinput / ydotool';"],
        "portal/libei delivery method": ["const a = 'Portal / libei';", "const b = 'libei';"],
        "X11 delivery method": ["const a = 'X11 / XWayland';", "const b = 'XWayland';"],
        "Wayland virtual keyboard": ["const a = 'Wayland Virtual Keyboard';"],
        "OS-name branching": [
            "if (navigator.platform.startsWith('Linux')) {}",
            "if (process.platform === 'win32') {}",
            "const ua = navigator.userAgent;",
            "if (isLinux) {}",
            "if (isMacOS) {}",
            "if (isWindows) {}",
        ],
    }

    def test_every_shipped_rule_has_a_failing_fixture(self) -> None:
        shipped = {label for label, _ in boundary_policy.FRONTEND_FORBIDDEN_PATTERNS}
        self.assertEqual(shipped, set(self.RULES))

    def test_each_rule_fails_with_its_own_label(self) -> None:
        for label, fixtures in self.RULES.items():
            for fixture in fixtures:
                with self.subTest(rule=label, fixture=fixture):
                    with tempfile.TemporaryDirectory() as temporary:
                        errors = scan(Path(temporary), {"app/leak.ts": fixture + "\n"})
                    self.assertTrue(
                        any(f"forbidden {label}" in error for error in errors),
                        f"expected {label} violation for {fixture!r}, got {errors!r}",
                    )
                    self.assertTrue(
                        any("app/leak.ts:1" in error for error in errors), errors
                    )

    def test_jsx_text_and_identifiers_are_scanned(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            errors = scan(
                Path(temporary),
                {
                    "app/View.tsx": (
                        "export const View = () => (\n"
                        "  <p title=\"Intel NPU\">NVIDIA RTX 4090</p>\n"
                        ");\n"
                    )
                },
            )
        self.assertEqual(len(errors), 3, errors)
        for literal in ("Intel NPU", "NVIDIA", "RTX 4090"):
            self.assertTrue(any(repr(literal) in error for error in errors), errors)

    def test_capability_token_vocabulary_is_not_a_claim(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            errors = scan(
                Path(temporary),
                {
                    "lib/states.ts": "export const T = 'trigger-x11-fallback';\n",
                    "lib/hotkey.ts": "export const C = 'x11.hotkey';\n",
                    "lib/portal.ts": "export const P = 'trigger-portal';\n",
                },
            )
        self.assertEqual(errors, [])

    def test_catalog_token_exemption_does_not_excuse_a_bare_name(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            errors = scan(
                Path(temporary),
                {"app/leak.ts": "const a = 'X11';\nconst b = 'NVIDIA';\n"},
            )
        self.assertTrue(any("X11" in error for error in errors), errors)
        self.assertTrue(any("NVIDIA" in error for error in errors), errors)


class ScopeAndPolicyTests(unittest.TestCase):
    def test_test_modules_are_excluded_from_the_scan(self) -> None:
        fixtures = {
            "__tests__/Guard.test.ts": "const a = 'Intel NPU';\n",
            "lib/thing.test.tsx": "const a = 'NVIDIA';\n",
            "lib/thing.spec.ts": "const a = 'OpenVINO';\n",
            "test/mockBridge.ts": "const a = 'uinput';\n",
        }
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            make_source_tree(root, {**fixtures, "app/ok.ts": "export const a = 1;\n"})
            self.assertEqual(
                frontend_validator.validate_frontend_sources(
                    root,
                    allowlist=(),
                    patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                    min_files=1,
                ),
                [],
            )
            scanned, excluded, _errors = frontend_validator._discover_source_files(
                root / "apps" / "narrivy-desktop" / "src"
            )
        self.assertEqual([p.as_posix() for p in scanned], ["app/ok.ts"])
        self.assertEqual(len(excluded), 4)

    def test_coverage_floor_fails_closed_on_a_vacuous_scan(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            make_source_tree(root, {"app/ok.ts": "export const a = 1;\n"})
            errors = frontend_validator.validate_frontend_sources(
                root,
                allowlist=(),
                patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                min_files=2,
            )
        self.assertTrue(any("coverage floor" in error for error in errors), errors)

    def test_empty_or_unlabelled_policy_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            make_source_tree(root, {"app/ok.ts": "export const a = 1;\n"})
            no_rules = frontend_validator.validate_frontend_sources(
                root, allowlist=(), patterns=(), min_files=1
            )
            unlabelled = frontend_validator.validate_frontend_sources(
                root,
                allowlist=(),
                patterns=(("", __import__("re").compile("x")),),
                min_files=1,
            )
        self.assertTrue(any("no forbidden patterns" in error for error in no_rules), no_rules)
        self.assertTrue(any("unlabelled" in error for error in unlabelled), unlabelled)

    def test_missing_source_dir_and_unreadable_catalog_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            missing = frontend_validator.validate_frontend_sources(
                root, allowlist=(), patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS
            )
        self.assertTrue(any("source directory is missing" in error for error in missing), missing)
        self.assertTrue(any("cannot be read as JSON" in error for error in missing), missing)

    def test_malformed_catalog_entries_fail_closed(self) -> None:
        for catalog in ('{"version": 1, "states": []}', '{"version": 1, "states": [{"capability": 1}]}'):
            with self.subTest(catalog=catalog):
                with tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    make_source_tree(
                        root, {"app/ok.ts": "export const a = 1;\n"}, catalog=catalog
                    )
                    errors = frontend_validator.validate_frontend_sources(
                        root,
                        allowlist=(),
                        patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                        min_files=1,
                    )
                self.assertTrue(
                    any("capability-states.json" in error for error in errors), errors
                )

    def test_invalid_utf8_source_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base = make_source_tree(root, {"app/ok.ts": "export const a = 1;\n"})
            (base / "app" / "bad.ts").write_bytes(b"\xff\xfe")
            errors = frontend_validator.validate_frontend_sources(
                root, allowlist=(), patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                min_files=1,
            )
        self.assertTrue(any("cannot be read as UTF-8" in error for error in errors), errors)

    def test_allowlist_requires_a_real_justified_file(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            make_source_tree(root, {"app/ok.ts": "export const a = 1;\n"})
            stale = frontend_validator.validate_frontend_sources(
                root,
                allowlist=(("app/gone.ts", "reviewed"),),
                patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                min_files=1,
            )
            unjustified = frontend_validator.validate_frontend_sources(
                root,
                allowlist=(("app/ok.ts", "   "),),
                patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                min_files=1,
            )
            duplicated = frontend_validator.validate_frontend_sources(
                root,
                allowlist=(("app/ok.ts", "a"), ("app/ok.ts", "b")),
                patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                min_files=1,
            )
            malformed = frontend_validator.validate_frontend_sources(
                root,
                allowlist=(("app/ok.ts",),),
                patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                min_files=1,
            )
        self.assertTrue(any("is stale" in error for error in stale), stale)
        self.assertTrue(any("no written justification" in error for error in unjustified), unjustified)
        self.assertTrue(any("more than once" in error for error in duplicated), duplicated)
        self.assertTrue(any("(path, justification) pair" in error for error in malformed), malformed)

    def test_allowlist_exemption_must_still_be_load_bearing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            make_source_tree(
                root, {"app/leak.ts": "const a = 'Intel NPU';\n", "app/ok.ts": "export const a = 1;\n"}
            )
            used = frontend_validator.validate_frontend_sources(
                root,
                allowlist=(("app/leak.ts", "reviewed placeholder"),),
                patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                min_files=1,
            )
            unused = frontend_validator.validate_frontend_sources(
                root,
                allowlist=(("app/ok.ts", "reviewed placeholder"),),
                patterns=boundary_policy.FRONTEND_FORBIDDEN_PATTERNS,
                min_files=1,
            )
        self.assertEqual(used, [])
        self.assertTrue(
            any("no longer needs its exemption" in error for error in unused), unused
        )


class ShippedPolicyTests(unittest.TestCase):
    """The policy that ships must itself be complete and justified."""

    def test_every_shipped_allowlist_entry_is_justified_and_present(self) -> None:
        base = ROOT / "apps" / "narrivy-desktop" / "src"
        # An empty allowlist is the goal; any entry that exists must be
        # justified and point at a real file.
        for path, justification in boundary_policy.FRONTEND_ALLOWLIST:
            with self.subTest(path=path):
                self.assertTrue(justification.strip())
                self.assertGreater(len(justification.split()), 15, justification)
                self.assertTrue((base / path).is_file(), f"stale allowlist entry: {path}")

    def test_shipped_rules_cover_every_mandated_category(self) -> None:
        labels = " ".join(
            label for label, _ in boundary_policy.FRONTEND_FORBIDDEN_PATTERNS
        ).lower()
        for token in (
            "accelerator",
            "runtime",
            "delivery",
            "keyboard",
            "os-name branching",
        ):
            self.assertIn(token, labels)

    def test_no_mandated_literal_class_is_absent_from_the_pattern_table(self) -> None:
        """Every literal class named in the defect report must actually match."""

        table = boundary_policy.FRONTEND_FORBIDDEN_PATTERNS
        for literal in (
            "NVIDIA RTX 4090",
            "Intel NPU",
            "Intel Arc",
            "Whisper Large-v3 INT8",
            "M1 Max",
            "OpenVINO",
            "uinput / ydotool",
            "Portal / libei",
            "X11 / XWayland",
            "Wayland Virtual Keyboard",
            "navigator.platform",
            "process.platform",
            "navigator.userAgent",
            "isLinux",
            "isMacOS",
            "isWindows",
        ):
            with self.subTest(literal=literal):
                matched = [
                    label
                    for label, pattern in table
                    if any(m.group(0) for m in pattern.finditer(f"const a = '{literal}';"))
                    or any(m.group(0) for m in pattern.finditer(f"if ({literal}) {{}}"))
                ]
                self.assertTrue(matched, f"no shipped rule matches {literal!r}")

    def test_catalog_and_source_dir_exist(self) -> None:
        self.assertTrue((ROOT / boundary_policy.FRONTEND_CAPABILITY_CATALOG).is_file())
        self.assertTrue((ROOT / boundary_policy.FRONTEND_SOURCE_DIR).is_dir())


class LiveFrontendGuardTests(unittest.TestCase):
    """Run the real script against the real tree, proving it is not vacuous."""

    SRC = ROOT / "apps" / "narrivy-desktop" / "src"

    def run_guard(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPTS / "verify_frontend_boundaries.py")],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )

    def plant(self, relative: str, text: str) -> Path:
        path = self.SRC / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        self.assertFalse(path.exists(), f"refusing to overwrite {relative}")
        path.write_text(text, encoding="utf-8")
        self.addCleanup(path.unlink, True)
        return path

    def test_live_tree_passes(self) -> None:
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Frontend boundaries OK", result.stdout)

    def test_planted_violation_fails_with_a_useful_message(self) -> None:
        self.plant(
            "app/components/SettingsCompute/_governance_plant.tsx",
            "// The Compute tab once hard-coded these; a comment must stay clean.\n"
            "export const resolved = (os: string): string => {\n"
            "  if (navigator.platform.startsWith('Linux')) return 'Intel NPU / OpenVINO';\n"
            "  if (process.platform === 'win32') return 'NVIDIA RTX 4090';\n"
            "  return `M1 Max via ' + 'X11 / XWayland' + ' (uinput / ydotool, '\n"
            "    + 'Portal / libei, Wayland Virtual Keyboard) Whisper Large-v3 INT8`;\n"
            "};\n",
        )
        result = self.run_guard()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Frontend boundary validation FAILED", result.stdout)
        for expected in (
            "_governance_plant.tsx:3",
            "'navigator.platform'",
            "'Intel NPU'",
            "'OpenVINO'",
            "_governance_plant.tsx:4",
            "'process.platform'",
            "'NVIDIA'",
            "'RTX 4090'",
            "_governance_plant.tsx:5",
            "'M1 Max'",
            "'X11'",
            "'XWayland'",
            "'uinput'",
            "'ydotool'",
            "_governance_plant.tsx:6",
            "'Wayland Virtual Keyboard'",
            "'Whisper'",
            "'Portal / libei'",
        ):
            self.assertIn(expected, result.stdout)
        # The comment on line 1 must not be reported.
        self.assertNotIn("_governance_plant.tsx:1", result.stdout)
        self.assertNotIn("_governance_plant.tsx:2", result.stdout)

    def test_planted_test_module_does_not_trip_the_guard(self) -> None:
        self.plant(
            "__tests__/_governance_plant.test.ts",
            "// Enumerating the denylist is this test's job.\n"
            "const FORBIDDEN = ['Intel NPU', 'OpenVINO', 'uinput', 'X11 / XWayland'];\n"
            "export default FORBIDDEN;\n",
        )
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_planted_capability_token_source_passes(self) -> None:
        self.plant(
            "lib/_governance_plant.ts",
            "export const STATE = 'trigger-x11-fallback';\n"
            "export const CAPABILITY = 'x11.hotkey';\n",
        )
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_scan_covers_a_meaningful_share_of_the_live_tree(self) -> None:
        scanned, excluded = frontend_validator.scanned_file_counts(ROOT)
        self.assertGreaterEqual(scanned, boundary_policy.FRONTEND_MIN_SCANNED_FILES)
        self.assertGreater(excluded, 0)
        scanned_paths = {
            p.relative_to(self.SRC).as_posix()
            for p in self.SRC.rglob("*")
            if p.suffix in boundary_policy.FRONTEND_SOURCE_SUFFIXES
            and p.is_file()
            and not frontend_validator.is_test_module(p.relative_to(self.SRC))
        }
        self.assertEqual(len(scanned_paths), scanned)
        self.assertIn("app/screens/Models.tsx", scanned_paths)
        self.assertIn("lib/resolved.ts", scanned_paths)
        self.assertIn("lib/capabilityStates.ts", scanned_paths)
        self.assertTrue(
            {"app", "lib", "state", "ui", "hooks", "i18n", "theme"}
            & {Path(s).parts[0] for s in scanned_paths}
        )


if __name__ == "__main__":
    unittest.main()
