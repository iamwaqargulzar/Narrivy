#!/usr/bin/env python3
"""F-H16: no Rust test may compose a live delivery rung.

The real `System*Rung` rungs (today `SystemWtypeKeyboardRung`) and the live
clipboard-paste parts (`WaylandClipboardHandoff`, `WtypePasteChord`,
`UinputPasteChord`) write the user's clipboard or type into the focused
window. `cargo test` run from a desktop terminal must never do that, so:

1. test code (integration test files and `#[cfg(test)]` / `#[test]` items)
   never names them;
2. outside the adapter that defines them, production code may construct them
   only inside an item gated with `not(test)`, so tests that compose the real
   ladder get the `cfg(test)` stand-ins instead.

The scanner is checked against synthetic sources first, so a pass on the real
tree is not vacuous.
"""

from __future__ import annotations

import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# The adapter that defines the live rungs. Its own production code builds
# them; its tests are still checked.
DEFINING_CRATE = "adapters/platform-linux/"

LIVE_NAME = re.compile(
    r"\b(?:System\w*Rung|WaylandClipboardHandoff|WtypePasteChord|UinputPasteChord)\b"
)
LIVE_CONSTRUCTION = re.compile(
    r"\b(?:System\w*Rung|WaylandClipboardHandoff|WtypePasteChord|UinputPasteChord)"
    r"\s*(?:::\s*\w+\s*\(|\{)"
)
ATTRIBUTE = re.compile(r"#!?\[")
CFG = re.compile(r"^#\[\s*cfg\s*\((.*)\)\s*\]$", re.S)
TEST_ATTRIBUTE = re.compile(r"^#\[\s*(?:\w+\s*::\s*)*test\b")
ITEM_KEYWORD = re.compile(r"\b(?:fn|impl|mod|struct|enum|trait|union|where)\b")


def _blank(match: re.Match[str]) -> str:
    return re.sub(r"[^\n]", " ", match.group(0))


def strip_comments_and_strings(source: str) -> str:
    """Blank comments and literals, keeping offsets and line numbers."""
    pattern = re.compile(
        r"//[^\n]*"
        r"|/\*.*?\*/"
        r'|b?r(#*)".*?"\1'
        r'|b?"(?:\\.|[^"\\])*"'
        r"|b?'(?:\\u\{[0-9a-fA-F]+\}|\\.|[^\\'\n])'",
        re.S,
    )
    return pattern.sub(_blank, source)


def _attribute_end(code: str, start: int) -> int:
    depth = 0
    for index in range(start, len(code)):
        if code[index] == "[":
            depth += 1
        elif code[index] == "]":
            depth -= 1
            if depth == 0:
                return index + 1
    return len(code)


def _item_end(code: str, start: int) -> int:
    """End of the item after its attributes: its matching `}`, or a `;`
    (or `,` for a field or argument) at depth zero."""
    depth = 0
    # Generic brackets count only in the item header (`impl<A, B>`), before
    # its body or initializer, where `<` cannot be a comparison.
    header = True
    index = start
    while index < len(code):
        char = code[index]
        if header and char in "<>" and code[index - 1] not in "-=":
            depth += 1 if char == "<" else -1
        elif header and depth == 0 and (char == "{" or (char == "=" and code[index + 1] != ">")):
            header = False
            if char == "{":
                depth += 1
        elif char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
            if depth < 0:
                return index
            if depth == 0 and char == "}":
                return index + 1
        elif char == ";" and depth == 0:
            return index + 1
        elif char == "," and depth == 0 and not ITEM_KEYWORD.search(code, start, index):
            # A gated field, argument or variant; a `where` clause comma is not
            # the end of an item.
            return index + 1
        elif char == "#" and depth == 0 and ATTRIBUTE.match(code, index):
            index = _attribute_end(code, index)
            continue
        index += 1
    return len(code)


def _is_test_gate(attribute: str) -> bool:
    if TEST_ATTRIBUTE.match(attribute):
        return True
    cfg = CFG.match(attribute)
    if not cfg:
        return False
    expression = re.sub(r"not\s*\(\s*test\s*\)", "", cfg.group(1))
    return re.search(r"\btest\b", expression) is not None


def _is_not_test_gate(attribute: str) -> bool:
    cfg = CFG.match(attribute)
    return bool(cfg) and re.search(r"not\s*\(\s*test\s*\)", cfg.group(1)) is not None


def gated_regions(code: str) -> tuple[list[tuple[int, int]], list[tuple[int, int]]]:
    """Return (test regions, `not(test)` regions) as offset ranges."""
    tests: list[tuple[int, int]] = []
    production: list[tuple[int, int]] = []
    for match in ATTRIBUTE.finditer(code):
        start = match.start()
        end = _attribute_end(code, start)
        attribute = re.sub(r"\s+", " ", code[start:end])
        if _is_test_gate(attribute):
            tests.append((start, _item_end(code, end)))
        elif _is_not_test_gate(attribute):
            production.append((start, _item_end(code, end)))
    return tests, production


def _inside(offset: int, regions: list[tuple[int, int]]) -> bool:
    return any(start <= offset < end for start, end in regions)


def is_test_file(path: str) -> bool:
    parts = Path(path).parts
    return "src" not in parts and ("tests" in parts or "benches" in parts)


def violations(path: str, source: str) -> list[str]:
    code = strip_comments_and_strings(source)

    def where(offset: int) -> str:
        return f"{path}:{code.count(chr(10), 0, offset) + 1}"

    if is_test_file(path):
        return [
            f"{where(m.start())}: test file names live rung {m.group(0)}"
            for m in LIVE_NAME.finditer(code)
        ]
    tests, production = gated_regions(code)
    found = [
        f"{where(m.start())}: test code names live rung {m.group(0)}"
        for m in LIVE_NAME.finditer(code)
        if _inside(m.start(), tests)
    ]
    if not path.startswith(DEFINING_CRATE):
        found += [
            f"{where(m.start())}: live rung built outside a not(test) item"
            for m in LIVE_CONSTRUCTION.finditer(code)
            if not _inside(m.start(), tests) and not _inside(m.start(), production)
        ]
    return found


def rust_sources() -> list[str]:
    listed = subprocess.run(
        ["git", "ls-files", "-z", "--", "*.rs"],
        cwd=ROOT,
        capture_output=True,
        check=True,
    ).stdout.decode()
    return sorted(
        path
        for path in listed.split("\0")
        if path and not path.startswith(("archive/", "agent/history/"))
    )


class ScannerFixtures(unittest.TestCase):
    def test_unit_test_naming_a_live_rung_fails(self) -> None:
        source = """
fn production() {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn types() {
        let _ = SystemWtypeKeyboardRung::new();
    }
}
"""
        self.assertEqual(len(violations("services/x/src/lib.rs", source)), 1)

    def test_cfg_all_test_function_naming_live_clipboard_fails(self) -> None:
        source = """
#[cfg(all(feature = "asr", test))]
fn live() -> Box<dyn DirectRung> {
    Box::new(ClipboardPasteRung::for_focused_window(
        WaylandClipboardHandoff::new(s),
        WtypePasteChord::shift_insert(),
    ))
}
"""
        self.assertEqual(len(violations("services/x/src/lib.rs", source)), 2)

    def test_integration_test_file_naming_a_live_rung_fails(self) -> None:
        source = "use narrivy_platform_linux::delivery::SystemWtypeKeyboardRung;\n"
        self.assertEqual(len(violations("services/x/tests/e2e.rs", source)), 1)

    def test_ungated_production_construction_fails(self) -> None:
        # A helper with no cfg gate is reachable from tests (the F-H16 bug).
        source = """
fn live_keyboard_rung() -> Box<dyn DirectRung> {
    Box::new(SystemWtypeKeyboardRung::new())
}
"""
        self.assertEqual(len(violations("services/x/src/lib.rs", source)), 1)

    def test_not_test_gated_construction_and_stand_in_pass(self) -> None:
        source = """
use narrivy_platform_linux::delivery::SystemWtypeKeyboardRung;

/// SystemWtypeKeyboardRung in a doc comment is fine.
#[cfg(all(feature = "asr", not(test)))]
fn live_keyboard_rung() -> Box<dyn DirectRung> {
    Box::new(SystemWtypeKeyboardRung::new())
}

#[cfg(all(feature = "asr", test))]
fn live_keyboard_rung() -> Box<dyn DirectRung> {
    Box::new(UnavailableRung::new("wtype-virtual-keyboard", "disabled"))
}

#[cfg(test)]
mod tests {
    const NOTE: &str = "SystemWtypeKeyboardRung";
    #[test]
    fn composes() { let _ = live_keyboard_rung(); }
}
"""
        self.assertEqual(violations("services/x/src/lib.rs", source), [])

    def test_defining_adapter_may_build_but_its_tests_may_not(self) -> None:
        source = """
impl Default for SystemWtypeKeyboardRung {
    fn default() -> Self { SystemWtypeKeyboardRung::new() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() { let _ = SystemWtypeKeyboardRung::default(); }
}
"""
        path = "adapters/platform-linux/src/delivery/routed.rs"
        self.assertEqual(len(violations(path, source)), 1)


class RepositoryHasNoLiveRungInTests(unittest.TestCase):
    def test_no_rust_test_composes_a_live_delivery_rung(self) -> None:
        sources = rust_sources()
        self.assertTrue(
            any(path.startswith("services/narrivyd/src/") for path in sources),
            "the scan found no daemon sources",
        )
        found: list[str] = []
        for path in sources:
            found += violations(path, (ROOT / path).read_text(encoding="utf-8"))
        self.assertEqual(found, [])

    def test_the_daemon_still_builds_the_live_rungs_somewhere(self) -> None:
        # Keeps rule 2 meaningful: if the construction moves, update the scan.
        session = (ROOT / "services/narrivyd/src/daemon/session.rs").read_text(
            encoding="utf-8"
        )
        code = strip_comments_and_strings(session)
        _, production = gated_regions(code)
        built = [m.start() for m in LIVE_CONSTRUCTION.finditer(code)]
        self.assertTrue(built)
        self.assertTrue(all(_inside(offset, production) for offset in built))


if __name__ == "__main__":
    unittest.main()
