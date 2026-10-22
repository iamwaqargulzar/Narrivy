#!/usr/bin/env python3
"""Fail-closed static gate for the AGENTS.md rule that ordinary logs and
diagnostics never carry transcript text, audio, clipboard/selection/context
content, tokens, secrets, or environment dumps.

Every first-party Rust source under crates/, engines/, adapters/, services/, and
apps/ is scanned for diagnostic emissions - the tracing/log facade macros,
eprintln!/eprint!/dbg!, and println!/print! outside apps/ - that can carry a
content-bearing identifier. An emission passes only when the sensitive value is
wrapped in the project's own redaction facility (narrivy-diagnostics
Redactor::scrub_string, Redactor::redact_json, SupportReport::to_json_pretty).
That facility is re-verified on every run, so the exemption cannot become
vacuous, and a whole-process environment dump is always a finding.

Excluded from scope: test code, cargo build-script protocol lines, and
generated trees. Client stdout under apps/ is the user-requested product
output channel, not an ordinary log. Known limitation: this is a lexical
check, so an emission that Debug-formats an identifier with no sensitive name
(for example a derived Debug of a transcript-carrying struct) is not detected.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_ROOTS = ("crates", "engines", "adapters", "services", "apps")
GENERATED_DIRS = frozenset({"target", "node_modules", "dist", ".vite", "__pycache__"})
TEST_DIRS = frozenset({"tests", "benches", "examples"})

FACADE_MACROS = frozenset({"trace", "debug", "info", "warn", "error", "event", "span"})
STDERR_MACROS = frozenset({"eprintln", "eprint", "dbg"})
STDOUT_MACROS = frozenset({"println", "print"})
EMISSION_MACROS = FACADE_MACROS | STDERR_MACROS | STDOUT_MACROS

CONTENT_KEYS = frozenset(
    {
        "transcript",
        "tentative_transcript",
        "raw_text",
        "final_text",
        "last_delivered",
        "text",
        "spoken",
        "words",
        "tokens",
        "audio",
        "samples",
        "clipboard",
        "selection",
        "context",
    }
)
CREDENTIAL_KEYS = frozenset(
    {
        "token",
        "secret",
        "password",
        "passphrase",
        "credential",
        "cookie",
        "authorization",
        "api_key",
        "apikey",
        "access_key",
        "private_key",
    }
)
ENVIRONMENT_KEYS = frozenset({"env", "environ", "environment", "envvars"})
SENSITIVE_KEYS = CONTENT_KEYS | CREDENTIAL_KEYS | ENVIRONMENT_KEYS

REDACTION_CALLS = ("scrub_string", "redact_json", "scrub_kv_pairs", "to_json_pretty", "redact")
REDACTION_RE = re.compile(
    r"\b(?:" + "|".join(REDACTION_CALLS) + r")\s*(?:::<[^>]*>\s*)?\("
)
ENV_DUMP_RE = re.compile(r"\benv::(?:vars|vars_os|iter)\s*\(")
INLINE_CAPTURE_RE = re.compile(r"\{([A-Za-z_][A-Za-z0-9_]*)\s*[:}]")
FORMAT_ARGUMENT_RE = re.compile(r"\{[0-9]*\s*[:}]")
IDENTIFIER_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
SPLIT_RE = re.compile(r"[^a-z0-9]+")
TEST_ATTR_RE = re.compile(
    r"#\s*\[\s*(?:[A-Za-z_][A-Za-z0-9_]*::\s*)?test\s*\]"
)
TEST_CFG_RE = re.compile(r"#\s*\[\s*cfg\s*\([^)]*\btest\b[^)]*\)\s*\]")
TEST_MOD_RE = re.compile(r"\bmod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{")
REQUIRED_DENIED_KEYS = (
    "transcript",
    "text",
    "audio",
    "samples",
    "clipboard",
    "token",
    "secret",
    "password",
    "authorization",
    "cookie",
)


def _blank(buffer: list[str], start: int, end: int) -> None:
    for index in range(max(start, 0), min(end, len(buffer))):
        if buffer[index] not in "\r\n":
            buffer[index] = " "


def _raw_string_open(text: str, index: int) -> tuple[int, int] | None:
    """Return ``(opening_length, hash_count)`` for a Rust raw/byte raw string."""

    if index and (text[index - 1].isalnum() or text[index - 1] == "_"):
        return None
    prefix = 2 if text.startswith(("br", "cr"), index) else 1 if text[index] == "r" else 0
    if not prefix:
        return None
    cursor = index + prefix
    while cursor < len(text) and text[cursor] == "#":
        cursor += 1
    if cursor >= len(text) or text[cursor] != '"':
        return None
    return cursor - index + 1, cursor - index - prefix


def mask_rust(text: str, *, literals: bool, tests: bool) -> str:
    """Mask Rust source in place, preserving every offset and newline.

    Comments are always removed. With ``literals`` the body of every
    string/char literal is removed too, which is what structural scanning
    needs; without it the format string stays readable so inline
    ``{capture}`` names can be resolved. With ``tests`` the body of
    ``#[cfg(test)]`` items and attributed test functions is removed, because
    test fixtures legitimately carry transcript-shaped strings.

    Block comments nest and raw strings may carry hash delimiters, so the scan
    is a single left-to-right pass. Rust lifetimes such as ``'a`` are not
    char literals and must stay visible as code.
    """

    masked = list(text)
    length = len(text)
    index = 0
    while index < length:
        if text.startswith("//", index):
            end = text.find("\n", index + 2)
            end = length if end < 0 else end
            _blank(masked, index, end)
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
            _blank(masked, index, cursor)
            index = cursor
            continue
        raw = _raw_string_open(text, index)
        if raw is not None and literals:
            opening_length, hashes = raw
            closing = '"' + "#" * hashes
            end = text.find(closing, index + opening_length)
            end = length if end < 0 else end + len(closing)
            _blank(masked, index, end)
            index = end
            continue
        prefix = -1
        if text[index] == '"':
            prefix = 0
        elif text.startswith('b"', index) or text.startswith("b'", index):
            prefix = 1
        if prefix >= 0:
            quote = index + prefix
            cursor = quote + 1
            escaped = False
            while cursor < length:
                char = text[cursor]
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == text[quote]:
                    cursor += 1
                    break
                cursor += 1
            if literals:
                _blank(masked, index, cursor)
            index = cursor
            continue
        if text[index] == "'":
            quote = index
        else:
            quote = -1
        if quote >= 0:
            cursor = index + 1
            if cursor < length and text[cursor] == "\\":
                while cursor < length and text[cursor] not in "\r\n":
                    if text[cursor] == "'" and text[cursor - 1] != "\\":
                        cursor += 1
                        break
                    cursor += 1
            elif cursor + 1 < length and text[cursor + 1] == "'":
                cursor += 2
            else:
                cursor = -1
            if cursor > 0:
                if literals:
                    _blank(masked, index, cursor)
                index = cursor
                continue
        index += 1
    if tests:
        return _mask_test_items("".join(masked))
    return "".join(masked)


def _balanced_end(code: str, start: int) -> int:
    """Index just past the ``}`` matching the ``{`` at or after ``start``."""

    opening = code.find("{", start)
    if opening < 0:
        return -1
    depth = 0
    for index in range(opening, len(code)):
        char = code[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1
    return -1


def _next_item_end(code: str, anchor_end: int) -> int:
    cursor = anchor_end
    while cursor < len(code):
        char = code[cursor]
        if char in " \t\r\n":
            cursor += 1
        elif code.startswith("#[", cursor):
            close = code.find("]", cursor)
            cursor = len(code) if close < 0 else close + 1
        elif code.startswith("//", cursor) or code.startswith("/*", cursor):
            return -1
        elif IDENTIFIER_RE.match(code, cursor):
            end = _balanced_end(code, cursor)
            if end < 0:
                return -1
            word = IDENTIFIER_RE.match(code, cursor).group(0)
            brace = code.find("{", cursor)
            if word in {"fn", "unsafe", "const", "async", "extern"} and brace < 0:
                semicolon = code.find(";", cursor)
                return -1 if semicolon < 0 else semicolon + 1
            return end
        else:
            return -1
    return -1


def _mask_test_items(code: str) -> str:
    """Blank out ``#[cfg(test)]`` items and attributed test functions."""

    buffer = list(code)
    patterns = (TEST_ATTR_RE, TEST_CFG_RE, TEST_MOD_RE)
    position = 0
    while position < len(code):
        best: tuple[int, int] | None = None
        for pattern in patterns:
            match = pattern.search(code, position)
            if match and (best is None or match.start() < best[0]):
                best = (match.start(), match.end())
        if best is None:
            break
        end = _next_item_end(code, best[1])
        if end < 0:
            position = best[1]
            continue
        _blank(buffer, best[0], end)
        position = end
    return "".join(buffer)


def _call_args(code: str, open_paren: int) -> tuple[int, int] | None:
    """Half-open span of the argument text for a macro call body."""

    depth = 0
    for index in range(open_paren, len(code)):
        char = code[index]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
            if depth == 0:
                return open_paren + 1, index
    return None


def _redaction_spans(structure: str, start: int, end: int) -> list[tuple[int, int]]:
    spans: list[tuple[int, int]] = []
    for match in REDACTION_RE.finditer(structure, start, end):
        args = _call_args(structure, match.end() - 1)
        if args:
            spans.append(args)
    return spans


def _sensitive_key(identifier: str) -> str | None:
    lowered = identifier.lower()
    if lowered in SENSITIVE_KEYS:
        return lowered
    for part in SPLIT_RE.split(lowered):
        if part in SENSITIVE_KEYS:
            return part
    return None


def _is_product_stdout(relative: Path) -> bool:
    """Whether stdout in this file is the product's own output channel.

    Only the command-line client's stdout qualifies. It writes to the terminal
    the user asked it to write to, and nothing in packaging redirects it: the
    systemd unit's `ExecStart` is the daemon, the desktop entry launches the GUI
    shell, and the one shortcut the desktop surfaces is `narrivy-cli toggle`,
    which is content-free.

    A GUI shell's stdout is *not* a product channel. It is inherited from the
    desktop session, so it lands in the journal, and exempting it would let a
    transcript `println!` in `apps/narrivy-desktop` pass this gate unnoticed.
    """
    return relative.parts[:2] == ("apps", "narrivy-cli")


def _rust_sources() -> list[Path]:
    sources: list[Path] = []
    for name in SOURCE_ROOTS:
        base = ROOT / name
        if not base.is_dir():
            raise SystemExit(f"check_log_redaction: required source root is missing: {name}")
        for path in base.rglob("*.rs"):
            parts = path.relative_to(ROOT).parts
            if any(part in GENERATED_DIRS or part in TEST_DIRS for part in parts):
                continue
            sources.append(path)
    return sorted(sources)


def scan_source(path: Path) -> list[str]:
    """Return violations for one Rust file; an empty list means it is clean."""

    relative = path.relative_to(ROOT)
    text = path.read_text(encoding="utf-8", errors="ignore")
    structure = mask_rust(text, literals=True, tests=True)
    with_literals = mask_rust(text, literals=False, tests=True)
    findings: list[str] = []
    seen: set[tuple[int, str, str]] = set()

    for match in IDENTIFIER_RE.finditer(structure):
        macro = match.group(0)
        if macro not in EMISSION_MACROS:
            continue
        if match.end() >= len(structure) or structure[match.end()] != "!":
            continue
        if macro in STDOUT_MACROS and _is_product_stdout(relative):
            continue
        cursor = match.end() + 1
        while cursor < len(structure) and structure[cursor] in " \t\r\n":
            cursor += 1
        if cursor >= len(structure) or structure[cursor] not in "({":
            continue
        args = _call_args(structure, cursor)
        if args is None:
            continue
        start, end = args
        body = structure[start:end]
        if _is_cargo_directive(with_literals[start:end]):
            continue
        line = structure.count("\n", 0, match.start()) + 1
        safe = _redaction_spans(structure, start, end)
        for dump in ENV_DUMP_RE.finditer(body):
            findings.append(
                f"{relative}:{line} {macro}! emits a whole-process environment dump "
                f"({body[dump.start():dump.end()].strip()}); "
                "pass only explicitly chosen values"
            )
        for identifier in IDENTIFIER_RE.finditer(body):
            offset = start + identifier.start()
            name = identifier.group(0)
            if any(low <= offset < high for low, high in safe):
                continue
            key = _sensitive_key(name)
            if key is None or (line, macro, name) in seen:
                continue
            seen.add((line, macro, name))
            findings.append(_leak_finding(relative, line, macro, name, key))
        for capture in INLINE_CAPTURE_RE.finditer(with_literals[start:end]):
            offset = start + capture.start(1)
            name = capture.group(1)
            if any(low <= offset < high for low, high in safe):
                continue
            key = _sensitive_key(name)
            if key is None or (line, macro, name) in seen:
                continue
            seen.add((line, macro, name))
            findings.append(_leak_finding(relative, line, macro, name, key))
        literal = _first_literal_span(with_literals, start, end)
        if literal is None:
            continue
        prose_words = {
            word: key
            for word in IDENTIFIER_RE.findall(with_literals, *literal)
            if (key := _sensitive_key(word))
        }
        if not prose_words or not (
            FORMAT_ARGUMENT_RE.search(with_literals[start:end])
            or INLINE_CAPTURE_RE.search(with_literals[start:end])
        ):
            continue
        unredacted = _unredacted_value(structure, literal[1], end, safe)
        if unredacted is None:
            continue
        word, key = sorted(prose_words.items())[0]
        mark = (line, macro, word)
        if mark in seen:
            continue
        seen.add(mark)
        findings.append(
            f"{relative}:{line} {macro}! can emit sensitive content "
            f"({macro}! describes '{word}' and interpolates unredacted "
            f"'{unredacted}'); route the value through "
            f"narrivy_diagnostics::Redactor or report only a length"
        )
    return findings


def _leak_finding(relative: Path, line: int, macro: str, name: str, key: str) -> str:
    return (
        f"{relative}:{line} {macro}! can emit sensitive content "
        f"({name} matches '{key}'); route it through "
        f"narrivy_diagnostics::Redactor ({macro}! is a diagnostic log sink)"
    )


def _is_cargo_directive(body: str) -> bool:
    stripped = body.lstrip()
    return stripped.startswith(('"cargo:', 'r"cargo:', 'r#"cargo:', "r##\"cargo:"))


def _first_literal_span(text: str, start: int, end: int) -> tuple[int, int] | None:
    """Span of the format string that opens a macro argument list."""

    index = start
    while index < end:
        char = text[index]
        if char in " \t\r\n(),=;":
            index += 1
            continue
        raw = _raw_string_open(text, index)
        if raw is not None:
            opening_length, hashes = raw
            closing = '"' + "#" * hashes
            stop = text.find(closing, index + opening_length)
            return index, end if stop < 0 else min(stop + len(closing), end)
        if char == '"':
            cursor = index + 1
            while cursor < end:
                if text[cursor] == "\\":
                    cursor += 2
                    continue
                if text[cursor] == '"':
                    return index, cursor + 1
                cursor += 1
            return index, end
        return None
    return None


def _unredacted_value(
    structure: str, start: int, end: int, safe: list[tuple[int, int]]
) -> str | None:
    """First interpolated expression in the argument list that was not redacted.

    Callee names are not values, so ``r.scrub_string(t)`` is entirely accounted
    for by the redaction span around ``t`` and passes.
    """

    for identifier in IDENTIFIER_RE.finditer(structure, start, end):
        offset = identifier.start()
        if any(low <= offset < high for low, high in safe):
            continue
        after = structure[identifier.end() :].lstrip()
        if after[:1] in {"(", "."}:
            continue
        if structure[:offset].rstrip().endswith("."):
            continue
        return identifier.group(0)
    return None


def check_redaction_facility() -> list[str]:
    """Verify the exemption target still exists and is the real implementation."""

    errors: list[str] = []
    redactor = ROOT / "crates/narrivy-diagnostics/src/redactor.rs"
    support = ROOT / "crates/narrivy-diagnostics/src/support.rs"
    lib = ROOT / "crates/narrivy-diagnostics/src/lib.rs"
    for path in (redactor, support, lib):
        if not path.is_file():
            errors.append(f"redaction facility source is missing: {path.relative_to(ROOT)}")
    if errors:
        return errors
    redactor_src = redactor.read_text(encoding="utf-8")
    support_src = support.read_text(encoding="utf-8")
    lib_src = lib.read_text(encoding="utf-8")
    for needle in ("pub struct Redactor", "pub fn scrub_string", "pub fn redact_json"):
        if needle not in redactor_src:
            errors.append(
                f"redaction facility no longer provides {needle!r} in "
                f"{redactor.relative_to(ROOT)}"
            )
    exports = "\n".join(
        line for line in lib_src.splitlines() if line.strip().startswith("pub use")
    )
    for name in ("Redactor", "SupportReport"):
        if not re.search(rf"\b{name}\b", exports):
            errors.append(f"{lib.relative_to(ROOT)} no longer re-exports {name}")
    pretty = re.search(r"pub fn to_json_pretty.*?\n    \}", support_src, re.S)
    if pretty is None:
        errors.append(f"{support.relative_to(ROOT)} no longer provides to_json_pretty")
    elif "redact_json" not in pretty.group(0):
        errors.append(
            f"SupportReport::to_json_pretty in {support.relative_to(ROOT)} no longer "
            "applies Redactor::redact_json; unredacted reports would reach users"
        )
    declared = re.search(r"DEFAULT_DENIED_KEYS[^=]*=\s*\[(.*?)\];", redactor_src, re.S)
    if declared is None:
        errors.append(
            f"DEFAULT_DENIED_KEYS is no longer declared in {redactor.relative_to(ROOT)}"
        )
    else:
        keys = {m.group(1).lower() for m in re.finditer(r'"([^"]+)"', declared.group(1))}
        for required in REQUIRED_DENIED_KEYS:
            if required not in keys:
                errors.append(
                    f"Redactor::DEFAULT_DENIED_KEYS in {redactor.relative_to(ROOT)} "
                    f"no longer denies '{required}'"
                )
    return errors


def main() -> int:
    if sys.argv != [sys.argv[0]]:
        print("check_log_redaction: this gate takes no arguments", file=sys.stderr)
        return 2
    findings = check_redaction_facility()
    sources = _rust_sources()
    for path in sources:
        findings.extend(scan_source(path))
    if findings:
        print("Log redaction check FAILED")
        print(
            "Ordinary logs and diagnostics must never carry transcript, audio,"
        )
        print(
            "clipboard/selection, token, secret, or environment content. Redact"
        )
        print("with narrivy_diagnostics::Redactor or build a SupportReport.")
        for finding in sorted(set(findings)):
            print(f" - {finding}")
        return 1
    print(
        f"Log redaction OK: {len(sources)} Rust sources scanned, no content-bearing "
        "diagnostic emission outside the narrivy-diagnostics redaction facility."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
