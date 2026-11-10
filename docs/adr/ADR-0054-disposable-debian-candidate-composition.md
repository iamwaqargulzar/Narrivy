# ADR-0054: Disposable Debian candidate composition

Status: Accepted design for controlled Recovery 4Q-C4 (2026-09-01);
non-release, non-installing, and not package qualification

## Context

ADR-0053 can create the fixed resident-worker subtree, but it deliberately does
not establish Debian archive membership. The historical package and legacy
builder remain unsuitable authorities: they omit the resident runtime, permit
missing/debug substitutions, generate stub supply-chain material, overwrite a
protected artifact, and compose a service that can fall back to mock, fixture,
or null behavior.

Archive structure should be tested before installation work, without repairing
the legacy builder, overwriting a retained artifact, enabling a user service,
or implying that the repository has a qualified model distribution.

## Decision

Recovery 4Q-C4 adds a separate builder for one disposable package named
`narrivy-recovery-candidate`, version `0.0.0~recovery4qc4`. It requires explicit
absolute release ELF paths for the daemon, CLI, desktop, and resident worker,
plus fixed service, desktop, icon, license, and notices inputs. It has no
missing or debug fallback.

The builder validates stable current-user-owned non-symlink inputs, copies the
three product binaries with ELF64 amd64 checks, and delegates the worker and
runtime-manifest subtree to ADR-0053. It accepts only the reviewed strict
installed-runtime service composition and rejects mock, sine, null, raw runtime
identity, optional environment files, and unbounded restart semantics.
The service and desktop entry must match their complete canonical byte
sequences, including section order. All admitted asset inputs reject group or
world write permission.

ADR-0063 later makes the service-manager containment contract explicit:
`ExitType=cgroup`, `KillMode=control-group`, a three-second stop timeout with
kill-on-timeout, and final `SIGKILL` are required in the byte-exact template.
This is structural intent only until native package-artifact qualification.

Before constructing the archive, the supplied daemon is launched once with an
empty private model cache and exact strict installed-runtime environment. It
must exit 1 with the fixed content-free missing-model message, no stdout, no
IPC socket, and no surviving process group. This proves that the binary contains
the installed resident lane and fails closed when its selected model is absent;
it does not prove successful daemon startup or model behavior.
The daemon is copied to a digest/size-verified sealed executable memfd before
the probe. A temporary child-subreaper boundary detects, kills, and reaps even
descendants that escape the original process group into another session.

The archive is built only below a mode-0700 temporary directory by the fixed
root-owned `/usr/bin/dpkg-deb`. Its data and control tar streams are then read
back and compared with an exact file inventory, byte payloads, modes, and root
ownership. The only maintainer script is `preinst`, which always exits 1, so
the candidate cannot be installed accidentally. Two builds from identical
inputs must be byte-identical. The archive and entire build tree are deleted
before the command returns.

C3's non-serialized root identity is required at the handoff. Subsequent
directory walks, writes, binary copies, and timestamp/mode normalization are
anchored to retained directory descriptors with fixed components and
`O_NOFOLLOW`; restrictive umasks are repaired on the exact newly opened
directory. The archive output is exclusively created with `O_NOFOLLOW`, held
by descriptor, and supplied to `dpkg-deb` only as that inherited descriptor.
The same descriptor is copied into a size-capped sealed memfd used for both tar
inspections and the reported digest. Tar names, types, declared sizes,
expanded-byte totals, exact directories, duplicate membership, metadata, and
payload hashes are checked before or during bounded streaming reads.

No model is bundled. The service selects `whisper-tiny.en-Q8_0` only to make
the empty-cache negative probe deterministic. The current registry is not a
release authority and does not contain an exact-digest model that is also
streaming-capable, so this ID is not adopted as the product's release default.

## Trust semantics

A successful run proves only deterministic, inspected Debian archive
composition from the supplied local inputs and a fail-closed missing-model
daemon path. It does not establish:

- retained release-artifact identity, signing, SBOM, or external provenance;
- dpkg installation, package-database membership, activation, upgrade,
  rollback, removal, or clean-machine behavior;
- model license, acquisition, installation, streaming capability, or native
  transcription behavior;
- maintainer-script correctness or per-user systemd lifecycle; or
- any release requirement, evidence, or readiness transition.

The protected historical package and build log remain immutable. The legacy
builder remains unchanged and blocked.

## Consequences

- Exact archive membership, modes, ownership, deterministic bytes, and the C3
  runtime subtree can be tested without retaining a package.
- Strict service defaults can no longer silently select substitute behavior in
  this candidate.
- Installation is intentionally prevented until a separate phase resolves
  model provisioning, canonical package metadata, maintainer lifecycle, and
  clean-machine qualification.
- C4 creates no evidence, manifest reseal, requirement change, package output,
  commit, push, or release claim.
