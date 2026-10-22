# ADR-0011 — Same-user local IPC is a security boundary

**Status:** Accepted for Linux 1.0 architecture.

> **2026-08-28 clarification:** ADR-0031 defines the exact principal and
> threat-model limit. Kernel peer credentials authenticate an effective UID,
> not a particular process. Filesystem controls cannot exclude a malicious
> process already running as that same UID, and the implementation must not
> claim otherwise.

## Context

`narrivyd` can control microphone capture and text delivery. Any unrelated local process able to impersonate a legitimate client could trigger sensitive operations or read metadata/state.

## Decision

Use a per-user IPC endpoint with least-privilege filesystem/ACL permissions and validate peer identity using OS-supported peer credentials where reliable. On Linux, prefer a Unix-domain endpoint rooted in the user's secure runtime directory; reject insecure ownership/permission state rather than silently broadening access.

Protocol handshake/version negotiation occurs after transport-level peer checks. Privileged daemon commands remain policy-checked even for an authorized same-user client.

Endpoint creation/cleanup must defend against stale-path, symlink, path replacement and cross-user races. Request size/time/rate limits bound resource abuse.

## Consequences

- transport implementation is platform-specific;
- integration tests need same-user/foreign-user or permission simulations where practical;
- development overrides must be explicit and cannot ship enabled;
- future Windows/macOS adapters implement equivalent same-user policy behind the transport abstraction.
