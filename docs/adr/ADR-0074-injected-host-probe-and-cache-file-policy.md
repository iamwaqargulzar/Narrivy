# ADR-0074: Injected Host-Fact and Cache-Protection Contracts

- Status: Accepted
- Date: 2026-09-28

## Context

Two crates that the portable-boundary validator protects carried operating
system and process-spawn implementation, and both were reachable only through
test or example targets.

`narrivy-compute` decides which compute device Narrivy may plan onto. Its
discovery module spawned `python3 -c 'import openvino'` with a scrubbed
environment, ran `ldconfig -p` and parsed the listing, ran `lspci`, ran
`vulkaninfo --summary`, ran the enumeration sidecar, read `/dev/accel` and
`/proc/cpuinfo`, and set a POSIX execute bit in a test helper. Its
qualification module spawned the sidecar with an unbounded `Command::output`.
Enumeration is not capability, and the module's own doc comment says so, but
neither the enumeration nor the qualification could be exercised on a host that
is not Linux, and neither could be asserted down to the exact program,
arguments and environment a decision rested on.

`narrivy-model-manager` applied 0600 to the staged download and to the metadata
sidecar, applied 0700 to the cache root and every component below it, compared
POSIX uids to refuse a cache directory the user did not own, and compared
device plus inode to close the stat-then-open race. All four are `#[cfg(unix)]`,
so on any other target the model cache silently lost its owner-only guarantee
while the crate's documentation still promised private storage.

A 0600 model cache is a privacy control, not incidental hardening. Removing it
to make the crate portable would have been a security regression, and the
boundary rule exists precisely so that "portable" is achieved by moving the
mechanism, not by deleting the guarantee.

## Decision

1. `narrivy-compute::probe` defines the `HostProbe` contract and the
   `ProbeCommand` / `ProbeEnvironment` / `ProbeOutcome` / `ProbeError` value
   types. The crate contains no process spawning, no environment handling and
   no device probing; every fact about the host arrives through an injected
   probe.
2. There is no default `HostProbe`. A zero-effort implementation that reports
   "no devices, no runtime, no cores" is exactly the shape that turns an
   unprobed host into a false negative claim, so the crate cannot reach one by
   accident.
3. The portable crate keeps *what* a probe must establish: which helper
   answers which question, with which arguments, under which environment, and
   inside which budget. Those are request values, not invocations, and a test
   asserts them directly. The platform owns *how* a request is run.
4. A probe that cannot be run, and a probe that runs and reports failure, are
   both "the host does not have it". Neither ever becomes a capability claim.
5. `qualify_decode` runs through the same `HostProbe` under its declared
   timeout, so a wedged sidecar surfaces as `SidecarTimeout` instead of
   hanging the caller. A post-run wall-time comparison is retained, so an
   implementation that overshoots its own budget is rejected too.
6. `narrivy-model-manager::policy` defines `CacheFilePolicy`, whose contract is
   stated as an obligation, never as a bit pattern: a path must be owner-only,
   a new file must be created owner-only, an owner identity must be reportable,
   and object identity must be provable or refused. The portable crate keeps
   the requirement; the platform chooses the representation, so composition
   cannot quietly weaken the guarantee into a no-op without failing a test.
7. `ModelStore::new` takes the policy as a required argument. A default is not
   offered for the same reason as in decision 2: a store cannot know how the
   host protects a file it created.
8. A policy that cannot honour an obligation fails with
   `ModelManagerError::CacheProtectionUnavailable` rather than reporting
   success. Owner identity the platform cannot report is an error, not a
   `None`, so a host without a portable owner model refuses the cache rather
   than skipping the ownership proof. Object identity the platform cannot prove
   is `false`, which the store already treats as "refuse these bytes".
9. When the staged download cannot be made owner-only, the staged part is
   removed before the failure propagates. Bytes that cannot be protected must
   not be left on disk at whatever the transport's umask produced. This matches
   the size-mismatch and digest-mismatch paths, which already clean up.
10. The Linux implementations live in `adapters/platform-linux`:
    `compute_probe::LinuxHostProbe` and `model_cache_policy::PosixCacheFilePolicy`.
    No new adapter family is created. `adapters/process-linux` was considered
    and rejected: `component-boundaries.md` scopes it to no model-selection
    policy, which is exactly what "which helper proves a runtime is installed"
    is, and it accepts only `/usr/bin` and `/bin` executables, which excludes
    the `/usr/local/*` search path a vendor runtime pack installs into.
11. The demo `examples/p5_discover` and the host-dependent qualification
    coverage supply their own probe. Both live outside `src/`, because the
    library must contain no process-spawning implementation at all; the
    product composition injects the reviewed Linux probe instead.

## Truth boundary

Discovery remains enumeration plus a runtime-presence check. It does not prove
that an accelerator can decode a model, and it never does: a device is
selectable only when a measured passing qualification tuple also exists, which
is the existing `narrivy-compute` rule and is unchanged here.

Injecting a probe does not make a host claim more trustworthy. A probe
implementation that lies still produces a false claim, and that is a platform
defect, not a portable-crate one.

`PosixCacheFilePolicy` reproduces the previous Linux behaviour exactly: 0600 on
files, 0700 on directories, uid equality against the cache root, and
device-plus-inode identity. It does not add ACL, SELinux, quota, or immutable
flag handling, and it does not make completed-directory publication atomic.

The `UnavailableCacheFilePolicy` variant exists so the fail-closed path is
reachable in real composition. It is not a supported platform.

## Consequences and deferred work

- `narrivy-compute` and `narrivy-model-manager` now build, lint and test
  identically on every target the workspace supports, and their discovery,
  qualification and cache-protection decisions are assertable without a
  platform.
- Every existing `ModelStore::new` caller must supply a policy. Existing
  `narrivy-compute` discovery, qualification and example callers must supply a
  probe.
- A device class is still reported as unavailable when no passing qualification
  tuple exists, which on a fresh install means CPU only. Wiring the
  qualification producer so an accelerator can ever be selected is separate
  work and is not created by this ADR.
- Completed-directory atomic publication, install locks, and
  directory-descriptor or `O_NOFOLLOW` mechanics remain deferred, exactly as
  ADR-0051 records for the model cache.
- No evidence, requirement status, package, manifest seal, or release claim is
  created by this ADR.
