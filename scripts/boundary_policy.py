#!/usr/bin/env python3
"""Shared policy for Narrivy's portable-crate boundary validators."""

from __future__ import annotations

import re
from pathlib import Path


PROTECTED_PACKAGES: tuple[str, ...] = (
    "narrivy-types",
    "narrivy-platform-api",
    "narrivy-asr-api",
    "narrivy-policy",
    "narrivy-pipeline",
    "narrivy-ipc",
    "narrivy-journal-api",
    "narrivy-diagnostics",
    "narrivy-compute",
    "narrivy-core",
    "narrivy-model-manager",
    "narrivy-history",
)

# A workspace member with no production reverse dependency is either dead code
# or a missing composition, and both are release-relevant: an orphan can ship
# with its own passing ``cargo test`` while nothing in the product ever calls
# it. Entry points (a package that declares a ``bin`` target) are exempt because
# the release graph reaches them by packaging, not by a Cargo edge.
#
# A library leaf that is genuinely never depended upon must be named here with
# a written justification that survives review. A stale or unjustified entry is
# itself a failure, so the exemption can never quietly become a blanket.
LIBRARY_LEAF_ALLOWLIST: dict[str, str] = {
    "narrivy-platform-null": (
        "Null implementation of the narrivy-platform-api contract. The crate "
        "documents itself as the portable integration-test backend, and its only "
        "Cargo edge is the narrivy-core dev-dependency used by the contract "
        "conformance tests. It is a contract-conformance reference, not a "
        "production composition edge."
    ),
}

# These are implementation technologies, not portable contract dependencies.
# Matching is by exact normalized package name or a hyphen-delimited family
# suffix, never an arbitrary substring.
FORBIDDEN_IMPLEMENTATION_FAMILIES: tuple[str, ...] = (
    "ashpd",
    "cocoa",
    "core-foundation",
    "coreaudio",
    "cuda",
    "cudarc",
    "dbus",
    "directml",
    "evdev",
    "fcitx",
    "ibus",
    "libc",
    "libei",
    "libspa",
    "nix",
    "objc",
    "objc2",
    "onnx-runtime",
    "onnxruntime",
    "openvino",
    "ort",
    "pipewire",
    "reis",
    "rustix",
    "tauri",
    "wasapi",
    "wayland",
    "winapi",
    "windows",
    "wireplumber",
    "x11",
    "x11rb",
    "xcb",
    "xkbcommon",
    "zbus",
)

# A normal/build dependency from a portable crate into one of these workspace
# layers reverses the canonical dependency direction. Dev dependencies are
# intentionally handled separately by the dependency validator.
FORBIDDEN_WORKSPACE_LAYERS: frozenset[str] = frozenset(
    {
        "adapters",
        "apps",
        "delivery",
        "engines",
        "modules",
        "plugins",
        "processors",
        "services",
    }
)


def normalize_package_name(name: str) -> str:
    """Return Cargo's comparison form for a package identity."""

    return name.strip().lower().replace("_", "-")


def forbidden_implementation_family(name: str) -> str | None:
    """Return the forbidden family matching an actual Cargo package name."""

    normalized = normalize_package_name(name)
    for family in FORBIDDEN_IMPLEMENTATION_FAMILIES:
        if normalized == family or normalized.startswith(f"{family}-"):
            return family
    return None


SOURCE_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("standard-library OS module", re.compile(r"\bstd\s*::\s*os\s*::")),
    ("Unix socket type", re.compile(r"\b(?:UnixStream|UnixListener|UnixDatagram)\b")),
    (
        "platform file-descriptor/filesystem extension",
        re.compile(
            r"\b(?:AsRawFd|IntoRawFd|FromRawFd|OwnedFd|BorrowedFd|RawFd|"
            r"PermissionsExt|OpenOptionsExt|FileTypeExt|MetadataExt)\b"
        ),
    ),
    ("libc FFI", re.compile(r"\blibc\s*::")),
    (
        "native extern ABI",
        re.compile(r'\b(?:unsafe\s+)?extern\s*"(?:C|C-unwind|system)"'),
    ),
    (
        "platform/framework/runtime implementation",
        re.compile(
            r"\b(?:ashpd|cocoa|core_foundation|coreaudio|cuda|cudarc|dbus|"
            r"directml|evdev|fcitx|ibus|libei|libspa|nix|objc2?|onnx_runtime|"
            r"onnxruntime|openvino|ort|pipewire|reis|rustix|tauri|wasapi|"
            r"wayland|winapi|windows|windows_sys|wireplumber|x11|x11rb|xcb|"
            r"xkbcommon|zbus)\s*(?:::|_)"
        ),
    ),
)


def protected_package_dir(root: Path, package: str) -> Path:
    """Map a protected Cargo package identity to its canonical directory."""

    return root / "crates" / package


# ---------------------------------------------------------------------------
# Frontend source policy
# ---------------------------------------------------------------------------
#
# The frontend renders capabilities reported by the daemon. It must never name
# an OS, a device, a vendor runtime, or a delivery mechanism as if it had
# probed for one: an OS/hardware name in user-visible text is a claim the
# daemon never made, and OS-name branching in the UI re-creates exactly the
# feature detection that the capability contract replaced.

FRONTEND_SOURCE_DIR: str = "apps/narrivy-desktop/src"
FRONTEND_SOURCE_SUFFIXES: frozenset[str] = frozenset({".ts", ".tsx"})

# A test that enumerates the forbidden literals is how the denylist stays
# honest, so test code is out of scope for the scan itself. Test directories
# and ``*.test``/``*.spec`` modules are excluded; a separate coverage floor in
# the validator keeps the exclusion from becoming a total bypass.
FRONTEND_TEST_DIR_NAMES: frozenset[str] = frozenset({"__tests__", "test", "tests"})
FRONTEND_TEST_FILE_RE: re.Pattern[str] = re.compile(r"\.(?:test|spec)\.[cm]?[jt]sx?$")

# The single legitimate source for capability tokens. A forbidden literal that
# is a fragment of a catalog-declared capability or state token is vocabulary,
# not a claim: ``trigger-x11-fallback`` and ``x11.hotkey`` are the daemon's
# own identifiers, and the daemon chooses them.
FRONTEND_CAPABILITY_CATALOG: str = "spec/catalogs/capability-states.json"

FRONTEND_FORBIDDEN_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    (
        "accelerator vendor or model name",
        re.compile(
            r"\bNVIDIA\b|\bIntel\s+(?:NPU|Arc|iGPU)\b|\bAMD\s+(?:Ryzen|Radeon)\b|"
            r"\bRadeon\b|\bRTX\s*\d{3,4}\b|\bGTX\s*\d{3,4}\b|"
            r"\bM[1-9]\s+(?:Max|Ultra|Pro)\b|\bApple\s+Silicon\b",
            re.IGNORECASE,
        ),
    ),
    (
        "engine runtime or model name",
        re.compile(r"\bOpenVINO\b|\bWhisper\b", re.IGNORECASE),
    ),
    (
        "virtual-input delivery method",
        re.compile(r"\buinput\b|\bydotool\b", re.IGNORECASE),
    ),
    (
        "portal/libei delivery method",
        re.compile(r"\blibei\b|\bPortal\s*/\s*libei\b", re.IGNORECASE),
    ),
    (
        "X11 delivery method",
        re.compile(r"\bXWayland\b|\bX11\b", re.IGNORECASE),
    ),
    (
        "Wayland virtual keyboard",
        re.compile(r"\bWayland\s+Virtual\s+Keyboard\b", re.IGNORECASE),
    ),
    (
        "OS-name branching",
        re.compile(
            r"\bnavigator\s*\.\s*(?:platform|userAgent)\b|\buserAgent\b|"
            r"\bprocess\s*\.\s*platform\b|"
            r"\bis(?:Linux|MacOS|MacOs|Mac|Window|Windows)\b"
        ),
    ),
)

# Reviewed per-file exemptions. Each entry is a path relative to the frontend
# source directory plus the written justification that a reviewer accepted.
# An entry that names a missing file, or a file that no longer needs it, is a
# failure so the exemption cannot outlive its reason.
# The last entry (Models.tsx post-1.0 engine placeholders) was removed on
# 2026-09-29 with the placeholders themselves.
FRONTEND_ALLOWLIST: tuple[tuple[str, str], ...] = ()

# A scan that covers almost nothing has proved nothing. The floor is well below
# the real tree and above a tree that has been gutted or fully excluded.
FRONTEND_MIN_SCANNED_FILES: int = 20
