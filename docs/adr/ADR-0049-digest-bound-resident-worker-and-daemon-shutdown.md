# ADR-0049: Digest-bound resident worker and explicit daemon shutdown

- Status: Accepted for controlled Recovery 4Q-A implementation
- Date: 2026-09-01
- Release effect: none; all Linux 1.0 requirements remain blocked

## Context

The process-isolated resident ASR worker previously came from an arbitrary
environment pathname. Its checks did not bind the executable bytes to an
externally reviewed identity, it inherited an insufficiently constrained
process boundary, and daemon shutdown did not explicitly close resident
admission and prove every worker generation reaped. Static daemon readiness
also reopened the worker path and could disagree with the already-running
engine.

This phase is intentionally narrower than complete resident-runtime trust.
The model remains pathname-opened and header-checked, and the Debian package
does not yet provide a package-manifest binding for the worker or model.

## Decision

1. `adapters/process-linux` owns a separate digest-bound executable-artifact
   contract. A controlled worker must be an absolute, current-effective-user
   owned, safely permissioned native ELF with an externally supplied exact
   SHA-256 and byte size. This contract is distinct from root-owned system
   helper resolution.
2. Binding opens the source without following symlinks, validates its complete
   ELF header and bounded program-header table, copies it into an executable
   memfd, sets exact mode 0500, applies write/grow/shrink/future-write/execute
   and seal seals, and rehashes the snapshot with positional reads. Source
   pathname changes after binding are not launch inputs.
3. Artifact children receive an empty environment, cwd `/`, a dedicated
   process group, parent-death SIGKILL plus expected-parent check, umask 0077,
   close-range CLOEXEC, and no-new-privileges. Arguments, paths and digests are
   redacted from `Debug` output. Process-group cleanup and direct-child reaping
   are bounded and residual descendants are typed failures.
4. Resident startup requires `NARRIVY_TRANSCRIBE_WORKER`,
   `NARRIVY_TRANSCRIBE_WORKER_SHA256`, and
   `NARRIVY_TRANSCRIBE_WORKER_SIZE`. One immutable snapshot is retained by the
   engine and reused by every recovery generation. Production has no raw-path
   constructor or fallback to sidecar/mock.
5. Engine readiness polls contained liveness in both Ready and Active states.
   Shutdown serializes permanent admission closure against recovery, waits for
   in-flight recovery ownership, sends the protocol shutdown operation, then
   forcibly terminates and reaps under the same absolute deadline if graceful
   exit does not complete.
6. The daemon binds and handshakes the engine once before IPC serving. Status,
   pipeline listing, StartSession and SelectPipeline use the same stored,
   content-free runtime snapshot. No status call reopens or rehashes the source
   executable. Every available/enabled/composed claim requires Ready plus the
   controlled digest binding.
7. Controlled-build binding is reported separately from installed-package
   provenance. Package binding remains `unavailable:not-composed`; this phase
   cannot establish a release claim.
8. Daemon shutdown removes the resident lookup handle, closes and joins the
   supervisor, always attempts resident shutdown, retries only typed timeouts,
   then closes and joins the event pump. Ordinary errors are sticky; a failed
   or timed-out final cleanup attempt cannot be erased by an earlier success.

## Trust boundary

`ControlledDigestBound` proves that the launched resident executable bytes
match an explicit digest and size supplied to this process and that recovery
generations reuse the same sealed snapshot. It does not prove who built those
bytes, that they belong to an installed package, that the package/SBOM and
release artifact agree, or that the model loaded by the worker is immutable.

Process groups contain ordinary descendants but are not a hostile-code
sandbox. A process which creates another session may escape group cleanup;
systemd/cgroup qualification remains required.

## Unsafe invariants

Unsafe code remains confined to the Linux process adapter. The parent performs
ordinary file inspection and memfd construction. Between fork and exec, only
reviewed raw syscalls configure no-new-privileges, parent death, parent
identity, umask and close-range CLOEXEC; no lock or allocator-dependent Rust
operation is introduced there.

## Alternatives rejected

- Keep executing the environment pathname after a superficial mode check:
  bytes and identity remain substitutable.
- Revalidate the pathname for every status request: this is expensive and
  describes a new disposable snapshot rather than the daemon-owned engine.
- Expose a production raw-path constructor for integration tests: this creates
  a bypass in the release surface. The diagnostic instead uses the production
  path/digest/size contract.
- Treat controlled digest binding as package provenance: no installed-package
  manifest or release-artifact chain exists yet.
- Let `Drop` be the normal daemon shutdown policy: cleanup results cannot be
  surfaced and ordering against supervisors/event publication is ambiguous.

## Deferred work

- immutable model descriptor/FD and digest binding plus worker attestation;
- installed worker/model package manifest, signature, SBOM and release
  provenance;
- systemd/cgroup abrupt-death and escaped-session qualification;
- clean-machine install, upgrade and rollback validation;
- remaining helper-family migration or removal;
- native microphone, latency, accuracy, memory, thermal and target-delivery
  qualification; and
- schema-v2 evidence and requirement transitions after independent review.

## Non-goals

No model binding change, package build, native microphone run, evidence/index
or requirement transition, manifest reseal, commit, push, or release claim.

