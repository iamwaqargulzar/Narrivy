# ADR-0020 — Evidence trust registry, verification profiles, and artifact binding

**Status:** Accepted  
**Date:** 2026-08-26

## Decision

Release-eligible evidence uses schema version 2 and is accepted only when its
file is listed in `agent/evidence/INDEX.json` with the exact file SHA-256.
The index is the authoritative registry of qualified evidence; an unregistered
schema-v2 record is not proof, even if it is otherwise schema-valid.

`spec/evidence-verification-profiles.json` is the reviewed, fail-closed mapping
from each traceability `verification_layer` to permitted evidence types,
execution environment, artifact-binding policy, native-platform requirement,
and behavioural-runner requirement. Each evidence record also names an approved
procedure for its verification ID. A procedure fixes the command argv, required
assertions, and—where behavioural proof is required—the repository runner and
its hash. It also declares `permitted_substitutions`. The profile and procedure
allowlists are both mandatory: an evidence record's substitutions must be a
subset of each; there is no implicit allowance for controlled evidence. The
registry starts empty; no unreviewed procedure can qualify.

Native-behaviour and package-artifact procedures are behavioural procedures.
They reject every schema-defined substitute (`mock`, `fixture`, `proxy`,
`stub`, and `synthetic-audio`) even if a future contract accidentally declares
one. Such a substitute cannot stand in for the actual executable or package
whose behaviour is claimed.

Only profiles for genuine native/package behaviour require
`artifact_under_test` with exactly one typed identity:
identity:

1. `executable` — an executable file invoked as the recorded command; or
2. `package` — a `.deb` in `artifacts/releases/` named as an argument to the
   recorded command.

The binding path and SHA-256 must appear identically in `artifact_hashes`.
Command parsing is token-aware, so a path mentioned only in prose or a shell
comment is not an artifact binding.
Native-platform/runtime claims bind the executable or package actually
exercised. Package-artifact claims bind a package. Release-lane checks bind
the record to the exact declared candidate package hash.

Unit/contract profiles instead permit `unit` evidence in a declared controlled
environment and forbid an artifact-under-test claim. They still require the
same semantic success, clean matching commit, authoritative index registration,
reviewed procedure provenance, and explicitly approved substitutions as every
other profile.

## Consequences

The evidence policy rejects failed, skipped, missing, malformed, or nonzero
exit evidence before a requirement can be marked passed. It rejects unit,
mock, or simulated evidence for native behaviour, and it rejects package
existence commands such as `test`, `true`, prose, comments, or unrelated
commands as behavioural proof. Source documents and other unrelated repository
files cannot substitute for the exercised binary or package. It also rejects
all substitutes for native/package claims, and undeclared substitutes for
controlled claims. Historical JSON and Markdown evidence remain quarantined
and are not registered.

This changes governance evidence format only. It does not modify product
runtime, packaging behavior, or qualification results.
