# Daemon and Local IPC

## Security boundary

The daemon can capture audio, deliver text, and access user-authorized context. Local IPC is therefore a security boundary, not an internal convenience socket.

## Transport abstraction

Domain/application code depends on protocol messages, not Unix socket/named-pipe APIs. Expected platform transports:

- Unix: per-user Unix-domain socket under a secure runtime directory such as `XDG_RUNTIME_DIR` when available;
- Windows later: per-user named pipe with explicit ACL;
- macOS later: per-user local transport behind the same abstraction.

See `ADR-0011-local-ipc-security.md`.

## Protocol envelope

Every request/event/response includes as applicable:

- protocol major/minor;
- message kind;
- request ID;
- session/transaction ID;
- monotonic sequence for ordered event streams;
- payload variant;
- typed error/result;
- optional capability/protocol feature list.

Do not expose Rust memory layouts as the wire contract. Choose a stable bounded serialization in P1 and version it.

## Handshake

Before privileged commands:

1. transport accepts connection under least-privilege endpoint permissions;
2. peer identity is validated where the OS exposes reliable peer credentials;
3. client sends protocol range/build identity/client role;
4. daemon selects compatible protocol/features or rejects with a typed incompatibility error;
5. rate/size/time limits become active.

No compatibility path may disable peer restrictions.

## Required commands/events

P1 protocol must be able to express:

- query capabilities/status;
- create/start/cancel dictation session;
- subscribe to session events;
- update safe configuration;
- query models/devices/modes/profiles/history metadata;
- request diagnostics tests/report;
- daemon graceful shutdown only under explicit allowed policy;
- errors and degradation reasons.

Raw audio does not need to traverse generic UI IPC unless a later architecture decision requires it.

Optional History and short-lived crash recovery are separate protocol
surfaces. History commands never read the recovery journal. A client that
needs the just-completed result requests only the exact event transaction via
the protocol-minor-4 recovery-entry command; absence of a History capability
is treated as disabled.

## Robustness

- bounded message and collection sizes;
- bounded request timeout;
- malformed/unknown message rejection;
- cancellation and disconnect semantics;
- no unbounded event queue for a slow client;
- notification-driven event dispatch with owned close/join, atomic publication
  admission closure, and separate eviction/incoming-drop/coalescing/rejection
  accounting;
- reconnect obtains a state snapshot plus events from a safe boundary rather than trusting stale UI state;
- stale socket cleanup must not follow arbitrary symlinks or remove another user's endpoint.

## Diagnostics privacy

IPC diagnostics metadata may include message kind, latency, error code, client role/build, sizes, and protocol versions. It must not log transcript/audio/clipboard/selection/context payloads by default.


> **ADR linkage:** See ADR-0012 and ADR-0013 for lifecycle and persistence decisions.
