# ADR-0041 — Notification-driven daemon event pump

**Status:** Recovery 4K controlled composition accepted (2026-08-29);
native latency, packaged lifecycle, and release qualification remain blocked

## Context

The daemon-to-IPC event pump checked its bounded queue every five milliseconds.
That imposed up to one polling interval of avoidable presentation latency and
woke an idle daemon roughly 200 times per second. Volatile partial hypotheses
made both costs more visible. Replacing the sleep with an unowned notification
would create lost-wakeup and shutdown races: a publisher could enqueue after
the pump observed an empty queue and stop flag, or a stop signal could arrive
between a predicate check and the wait.

## Decision

The bounded event bus owns a condition variable and keeps queue admission plus
closed state under the same mutex. Successful publication notifies the single
owned pump immediately. A 250-ms timeout is only a defensive lifecycle fallback;
it is not the normal dispatch schedule.

Shutdown first closes session-worker admission and attempts bounded worker
reaping. A timed-out worker is a daemon shutdown failure, never success. After
IPC connection shutdown completes, the event bus atomically closes publication
admission and wakes the pump. Events accepted before closure remain drainable;
later lifecycle or partial publications are rejected. If IPC shutdown itself
left an admitted connection behind, the closing pump consumes its bounded queue
without attempting further client writes. Pump panic and worker timeout produce
a failing daemon exit.

Queue pressure is reported by distinct counters: queued-item eviction,
incoming-item drop, same-stream coalescing, and post-close rejection. A dropped
incoming partial is not described as an oldest-item eviction. Predicate callbacks
used by the internal priority/coalescing methods must be pure, non-blocking, and
non-reentrant because they execute under the queue mutex.

## Verification boundary

Deterministic tests cover publication wakeup, close wakeup, pre-close draining,
post-close rejection, priority eviction, incoming partial drop, coalescing, and
owned integration-harness shutdown. Workspace tests and Clippy exercise the
real daemon composition without microphone or transcript content.

This controlled change removes the fixed polling term from dispatch. It does
not establish microphone-to-model, model-to-IPC, webview repaint, CPU residency,
power, native p50/p95 latency, package, or clean-machine behavior. No performance
or release requirement passes from these tests.

## Consequences

- Idle event dispatch no longer wakes on a five-millisecond cadence.
- Publication and shutdown have one linearization boundary; a late producer
  cannot strand an event behind an exited pump.
- Closing the bus is terminal for that daemon instance; restart creates a new
  state and transport epoch.
- Clients disconnected during daemon shutdown recover from a fresh status
  snapshot, not from a promise that closing events were delivered.
- The queue remains bounded and lifecycle events retain priority over volatile
  partials.

## Alternatives rejected

- reducing the polling interval, which increases idle CPU wakeups;
- an unbounded channel or detached per-event writer;
- a condition variable with a stop flag outside the queue lock;
- joining the pump while allowing new publications; or
- treating controlled notification tests as native latency evidence.
