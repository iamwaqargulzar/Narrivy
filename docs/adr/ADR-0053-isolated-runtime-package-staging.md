# ADR-0053: Isolated installed-runtime package staging

Status: Accepted design for controlled Recovery 4Q-C3 (2026-09-01);
non-release and not package qualification

## Context

ADR-0052 defines the installed worker and manifest paths, but the historical
Debian artifact and the legacy package builder do not contain or safely stage
that runtime. The builder can tolerate missing release binaries, substitute
debug binaries, generate stub supply-chain material, overwrite a release
artifact, and append a self-reported build log. Changing that builder while
also introducing the runtime layout would combine too many untrusted
behaviors.

A package-root layout can be established and tested independently without
building a `.deb`, invoking root, changing package metadata, or claiming that
dpkg installed the files.

## Decision

Recovery 4Q-C3 adds a standalone staging command for only two fixed leaves:

- `usr/lib/narrivy/narrivy-transcribe-worker`, mode 0755;
- `usr/share/narrivy/installed-runtime-manifest.json`, mode 0644.

The command requires an explicit absolute worker path and an explicit absolute
output root that does not exist. It refuses `/`, the repository root, protected
artifact/log targets, existing roots, symlink roots or parents, and missing
parents. It creates the isolated root atomically with mode 0700 and package
directories with mode 0755.

The source worker must be a current-user-owned, regular, non-symlink file with
exact mode 0755 and size in `1..=256 MiB`. It is opened with `O_NOFOLLOW`, and
the opened identity must match the observed leaf. Staging accepts only a
complete 64-bit little-endian x86-64 ELF executable/shared-object header with
a bounded non-empty program table, matching the Linux 1.0 `amd64` package lane.

The worker is copied through the opened descriptor into an exclusively created
destination while SHA-256 and byte count are computed. The source descriptor's
identity, size, and timestamps are rechecked before success. The generated
canonical JSON uses schema version 1, runtime ID `transcribe.cpp`, lowercase
digest, and exact copied size. Files and containing directories are fsynced.

If any operation fails, only the output root created by that invocation is
removed. An existing path is never overwritten, removed, or reused. Ordinary
output is content-free and contains only staged size and digest.

For an in-process follow-on composer, the returned dict-compatible manifest
also carries the staged root's device/inode identity as non-serialized process
metadata. That identity is not written into the runtime manifest or CLI output;
it exists only to prevent a replacement between C3 descriptor closure and the
next phase's descriptor acquisition.

## Trust semantics

Successful staging proves only that one builder-user-owned isolated tree
contains worker bytes matching its generated manifest. It does not establish:

- root ownership on an installed machine;
- Debian archive or dpkg membership;
- correspondence with the protected historical `.deb`;
- package version, architecture metadata, reproducibility, SBOM, license,
  signing, or external build/release provenance;
- a provisioned/selected model or a runnable systemd service; or
- native microphone, transcription, delivery, upgrade, removal, or release
  behavior.

Root ownership may arise only in a later reviewed package-construction step.
Package database membership and external release provenance remain independent
authorities. The legacy builder is unchanged and remains blocked.

## Consequences

- The C2 installed-runtime layout can be generated deterministically without
  invoking unsafe legacy packaging behavior.
- An actual release-profile worker can be exercised in a disposable root and
  securely removed afterward.
- Later work can replace or quarantine the legacy builder, compose this exact
  subtree into a disposable `.deb`, and independently inspect that package.
- Model provisioning and service selection remain separate because enabling
  installed mode before a verified model exists would make daemon startup fail.
- No evidence, requirement transition, manifest reseal, protected-file change,
  commit, push, package, or release claim follows from this decision.
