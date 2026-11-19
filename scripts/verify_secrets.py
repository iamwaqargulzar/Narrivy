#!/usr/bin/env python3
"""Fail-closed committed-secret scan for the Narrivy repository.

Enumerates every tracked text file with ``git ls-files`` plus every
untracked-but-not-ignored worktree file, and reports any match for a
credential signature: a private-key block, a provider-prefixed token
(AWS, GitHub, HuggingFace, Slack, OpenAI-style, GitLab, npm, Stripe,
SendGrid, JSON Web Token), an ``api_key`` assignment, or a high-entropy
literal assigned to a credential-named key.

Files are allowlisted by design, never by content: dependency lockfiles
(whose entries are integrity digests) and the governance test that
deliberately carries a fake credential in order to prove it is redacted.
Findings are printed as ``file:line`` with a masked preview so the report
itself does not re-leak the value. Generic words such as "password" or
"token" are deliberately not searched: they carry no signal.

Two deliberate strictnesses: an ``api_key = "..."`` assignment is a finding
even when the value looks like a placeholder, because tracked configuration
must hold references, not literals; and the entropy heuristic only applies to
single-token values, so a multi-word passphrase is outside its reach and
relies on the provider signatures instead.

Exit status is 0 only when the whole file set was enumerated and scanned
with no finding.
"""

from __future__ import annotations

import math
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MAX_FILE_BYTES = 4 * 1024 * 1024
MIN_ENTROPY_BITS_PER_CHAR = 4.0
MIN_OPAQUE_LENGTH = 20

ALLOWED_BASENAMES = frozenset(
    {
        "Cargo.lock",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "Pipfile.lock",
        "poetry.lock",
        "go.sum",
        "composer.lock",
        "Gemfile.lock",
    }
)

ALLOWED_PATHS = {
    "tests/governance/test_run_deepseek_task.py": (
        "fixture asserts the orchestrator redacts a fake OpenAI-style "
        "credential; the literal is the subject under test, not a real key"
    ),
}

SIGNATURES: tuple[tuple[str, str, re.Pattern[str]], ...] = (
    (
        "private-key-block",
        "PEM private key block",
        re.compile(r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----"),
    ),
    (
        "aws-access-key-id",
        "AWS access key ID",
        re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    ),
    (
        "github-token",
        "GitHub personal access / OAuth token",
        re.compile(r"\bgh[pousr]_[A-Za-z0-9]{36,}"),
    ),
    (
        "gitlab-token",
        "GitLab personal access token",
        re.compile(r"\bglpat-[A-Za-z0-9_\-]{16,}"),
    ),
    (
        "huggingface-token",
        "Hugging Face access token",
        re.compile(r"\bhf_[A-Za-z0-9]{34,}"),
    ),
    (
        "npm-token",
        "npm access token",
        re.compile(r"\bnpm_[A-Za-z0-9]{30,}"),
    ),
    (
        "slack-token",
        "Slack token",
        re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{10,}"),
    ),
    (
        "slack-webhook",
        "Slack incoming webhook URL",
        re.compile(r"hooks\.slack\.com/services/[A-Za-z0-9/]{20,}"),
    ),
    (
        "openai-style-key",
        "OpenAI-style secret key",
        re.compile(r"\bsk-(?:[A-Za-z0-9]{20,}|(?:live|test|proj)-[A-Za-z0-9-]{16,})"),
    ),
    (
        "stripe-key",
        "Stripe live/test secret key",
        re.compile(r"\b[sr]k_(?:live|test)_[A-Za-z0-9]{16,}"),
    ),
    (
        "sendgrid-key",
        "SendGrid API key",
        re.compile(r"\bSG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}"),
    ),
    (
        "json-web-token",
        "JSON Web Token",
        re.compile(r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}"),
    ),
    (
        "api-key-assignment",
        "api_key assigned a literal value",
        re.compile(r"""api[_-]?key\s*[:=]\s*["'][^"'\n]{12,}["']""", re.IGNORECASE),
    ),
)

CREDENTIAL_KEY_TOKENS = frozenset(
    {
        "apikey",
        "apisecret",
        "apitoken",
        "accesskey",
        "accesskeyid",
        "accesstoken",
        "auth",
        "authorization",
        "authtoken",
        "bearer",
        "bearertoken",
        "clientsecret",
        "connectionstring",
        "cookie",
        "credential",
        "credentials",
        "dsn",
        "encryptionkey",
        "idtoken",
        "passphrase",
        "passwd",
        "password",
        "privatekey",
        "refreshtoken",
        "secret",
        "secretkey",
        "secrets",
        "sessionkey",
        "signingkey",
        "token",
        "tokens",
    }
)

QUOTED_ASSIGNMENT_RE = re.compile(
    r"(?P<key>[A-Za-z_][A-Za-z0-9_.\-]*)\s*[:=]\s*[\"'](?P<value>[^\"'\n]{12,})[\"']"
)
BARE_ASSIGNMENT_RE = re.compile(
    r"(?P<key>[A-Za-z_][A-Za-z0-9_.\-]*)[ \t]*=[ \t]*"
    r"(?P<value>[A-Za-z0-9+/=_^~*%!\-]{16,})[ \t]*(?:#.*)?$",
    re.MULTILINE,
)
SPLIT_RE = re.compile(r"[^a-z0-9]+")
PLACEHOLDER_RE = re.compile(
    r"[<>{}$`]|redact|placeholder|changeme|change[-_]me|example|dummy|fake|sample"
    r"|notreal|your[-_]|xxxx|\.\.\.|todo|fixme|replace[-_]?me|insert[-_]?here",
    re.IGNORECASE,
)


def _git(*args: str) -> list[str]:
    result = subprocess.run(
        ["git", *args, "-z"], cwd=ROOT, capture_output=True, text=True
    )
    if result.returncode:
        raise SystemExit(
            f"verify_secrets: git {' '.join(args)} failed: {result.stderr.strip()}"
        )
    return [entry for entry in result.stdout.split("\0") if entry]


def candidate_files() -> list[str]:
    """Every text file that could carry a committed secret, git-ignored excluded."""

    return sorted(
        set(_git("ls-files")) | set(_git("ls-files", "--others", "--exclude-standard"))
    )


def read_text(relative: str) -> str | None:
    path = ROOT / relative
    try:
        if path.is_symlink() and not path.exists():
            return None
        if not path.is_file() or path.stat().st_size > MAX_FILE_BYTES:
            return None
        raw = path.read_bytes()
    except OSError as error:
        raise SystemExit(f"verify_secrets: cannot read {relative}: {error}")
    if b"\0" in raw[:8192]:
        return None
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return None


def shannon_entropy(value: str) -> float:
    counts = Counter(value)
    total = len(value)
    return -sum((n / total) * math.log2(n / total) for n in counts.values())


def credential_key(key: str) -> bool:
    return any(part in CREDENTIAL_KEY_TOKENS for part in SPLIT_RE.split(key.lower()))


def is_placeholder(value: str) -> bool:
    return bool(PLACEHOLDER_RE.search(value))


def looks_opaque(value: str) -> bool:
    """A long, high-entropy, single-token literal of the kind a key actually is.

    A real credential is machine-generated: no whitespace, at least three
    character classes, and near-uniform byte distribution. Requiring all three
    keeps prose, marker strings, and design-token values out of the results.
    """

    if len(value) < MIN_OPAQUE_LENGTH or is_placeholder(value):
        return False
    if any(char.isspace() for char in value):
        return False
    classes = sum(
        (
            any(c.islower() for c in value),
            any(c.isupper() for c in value),
            any(c.isdigit() for c in value),
            any(not c.isalnum() for c in value),
        )
    )
    return classes >= 3 and shannon_entropy(value) >= MIN_ENTROPY_BITS_PER_CHAR


def mask(value: str) -> str:
    head = value[:4]
    return f"{head}...<{len(value)} chars>"


def scan_text(text: str) -> list[tuple[int, str, str, str]]:
    """Return ``(line, rule, label, preview)`` findings for one file."""

    findings: list[tuple[int, str, str, str]] = []
    seen: set[tuple[int, str, str]] = set()
    lines = text.splitlines()
    offsets: list[int] = []
    cursor = 0
    for line in lines:
        offsets.append(cursor)
        cursor += len(line) + 1

    def line_of(position: int) -> int:
        low, high = 0, len(offsets) - 1
        while low < high:
            mid = (low + high + 1) // 2
            if offsets[mid] <= position:
                low = mid
            else:
                high = mid - 1
        return low + 1

    for rule, label, pattern in SIGNATURES:
        for match in pattern.finditer(text):
            key = (line_of(match.start()), rule, match.group(0))
            if key in seen:
                continue
            seen.add(key)
            findings.append((key[0], rule, label, mask(match.group(0))))

    for pattern in (QUOTED_ASSIGNMENT_RE, BARE_ASSIGNMENT_RE):
        for match in pattern.finditer(text):
            value = match.group("value")
            if not credential_key(match.group("key")) or not looks_opaque(value):
                continue
            rule = "opaque-credential-value"
            label = "high-entropy value assigned to a credential-named key"
            key = (line_of(match.start()), rule, match.group("key"))
            if key in seen:
                continue
            seen.add(key)
            findings.append((key[0], rule, label, f"{match.group('key')}={mask(value)}"))
    return findings


def is_allowed(relative: str) -> str | None:
    name = Path(relative).name
    if name in ALLOWED_BASENAMES:
        return f"lockfile {name} stores integrity digests, not credentials"
    parts = Path(relative).parts
    if "node_modules" in parts or ".git" in parts:
        return "third-party or VCS internals are not committed source"
    return ALLOWED_PATHS.get(relative)


def main() -> int:
    if sys.argv != [sys.argv[0]]:
        print("verify_secrets: this gate takes no arguments", file=sys.stderr)
        return 2
    files = candidate_files()
    scanned = 0
    skipped = 0
    findings: list[tuple[str, int, str, str, str]] = []
    allowlist_used: dict[str, int] = {}
    for relative in files:
        reason = is_allowed(relative)
        if reason is not None:
            skipped += 1
            allowlist_used.setdefault(f"{relative}: {reason}", 0)
            allowlist_used[f"{relative}: {reason}"] += 1
            continue
        text = read_text(relative)
        if text is None:
            skipped += 1
            continue
        scanned += 1
        for line, rule, label, preview in scan_text(text):
            findings.append((relative, line, rule, label, preview))

    if findings:
        print(f"Secrets check FAILED: {len(findings)} finding(s)")
        print(
            "A committed credential violates PRIV-005. Revoke the credential, then "
            "purge it from history; do not allowlist it."
        )
        for relative, line, rule, label, preview in sorted(findings):
            print(f" - {relative}:{line} [{rule}] {label}: {preview}")
        return 1
    print(
        f"Secrets check OK: {scanned} text files scanned, {skipped} allowlisted or "
        f"non-text, no credential signatures found."
    )
    for entry in sorted(allowlist_used):
        print(f"   allowlisted {entry}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
