#!/usr/bin/env python3
"""Fail-closed source boundary validation for the Narrivy desktop frontend.

The frontend renders capabilities reported by the daemon. It must not name an
OS, a device, a vendor runtime, or a delivery mechanism as if it had probed for
one, and it must not branch on OS names. A user-visible ``Intel NPU`` or
``X11 / XWayland`` is a claim the daemon never made, which is how the Compute
tab once claimed hardware that no daemon call could produce.

Only code is scanned: comments are masked, so documenting a denylist in a source
comment is not itself a violation. Test modules are excluded because
enumerating the forbidden literals is a test's job. Nothing else is exempt, and
every exemption must be justified and still load-bearing.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any, Iterable

try:
    from boundary_policy import (
        FRONTEND_ALLOWLIST,
        FRONTEND_CAPABILITY_CATALOG,
        FRONTEND_FORBIDDEN_PATTERNS,
        FRONTEND_MIN_SCANNED_FILES,
        FRONTEND_SOURCE_DIR,
        FRONTEND_SOURCE_SUFFIXES,
        FRONTEND_TEST_DIR_NAMES,
        FRONTEND_TEST_FILE_RE,
    )
except ModuleNotFoundError:  # Imported by the fixture suite.
    from scripts.boundary_policy import (
        FRONTEND_ALLOWLIST,
        FRONTEND_CAPABILITY_CATALOG,
        FRONTEND_FORBIDDEN_PATTERNS,
        FRONTEND_MIN_SCANNED_FILES,
        FRONTEND_SOURCE_DIR,
        FRONTEND_SOURCE_SUFFIXES,
        FRONTEND_TEST_DIR_NAMES,
        FRONTEND_TEST_FILE_RE,
    )


ROOT = Path(__file__).resolve().parents[1]


def _mask_range(buffer: list[str], start: int, end: int) -> None:
    for index in range(start, end):
        if buffer[index] not in "\r\n":
            buffer[index] = " "


def _skip_quoted(text: str, index: int, quote: str) -> int:
    """Return the index just past a quoted string that starts at ``index``."""

    cursor = index + 1
    while cursor < len(text):
        if text[cursor] == "\\":
            cursor += 2
            continue
        if text[cursor] == quote:
            return cursor + 1
        if text[cursor] == "\n":
            return cursor
        cursor += 1
    return len(text)


def mask_typescript_comments(text: str) -> str:
    """Mask ``//`` and ``/* */`` comments while preserving offsets/newlines.

    String and template literals stay visible because a forbidden literal in
    application source is exactly what must be reported. Template text is
    tracked as its own state, so a URL such as ``https://host//path`` inside a
    template can never open a comment and hide a violation that follows it, and
    ``${...}`` interpolations are counted so a comment inside an interpolation
    is still recognised as a comment.
    """

    masked = list(text)
    length = len(text)
    index = 0
    # Frames are ``["code", brace_depth]`` or ``["template", 0]``. A template's
    # brace depth counts nesting inside its current ``${...}`` interpolation.
    frames: list[list[Any]] = [["code", 0]]
    while index < length:
        char = text[index]
        kind, depth = frames[-1]

        if kind == "template":
            if char == "\\":
                index += 2
                continue
            if char == "`":
                frames.pop()
                index += 1
                continue
            if text.startswith("${", index):
                frames.append(["code", 0])
                index += 2
                continue
            # Template text is never a comment, so a URL containing ``//``
            # cannot open one and hide a literal that follows it.
            index += 1
            continue

        if text.startswith("//", index):
            end = text.find("\n", index + 2)
            end = length if end < 0 else end
            _mask_range(masked, index, end)
            index = end
            continue
        if text.startswith("/*", index):
            end = text.find("*/", index + 2)
            end = length if end < 0 else end + 2
            _mask_range(masked, index, end)
            index = end
            continue
        if char == "`":
            frames.append(["template", 0])
            index += 1
            continue
        if char in "'\"":
            index = _skip_quoted(text, index, char)
            continue
        if char == "{":
            frames[-1][1] = depth + 1
            index += 1
            continue
        if char == "}":
            if depth:
                frames[-1][1] = depth - 1
            else:
                frames.pop()
            index += 1
            continue
        index += 1
    return "".join(masked)


def is_test_module(relative: Path) -> bool:
    """Report whether a source path is test code rather than application source."""

    return (
        any(part in FRONTEND_TEST_DIR_NAMES for part in relative.parts[:-1])
        or bool(FRONTEND_TEST_FILE_RE.search(relative.name))
    )


def _discover_source_files(source_dir: Path) -> tuple[list[Path], list[Path], list[str]]:
    """Return ``(scanned, excluded_tests, errors)`` relative to the source dir."""

    errors: list[str] = []
    if not source_dir.is_dir():
        return [], [], [f"{source_dir.name} source directory is missing"]
    scanned: list[Path] = []
    excluded: list[Path] = []
    for path in sorted(source_dir.rglob("*")):
        if path.suffix not in FRONTEND_SOURCE_SUFFIXES or not path.is_file():
            continue
        relative = path.relative_to(source_dir)
        if "node_modules" in relative.parts:
            continue
        (excluded if is_test_module(relative) else scanned).append(relative)
    if not scanned:
        errors.append(f"{source_dir.name} contains no TypeScript source outside tests")
    return scanned, excluded, errors


def catalog_vocabulary(root: Path) -> tuple[frozenset[str], list[str]]:
    """Return the declared capability/state tokens and any catalog errors."""

    catalog_path = root / FRONTEND_CAPABILITY_CATALOG
    try:
        catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        return frozenset(), [f"{FRONTEND_CAPABILITY_CATALOG} cannot be read as JSON: {error}"]
    states = catalog.get("states") if isinstance(catalog, dict) else None
    if not isinstance(states, list) or not states:
        return frozenset(), [f"{FRONTEND_CAPABILITY_CATALOG} declares no capability states"]
    tokens: set[str] = set()
    for entry in states:
        if not isinstance(entry, dict):
            return frozenset(), [
                f"{FRONTEND_CAPABILITY_CATALOG} contains a non-object state entry"
            ]
        for key in ("capability", "state"):
            value = entry.get(key)
            if not isinstance(value, str) or not value.strip():
                return frozenset(), [
                    f"{FRONTEND_CAPABILITY_CATALOG} has a state entry without a {key}"
                ]
            tokens.add(value.strip().lower())
    if not tokens:
        return frozenset(), [f"{FRONTEND_CAPABILITY_CATALOG} declares an empty vocabulary"]
    return frozenset(tokens), []


def _allowlist_entries(
    source_dir: str, base: Path, allowlist: Iterable[tuple[str, str]]
) -> tuple[dict[str, str], list[str]]:
    """Validate the reviewed allowlist and fail closed on a stale entry."""

    errors: list[str] = []
    allowed: dict[str, str] = {}
    try:
        entries = list(allowlist)
    except TypeError:
        return {}, ["frontend allowlist is not an iterable of (path, justification) pairs"]
    for entry in entries:
        if not isinstance(entry, (tuple, list)) or len(entry) != 2:
            errors.append("frontend allowlist entry is not a (path, justification) pair")
            continue
        path, justification = entry
        if not isinstance(path, str) or not isinstance(justification, str):
            errors.append("frontend allowlist entry is not a (path, justification) pair of strings")
            continue
        if path in allowed:
            errors.append(f"frontend allowlist lists {source_dir}/{path} more than once")
            continue
        allowed[path] = justification
        if not (base / path).is_file():
            errors.append(
                f"frontend allowlist entry {source_dir}/{path} is stale; no such source file exists"
            )
        if not justification.strip():
            errors.append(f"frontend allowlist entry {path} has no written justification")
    return allowed, errors


def _vocabulary_spans(code: str, vocabulary: frozenset[str]) -> list[tuple[int, int]]:
    """Return the spans where a catalog-declared token is actually written.

    The exemption is deliberately narrow: a forbidden literal is vocabulary
    only when it is a fragment of a token that this file really spells out. A
    bare ``'X11'`` next to nothing is a claim even though ``x11.hotkey`` exists
    in the catalog.
    """

    spans: list[tuple[int, int]] = []
    for token in vocabulary:
        for match in re.finditer(re.escape(token), code, re.IGNORECASE):
            spans.append(match.span())
    return spans


def _is_vocabulary_fragment(span: tuple[int, int], spans: list[tuple[int, int]]) -> bool:
    return any(start <= span[0] and span[1] <= end for start, end in spans)


def validate_frontend_sources(
    root: Path,
    source_dir: str = FRONTEND_SOURCE_DIR,
    allowlist: Iterable[tuple[str, str]] = FRONTEND_ALLOWLIST,
    patterns: Iterable[tuple[str, re.Pattern[str]]] = FRONTEND_FORBIDDEN_PATTERNS,
    min_files: int = FRONTEND_MIN_SCANNED_FILES,
) -> list[str]:
    """Return deterministic violations; an empty list means validation passed."""

    root = root.resolve()
    errors: list[str] = []
    base = root / source_dir
    scanned, _excluded, discovery_errors = _discover_source_files(base)
    errors.extend(discovery_errors)

    vocabulary, catalog_errors = catalog_vocabulary(root)
    errors.extend(catalog_errors)

    if len(scanned) < min_files:
        errors.append(
            f"{source_dir} scanned only {len(scanned)} source file(s), below the "
            f"{min_files}-file coverage floor; the guard would be vacuous"
        )

    rule_list = list(patterns)
    if not rule_list:
        errors.append("frontend boundary policy declares no forbidden patterns")
    elif any(not isinstance(label, str) or not label.strip() for label, _ in rule_list):
        errors.append("frontend boundary policy has an unlabelled forbidden pattern")

    allowed, allowlist_errors = _allowlist_entries(source_dir, base, allowlist)
    errors.extend(allowlist_errors)

    for relative in scanned:
        path = base / relative
        display = f"{source_dir}/{relative.as_posix()}"
        try:
            resolved = path.resolve(strict=True)
            if not resolved.is_relative_to(base.resolve(strict=True)):
                errors.append(f"{display} escapes its source directory via symlink")
                continue
            text = resolved.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            errors.append(f"{display} cannot be read as UTF-8: {error}")
            continue

        code = mask_typescript_comments(text)
        vocabulary_spans = (
            _vocabulary_spans(code, vocabulary) if vocabulary else []
        )
        hits: list[str] = []
        for label, pattern in rule_list:
            for match in pattern.finditer(code):
                literal = match.group(0)
                # A fragment of a capability token this file really writes is
                # the daemon's own vocabulary (``trigger-x11-fallback``), not
                # a user-visible claim.
                if _is_vocabulary_fragment(match.span(), vocabulary_spans):
                    continue
                line = code.count("\n", 0, match.start()) + 1
                hits.append(f"{display}:{line} hard-codes a forbidden {label}: {literal!r}")

        if relative.as_posix() in allowed:
            if not hits:
                errors.append(
                    f"frontend allowlist entry {display} no longer needs its exemption; remove it"
                )
            continue
        errors.extend(hits)
    return sorted(set(errors))


def scanned_file_counts(root: Path, source_dir: str = FRONTEND_SOURCE_DIR) -> tuple[int, int]:
    """Return ``(scanned, excluded_tests)`` for the human-readable summary."""

    scanned, excluded, _errors = _discover_source_files((root.resolve() / source_dir))
    return len(scanned), len(excluded)


def main() -> int:
    errors = validate_frontend_sources(ROOT)
    if errors:
        print("Frontend boundary validation FAILED")
        for error in errors:
            print(f" - {error}")
        return 1
    scanned, excluded = scanned_file_counts(ROOT)
    print(
        f"Frontend boundaries OK for {FRONTEND_SOURCE_DIR}: {scanned} source file(s) "
        f"scanned, {excluded} test module(s) excluded, no OS/device/delivery literals."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
