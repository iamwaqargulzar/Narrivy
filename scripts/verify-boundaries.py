#!/usr/bin/env python3
"""Fail-closed source boundary validation for portable Narrivy crates."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Iterable

try:
    from boundary_policy import PROTECTED_PACKAGES, SOURCE_PATTERNS, protected_package_dir
except ModuleNotFoundError:  # Imported by the fixture suite.
    from scripts.boundary_policy import (
        PROTECTED_PACKAGES,
        SOURCE_PATTERNS,
        protected_package_dir,
    )


ROOT = Path(__file__).resolve().parents[1]


def _mask_range(buffer: list[str], start: int, end: int) -> None:
    for index in range(start, end):
        if buffer[index] not in "\r\n":
            buffer[index] = " "


def _raw_string_open(text: str, index: int) -> tuple[int, int] | None:
    """Return ``(opening_length, hash_count)`` for a Rust raw string."""

    if index and (text[index - 1].isalnum() or text[index - 1] == "_"):
        return None
    if text.startswith(("br", "cr"), index):
        prefix_length = 2
    elif text.startswith("r", index):
        prefix_length = 1
    else:
        return None
    cursor = index + prefix_length
    while cursor < len(text) and text[cursor] == "#":
        cursor += 1
    if cursor >= len(text) or text[cursor] != '"':
        return None
    return cursor - index + 1, cursor - index - prefix_length


def mask_rust_non_code(text: str) -> str:
    """Mask Rust comments and literals while preserving offsets/newlines.

    Rust block comments nest and raw strings may have hash delimiters. ABI
    strings following ``extern`` remain visible because they are syntax, not
    application content, and are forbidden in portable source.
    """

    masked = list(text)
    length = len(text)
    index = 0
    while index < length:
        if text.startswith("//", index):
            end = text.find("\n", index + 2)
            if end < 0:
                end = length
            _mask_range(masked, index, end)
            index = end
            continue

        if text.startswith("/*", index):
            depth = 1
            cursor = index + 2
            while cursor < length and depth:
                if text.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif text.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            _mask_range(masked, index, cursor)
            index = cursor
            continue

        raw_open = _raw_string_open(text, index)
        if raw_open is not None:
            opening_length, hashes = raw_open
            closing = '"' + ("#" * hashes)
            end = text.find(closing, index + opening_length)
            end = length if end < 0 else end + len(closing)
            _mask_range(masked, index, end)
            index = end
            continue

        if text[index] == '"':
            string_prefix = 0
        elif index + 1 < length and text[index] in "bc" and text[index + 1] == '"':
            string_prefix = 1
        else:
            string_prefix = -1
        if string_prefix >= 0:
            quote = index + string_prefix
            cursor = quote + 1
            escaped = False
            while cursor < length:
                char = text[cursor]
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    cursor += 1
                    break
                cursor += 1
            prefix = "".join(masked[max(0, index - 48) : index])
            is_abi = string_prefix == 0 and re.search(
                r"\b(?:unsafe\s+)?extern\s*$", prefix
            )
            if not is_abi:
                _mask_range(masked, index, cursor)
            index = cursor
            continue

        if text[index] == "'":
            char_quote = index
        elif index + 1 < length and text[index] == "b" and text[index + 1] == "'":
            char_quote = index + 1
        else:
            char_quote = -1
        if char_quote >= 0:
            closing = -1
            content = char_quote + 1
            if content < length and text[content] == "\\":
                cursor = content + 1
                while cursor < length and text[cursor] not in "\r\n":
                    if text[cursor] == "'" and text[cursor - 1] != "\\":
                        closing = cursor + 1
                        break
                    cursor += 1
            elif content + 1 < length and text[content + 1] == "'":
                closing = content + 2
            # Lifetimes (`'a`, `'static`) do not have an immediate closing
            # quote and must remain code; a Rust character contains one scalar
            # or one escaped scalar followed by its closing quote.
            if closing >= 0:
                _mask_range(masked, index, closing)
                index = closing
                continue

        index += 1
    return "".join(masked)


def _production_sources(base: Path) -> tuple[list[Path], list[str]]:
    errors: list[str] = []
    manifest = base / "Cargo.toml"
    source_dir = base / "src"
    if not manifest.is_file():
        errors.append(f"{manifest} is missing")
    if not source_dir.is_dir():
        errors.append(f"{source_dir} is missing")
        return [], errors

    paths = sorted(source_dir.rglob("*.rs"))
    build_script = base / "build.rs"
    if build_script.exists():
        if build_script.is_file():
            paths.append(build_script)
        else:
            errors.append(f"{build_script} is not a regular file")
    if not paths:
        errors.append(f"{source_dir} contains no Rust production source")
    return paths, errors


def validate_source_boundaries(
    root: Path, protected_packages: Iterable[str] = PROTECTED_PACKAGES
) -> list[str]:
    """Return deterministic violations; an empty list means validation passed."""

    root = root.resolve()
    errors: list[str] = []
    for package in protected_packages:
        base = protected_package_dir(root, package)
        paths, discovery_errors = _production_sources(base)
        errors.extend(discovery_errors)
        for path in paths:
            try:
                resolved = path.resolve(strict=True)
                if not resolved.is_relative_to(base.resolve(strict=True)):
                    errors.append(f"{path.relative_to(root)} escapes its package via symlink")
                    continue
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeError) as error:
                try:
                    display = path.relative_to(root)
                except ValueError:
                    display = path
                errors.append(f"{display} cannot be read as UTF-8: {error}")
                continue

            code = mask_rust_non_code(text)
            for label, pattern in SOURCE_PATTERNS:
                for match in pattern.finditer(code):
                    line = code.count("\n", 0, match.start()) + 1
                    errors.append(
                        f"{path.relative_to(root)}:{line} references forbidden {label}"
                    )
    return sorted(set(errors))


def main() -> int:
    errors = validate_source_boundaries(ROOT)
    if errors:
        print("Source boundary validation FAILED")
        for error in errors:
            print(f" - {error}")
        return 1
    print(f"Source boundaries OK for {len(PROTECTED_PACKAGES)} portable/core packages.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
