# narrivy-process-windows

Trusted system-helper launching on Windows: the Windows counterpart of
`narrivy-process-linux`, with the same public API (`TrustedExecutable`,
`VerifiedArtifactExecutable`, `VerifiedArtifactData`, `terminate_group_and_reap`,
`try_wait_contained`, `usage::*`).

- **Role:** Adapter crate: OS-specific mechanics live here, behind the portable contracts. It owns no product policy.
- **Containment:** every child starts suspended, joins its own Job Object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and only then runs. Closing the job
  (or the daemon exiting) kills the whole tree, which replaces Linux's
  process group plus `PR_SET_PDEATHSIG`. A job is not a hostile-code sandbox.
- **Helpers:** system helpers resolve only from the Windows system directory
  and must be owned and writable only by TrustedInstaller, SYSTEM or
  Administrators; the environment is cleared except `SystemRoot`.
- **Artifacts:** a digest-bound artifact is copied into a private snapshot
  file held open with a deny-write, deny-delete share mode, re-hashed through
  that handle, and launched from it. Windows has no sealed memfd, so this is
  the immutability boundary. Trust-anchored binding (an install root plus an
  owner) fails closed with `UnsupportedPlatform` until ACL-based anchoring is
  reviewed.
- **Usage:** process-tree CPU and memory via Toolhelp snapshots,
  `GetProcessTimes` and `K32GetProcessMemoryInfo`.
- **Tests:** run on Windows: `cargo test -p narrivy-process-windows --locked`.

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
