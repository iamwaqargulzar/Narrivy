# Release task 015: atomic installed toggle activation and truthful shortcut fallback

DeepSeek V4.1 Flash at provider reasoning=max implements this researched,
bounded task. Codex owns architecture, review, tests and acceptance. Read the
repository authority, active P11 requirements, trigger/IPC/session contracts,
ADR-0069/0070/0071/0072, and current source after task007 and task009 acceptance. Use
`apply_patch`. Do not commit/push, mutate desktop settings, touch user data,
fabricate evidence, promote requirements, or weaken IPC/security.

## Research conclusion

The current native COSMIC host exposes XDG RemoteDesktop v2 but no
`org.freedesktop.portal.GlobalShortcuts`; upstream COSMIC still tracks that
portal as missing. COSMIC Settings does support user-created command shortcuts.
Narrivy therefore needs a real portal listener where the interface exists and
an installed atomic command fallback suitable for a user-configured COSMIC
shortcut. This task implements the common atomic daemon/CLI activation seam and
truthful capability/probe contract; a subsequent bounded task may own the live
portal session listener and admission-time target provider.

## Required implementation

1. Add an additive IPC `ToggleSession` request (protocol-minor bump and
   compatibility constant/documentation). It carries the same optional
   selectable Raw/Clean pipeline identity and a typed/closed activation source,
   not an arbitrary policy string. Keep older Start/Stop commands compatible.
2. Handle toggle atomically in the daemon under the existing admission/session
   ownership rules. If no active session exists, perform the exact same strict
   readiness/admission/start path as StartSession. If the active session is
   recording, request graceful finalization exactly once (never Cancel). If it
   is already finalizing/transcribing/delivering/cleaning up, return a typed
   accepted-already-stopping or Busy outcome without opening a second session.
   Concurrent toggles must never produce two sessions, lose the Stop, or bypass
   the single strict permit. Shutdown admission remains dominant.
3. Add `narrivy-cli toggle [--pipeline raw|clean]` using that one request; do
   not implement status-then-start/stop client logic. Its installed path and
   output must contain no transcript or target content. Update usage/tests and
   packaging documentation so the exact user-managed shortcut command is
   `/usr/bin/narrivy-cli toggle`.
4. Repair the GlobalShortcuts availability probe: the canonical portal object
   is `/org/freedesktop/portal/desktop` (lowercase). Replace the PATH-resolved
   `busctl` subprocess probe with the crate's existing direct `zbus` boundary,
   bounded by one timeout. Query the exact interface/property and report
   available only when actually present; no desktop-name inference. Preserve an
   injected deterministic probe seam for tests. Do not claim a listener exists
   merely because the interface probes present.
5. Advertise separate content-free capability facts for atomic command toggle,
   portal interface availability, and live portal listener readiness. The last
   remains false until a real owned listener is composed. Frontend/onboarding
   may show the exact user-managed shortcut command only when the daemon reports
   atomic toggle available and no live portal listener; it must not claim the
   shortcut is configured or working until a later check observes it.

## Regressions

- idle toggle starts exactly one admitted Raw/Clean session;
- recording toggle requests graceful Stop and preserves the complete tail;
- concurrent double-toggle cannot create two sessions or convert Stop to
  Cancel; late toggle during terminal cleanup has a typed deterministic result;
- shutdown admission rejects toggle; unsupported/malformed pipeline has no
  capture side effect;
- CLI emits one ToggleSession wire request and never status-then-start/stop;
- direct probe uses the canonical object path, distinguishes absent/present
  interface and times out boundedly without spawning a helper;
- capability/UI wording distinguishes available command, configured shortcut,
  and live listener.

Run focused IPC/unix transport, daemon default+resident session, CLI, trigger,
frontend tests/build and strict Clippy for changed packages. End with exact
checks and the explicit remaining live portal-listener, admission-time target
binding and native shortcut qualification work.
