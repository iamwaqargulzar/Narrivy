# ADR-0031 — Linux IPC principal and endpoint-race limits

**Status:** Accepted clarification for Recovery 3A (2026-08-28); command
policy implemented by ADR-0061 (2026-09-04), release evidence remains blocked

## Context

Narrivy uses a Unix-domain socket below the user's private runtime directory.
The daemon can start microphone capture, perform delivery, and expose selected
state, so both clients and the daemon must reject a foreign OS user before any
protocol data is trusted.

Linux `SO_PEERCRED` proves the peer process's effective UID at connection
time. A 0700 directory and 0600 socket/lock exclude other users, but do not
distinguish processes already running as the same UID. POSIX also provides no
atomic "unlink this pathname only if it still names this inode" operation.
A malicious same-UID process can therefore replace a pathname between an
identity check and unlink, or replace the server pathname and present the same
UID to a client. Permission modes and another `SO_PEERCRED` check cannot solve
that principal-level limitation.

## Decision

1. The Linux transport principal is the effective UID. The server validates
   the connecting client's kernel credentials before reading its handshake or
   invoking a handler. The client validates the daemon's kernel credentials
   before serializing or sending its hello.
2. The handshake role and reported PID are diagnostic/routing data only. They
   are never proof of application identity or authority.
3. The adapter must retain the 0700 runtime directory, 0600 socket and lock,
   exact owner/type/link-count checks, non-following opens, endpoint flock,
   and inode-aware best-effort cleanup. These controls defend the cross-user
   boundary and prevent accidental deletion of a replacement inode; they are
   not described as cryptographic authentication or complete resistance to a
   hostile same-UID process.
4. Transport authentication alone does not authorize privileged commands.
   ADR-0061's reviewed command policy enumerates which
   operations are read-only, state-changing, privacy-sensitive, or destructive
   and enforce user-intent/lifecycle/rate limits appropriate to each class.
   A self-declared role cannot satisfy that policy.
5. The supported Linux threat model must state plainly whether hostile
   same-UID processes are out of scope. If stronger isolation is required, it
   needs a separate architecture using an independently protected principal or
   trusted broker; more pathname checks are not an acceptable substitute.

## Consequences

- Recovery 3A can establish a truthful same-UID, cross-user-resistant
  transport without claiming per-process identity.
- A same-UID pathname-substitution residual risk remains documented.
- Release remains blocked on clean evidence and qualification; ADR-0061
  specifies and implements command authorization/rate behaviour through real
  client/daemon paths without promoting role strings to identity.
- Endpoint cleanup tests prove refusal/preservation and best-effort inode
  correctness, not an impossible atomic guarantee against the same UID.

## Required verification

- injected client-side peer rejection sends zero handshake bytes;
- server-side foreign-peer rejection occurs before handshake/handler work;
- symlink, hard-link, wrong-type, wrong-owner, and replacement-inode tests
  preserve anything the adapter cannot prove it owns;
- command-policy tests use kernel-authenticated connections and cover every
  privileged command, role-string spoofing, rate limits, cancellation, and
  reconnect/restart behavior;
- ordinary logs contain neither command payload secrets nor transcript/audio.
