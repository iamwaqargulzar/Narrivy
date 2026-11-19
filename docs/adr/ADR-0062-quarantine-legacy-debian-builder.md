# ADR-0062: Quarantine the legacy Debian builder

Status: Accepted recovery safety boundary (2026-09-04); no replacement release
builder, package, evidence, or qualification claim

## Context

`scripts/build_deb.sh` remains the historical P10 entry point and is named by
the old acceptance scaffold, but it is not safe to execute. It can continue
after release binaries are missing, substitute debug binaries, synthesize stub
builder identity and provenance, tolerate absent generated material, overwrite
the retained historical `.deb`, and append a self-reported build log. Recovery
4Q-C3/C4 deliberately avoided this path and proved only an isolated runtime
tree and disposable install-blocked archive.

Leaving the historical command callable creates an accidental mutation and
false-release path while B-PACKAGE-015 is open. Preserving its body is useful
for identifying every behavior that the eventual replacement must reject, but
reachability is not required for that audit.

## Decision

The historical entry point exits with status 2 immediately after strict shell
mode is enabled and before it resolves inputs, creates a staging directory,
generates supply-chain files, writes an artifact, or appends a log. Its output
is fixed, content-free, and identifies the disposable install-blocked candidate
tool as the only current composition check.

A governance regression invokes the command with a valid-looking version and
proves the retained historical artifact and build log keep their exact
SHA-256 identities. The historical implementation remains below the
unconditional quarantine for forensic comparison; it is not an alternative or
an overrideable path.

The P10 qualification runner and P10 evidence-refresh module are also reduced
to import-safe entry points exposing only `main()`. Both exit 2 with fixed
content-free output and cannot write evidence, generate release metadata, call
the legacy builder, or reuse a stale `.deb`. The Recovery Phase 1 non-mutation
test covers them alongside the already quarantined P6/P9/P11 paths.

The P10 acceptance command remains intentionally fail-closed. Re-enabling this
path is prohibited. A replacement must use a new reviewed implementation and
must independently establish exact release inputs, immutable builder identity,
SBOM/provenance binding, installed-runtime membership, safe user-service
lifecycle, model provisioning, deterministic output, and clean-machine
install/upgrade/removal behavior before it can retain a candidate artifact.

## Consequences

- Routine or stale automation cannot overwrite the retained package or append
  a misleading build record through the known legacy entry point.
- Recovery C3/C4 remain available and unchanged for non-installing structural
  checks.
- No legacy behavior is legitimized, no package is produced, and no release
  requirement changes state.
- B-PACKAGE-015 stays open until the separate replacement and native package
  qualification exist.
