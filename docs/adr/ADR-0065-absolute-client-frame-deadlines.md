# ADR-0065 — Absolute deadlines for client handshake/request frames

Status: Accepted implementation prerequisite, 2026-09-05

## Problem

The portal executor needs finite request lifetimes. Unix IPC previously set a
read timeout once before read_frame and retained a fixed handshake write timeout.
A peer sending short chunks before each timeout could extend a frame indefinitely;
a request write could also exceed the caller's smaller timeout.

## Decision

The Unix client wraps handshake frame exchange and command request frame I/O in
a safe Read/Write adapter that recomputes the remaining absolute deadline before
every underlying socket read, write and flush. Request write and reply reads share
one deadline. A request error before a complete correlated response closes the connection and clears its buffers; callers
must reconnect explicitly. No command is replayed automatically, since a failed
read does not establish that a previously written operation was unexecuted.
A complete correlated daemon rejection preserves the usable connection; it is
a command outcome, not a framing failure.

Wire formats and portable contracts remain unchanged. Handshake frame exchange
retains its five-second budget. Connection and event-path completion is recorded below. This change does not
change daemon operation deadlines or supply the owned portal executor.
No native/release acceptance is implied.

## Verification

Unix socket-pair regressions show an expired deadline sends no bytes and a peer
trickling a complete frame cannot restart the absolute read budget. The complete
Unix transport test suite and warning-denying Clippy pass. Existing authentication
ordering and interleaved-event/request tests remain in the suite.

## Connection and event-path completion

`connect_as_until` accepts one absolute deadline for socket connection plus
authenticated handshake. The existing connect/reconnect APIs use their existing
five-second default as a total budget. The Unix adapter adds a direct exact
socket2 0.6.5 dependency, already present in Cargo.lock; nonblocking timed connect
and owned descriptor conversion introduce no client unsafe code. Failed connect
sockets are dropped and never reused. Peer authentication still precedes hello.

Event frames use the same absolute-deadline reader. A timeout before any prefix
byte leaves the connection usable for the next wait. An incomplete frame or
terminal read failure invalidates transport and buffers. This prevents a later
read from interpreting a partial frame remainder as a fresh length prefix.

33 Unix transport tests pass, including zero-budget connection without accept,
silent handshake EOF after timeout, idle-event reuse and incomplete-prefix
invalidation. Clippy passes with warnings denied.
