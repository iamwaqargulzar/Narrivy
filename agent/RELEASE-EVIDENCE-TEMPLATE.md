# Release evidence: what `release_gate.py` checks

The three release-evidence files in `artifacts/release-evidence/linux-1.0/`
are produced by **generators** (completion plan WP4 T4.4, WP6 T6.1–T6.3),
never typed by hand. This page lists what `scripts/release_gate.py` enforces,
so the generators and their tests can be checked against it. Re-read the
script before relying on this list; the script is authoritative.

## `release-artifacts.json`
- Valid against `spec/schemas/release-artifacts.schema.json`.
- `git_commit` equals the frozen release commit; `git_dirty` is `false`
  (WP6 T6.0 adds a live cleanliness check).
- `artifacts[]` includes the kinds `deb`, `sbom` and `provenance`, each with an
  existing path, the exact `sha256` and `size_bytes`. `sbom_path` and
  `provenance_path` point at those entries.
- `signing` is truthful: unsigned releases say so (B-SIGN-005).

## `qualification.json`
- Valid against `spec/schemas/linux-qualification.schema.json`.
- Bound to the same commit and the same package hash.
- A passing native `cosmic-wayland` environment (`e2e_pass: true`). That is
  the only required lane for the first release (owner, 2026-09-28).
- The target-app matrix covers the classes in `REQUIRED_TARGET_CLASSES`.
- The CPU fallback, accessibility and privacy/security results (WP6 T6.0
  binds these to evidence IDs).
- `accelerator_claims[]` only with evidence; none is expected for 1.0.

## `summary.md`
- At least 300 characters; no `TODO` or `REPLACE`.
- Names **Pop!_OS**, and must not claim any other distribution or desktop.
- Contains:
  - the build identity: commit, version, package SHA-256, model and its
    SHA-256;
  - the reference environment: OS, kernel, COSMIC version, CPU, microphone;
  - the verified capabilities and target-app results per class;
  - known limitations (only verified ones);
  - how to bind the COSMIC shortcut.
