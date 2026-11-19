# ADR-0030 — Repair portable transport and journal boundaries

**Status:** Implemented locally by transport slice Recovery 3A and journal
slice Recovery 3B (2026-08-28); cross-target qualification and the documented
same-UID filesystem-race decision remain required before acceptance

## Context

The current dependency gate reports `windows-sys` below `narrivy-core`,
`narrivy-ipc`, and `narrivy-pipeline`. Read-only graph inspection shows that
these edges come from the cross-platform `tempfile`/`rustix` stack and are
Windows-target-only. IPC and pipeline use `tempfile` only for tests; core also
uses it in its concrete filesystem journal. Filtering Cargo metadata for the
Linux target removes the reported packages.

That does not make the portable boundary healthy. The current source scanner
does not recognize standard-library platform types, and therefore exits zero
despite production coupling:

- `narrivy-core::journal` imports Unix `PermissionsExt`;
- `narrivy-ipc` owns `UnixStream`, `UnixListener`, Unix socket paths and modes,
  peer-credential FFI, and `flock` lifecycle behavior.

Canonical documentation describes `narrivy-ipc` as wire-level protocol with
transport adapters outside it and requires portable crates to compile on Linux,
Windows, and macOS. Treating a target-unaware transitive package-name scan as a
pass/fail oracle would both report false positives and miss the real direct
source coupling.

## Proposed decision

1. Keep protocol envelopes, framing, request/response/event types, versioning,
   and transport-neutral errors in the portable IPC crate.
2. Move Unix client/server sockets, endpoint filesystem ownership,
   `SO_PEERCRED`/`getpeereid`, `flock`, and default Unix socket-path resolution
   into a Linux/Unix transport adapter composed by clients and `narrivyd`.
3. Keep recovery-journal records, retention policy, and the storage port in a
   portable `narrivy-journal-api`; keep orchestration ordering in core. Compose
   the Unix 0700/0600, no-follow atomic implementation outside core.
4. Update source-boundary tests to detect standard-library OS modules and
   native socket/filesystem types. Add negative fixtures proving those imports
   fail in portable crates.
5. Make dependency validation semantic and fail-closed:
   - inspect direct normal/build dependencies of protected crates for forbidden
     implementation packages;
   - exclude dev-only test dependencies from production architecture claims;
   - do not treat target-specific internals of a reviewed cross-platform
     abstraction as direct coupling;
   - run the portable compile matrix for Linux, Windows MSVC, and macOS so
     target filtering cannot hide missing portability.
6. Preserve same-user authentication, endpoint modes, stale-socket safety,
   journal atomicity, cancellation, and typed errors during the move. No
   boundary fix may replace these with mocks or weaken their tests.

## Acceptance required before status becomes Accepted

- Portable crates contain no direct `std::os::*`, Unix socket, platform FFI,
  Tauri, or concrete platform/runtime imports in production code.
- Linux transport and journal adapters retain the current security and failure
  tests plus composition tests through the real daemon/client boundary.
- Dependency and source boundary validators include positive and negative
  fixtures and pass without exceptions for Narrivy-owned coupling.
- Portable crates compile for all three documented target families.
- Workspace check/test/Clippy and supply-chain gates pass without suppression.

## Recovery 3A disposition

The Unix client/server, credential checks, endpoint ownership, and connection
lifecycle now live in `adapters/ipc-unix`; the portable crate retains only
wire contracts, framing, and transport-neutral errors. The dependency
validator passes and the source validator now fails truthfully on the two
remaining Unix permission references in `narrivy-core::journal`. Linux checks
pass, but Windows and macOS target checks were not run because those targets
are not installed. This is partial implementation, not acceptance of the
whole ADR.

## Recovery 3B disposition

Recovery-journal records, typed errors, transaction-ID rules, and retention
policy now live in `narrivy-journal-api`. `adapters/journal-unix` owns private
directory/file enforcement, bounded serialization and reads, atomic replace,
file/directory synchronization, corruption quarantine, bounded quarantine
retention, and validated crash-temp cleanup. Core production code consumes
only the port; the daemon explicitly composes the Unix adapter. A literal
pre-extraction JSON fixture protects every field and state tag, and the
restored 100-entry torn-write/restart proof runs against the adapter.

The adapter revalidates path components, owner, mode, type, and link count on
operations, but pathname checks cannot defeat a malicious process already
running as the same UID between check and use. A directory-FD/openat design or
an explicit acceptance of that same-principal limit needs a follow-on ADR.
Only Linux GNU and the installed Rust 1.85 toolchain were exercised locally;
Windows/macOS native matrix results are not claimed. Separately, the daemon's
current runtime-directory journal location conflicts with ADR-0013's durable
XDG state location and remains release-blocking rather than being changed in
this boundary-only slice.

## Non-decision

This proposal does not authorize moving files yet, changing IPC wire formats,
weakening authentication, altering journal semantics, editing evidence, or
claiming cross-platform runtime support. A dedicated bounded work order and
independent review are required.
