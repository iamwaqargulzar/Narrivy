# ADR-0026 — Minimal Reis EI text sender

**Status:** Accepted for a non-composed Recovery Phase 2C implementation
(2026-08-27)

## Context

ADR-0025 defined a truthful actor-owned submission contract but deliberately
left the production direct-zbus driver unable to emit. A native libei binding
is not viable on the current support host: the installed libei 1.2 runtime
predates the text sender API introduced in libei 1.6 and the development
package is absent. Keycode synthesis would add keymap, modifier, Unicode, and
stuck-key state before the simpler compositor text capability has been tested.

Reis 0.7.1 is the current upstream Rust protocol implementation and is MIT
licensed, pure Rust, `unsafe_code = forbid`, and compatible with Narrivy's
portable MSRV. Its unmodified convenience path is not acceptable, however:

- `REIS_DEBUG` prints protocol arguments, including submitted text;
- the convenience handshake advertises pointer, keyboard, touch, and other
  interfaces outside Narrivy's least-privilege text boundary;
- the blocking iterator has no caller-owned absolute deadline;
- the high-level EI event converter is not `Send` and cannot be owned by the
  supervised portal actor;
- its buffered nonblocking flush does not expose the exact number of bytes
  accepted before `EAGAIN`.

## Decision

- Pin `reis = 0.7.1` with all optional features disabled. Pin the already-used
  `rustix = 1.1.4` event/time API directly for safe `poll(2)` and monotonic
  timestamps. No C library, helper executable, keymap library, restore token,
  input group, or `/dev/uinput` permission is added.
- Reject any process environment containing `REIS_DEBUG` before constructing
  an EI context. Narrivy never modifies the process environment to suppress
  the flag and never formats peer explanations, payload-bearing events, wire
  objects, or transport errors into ordinary diagnostics.
- Use Reis only for its low-level typed EI objects and wire codec. Narrivy owns
  a small sender state machine on the existing portal actor thread; it does not
  use the convenience handshake, blocking iterator, Tokio wrapper, or
  non-`Send` event converter.
- Advertise exactly version 1 of `ei_callback`, `ei_connection`, `ei_seat`,
  `ei_device`, `ei_pingpong`, and `ei_text`, with sender context. Reject missing,
  duplicate, unexpected, malformed, unnegotiated, zero-version, or
  above-advertised-version protocol objects. Bind only the text capability and
  become locally ready only after a text device is fully described and resumed.
- One submission is exactly `start_emulating`, one `ei_text.utf8`, one
  `ei_device.frame` using `CLOCK_MONOTONIC`, and `stop_emulating`, flushed in
  order. The existing payload contract keeps this to nonempty printable
  single-line ASCII of at most 254 bytes; Unicode, multiline text, chunking,
  and retry remain deferred.
- All handshake, discovery, write readiness, cancellation, and teardown use
  the caller's original absolute deadline. Polling is bounded to five
  milliseconds so cancellation is observed without detached work or a busy
  loop. The daemon actor remains the sole owner of the portal session, EI
  context, device, buffer, and submission sequence.
- The last clean cancellation point is before requests enter Reis's private
  write buffer. Once the first flush is attempted, an incomplete operation is
  conservatively `DispatchUncertain`, even if the OS may have accepted zero
  payload bytes, because Reis does not expose partial-write progress. This can
  over-report uncertainty but cannot falsely claim that no external effect
  occurred. Any uncertain result drops the entire EI context before the
  lifecycle performs bounded portal cleanup and mandatory Inbox recovery.
- A complete local flush is only `SubmittedUnverified`. It never proves that
  the compositor processed the frame or that a focused target retained the
  text. Direct delivery capability and every release requirement remain false
  until attended native target readback and later release qualification pass.
- The direct-zbus driver now owns this sender after `ConnectToEIS`. ADR-0027
  later adds an explicitly injected coordinator/recovery seam, but default
  daemon startup still composes none of it. No default IPC, CLI, or desktop
  route can request permission or submit text in this increment.

## Verification boundary

Controlled tests use a real Unix socket pair with Reis's EIS-side protocol
decoder. They verify the exact six-interface handshake, sender context,
text-only device binding, and one ordered start/text/frame/stop transaction.
They also verify missing text capability, rejection of an over-versioned peer,
clean cancellation before queueing, fail-closed debug configuration, and that
pre-dispatch EIS transport loss is closed without being mislabeled as an
authoritative portal revocation. This is real wire-protocol behavior, not a
mock sender, but it is not a compositor, portal, target-retention, native
platform, requirement, evidence, or release qualification.

## Consequences

Narrivy now has an actual low-latency EI text wire implementation without
breaking the actor, privacy, or dependency boundaries. Permission setup and
device discovery remain cold-path work; the submission hot path contains no
process spawn, filesystem write, thread creation, D-Bus call, keymap scan, or
payload copy beyond protocol serialization.

The remaining delivery-path blockers are explicit desktop parent-window
export, authorized startup/pipeline ownership, attended portal behavior, and
attended target-retention qualification. A failed or unsupported text device
must continue to recover to Inbox; keycode synthesis is a separate ADR only if
native text capability proves unavailable on a supported lane.

## Primary sources

- [Reis source and MIT license](https://github.com/ids1024/reis)
- [EI protocol overview](https://libinput.pages.freedesktop.org/libei/)
- [`ei_text` protocol](https://libinput.pages.freedesktop.org/libei/interfaces/ei_text/index.html)
- [`ei_device` protocol](https://libinput.pages.freedesktop.org/libei/interfaces/ei_device/index.html)
- [libei sender API](https://libinput.pages.freedesktop.org/libei/api/group__libei-sender.html)

## Alternatives rejected

1. Mark Reis's convenience client as approved. Rejected for debug disclosure,
   broad negotiation, non-`Send` ownership, and unbounded blocking behavior.
2. Add unsafe `Send` to the high-level converter. Rejected: its callback store
   is genuinely non-`Send`, and bypassing the type system would violate the
   actor's ownership proof.
3. Use the system libei C runtime. Rejected for this increment because the host
   runtime predates `ei_text`; silently falling back to keycodes would expand
   behavior and dependencies.
4. Call an open-source helper such as `eitype` or `wdotool`. Rejected because a
   helper process weakens lifecycle ownership, may expose payload diagnostics
   or persist tokens, and still cannot prove target retention.
5. Treat successful wire flush as verified delivery. Rejected by upstream
   semantics and the Phase 2B observation: transport acceptance is not target
   retention.
