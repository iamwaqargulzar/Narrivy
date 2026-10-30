# Robustness audit, October 2026

Scope: `services/narrivyd`, `engines/` and `adapters/platform-linux`, on the
`cloud/robustness` branch from `main` at `1.0.0-rc.13`.

Looked for: `unwrap()`/`expect()` in production paths, waits with no bound
(channel `recv`, thread `join`, subprocess waits), threads never joined at
shutdown, and blocking calls on audio capture threads. Rules: `AGENTS.md`
("Rust rules").

## Fixed

| Where | Problem | Fix |
|---|---|---|
| `adapters/platform-linux/src/bounded_command.rs` (new) | No shared way to run a short helper with a deadline and read its output. | `output_within`, `status_within`, `wait_until`: poll, kill and reap at the deadline, read pipes on their own threads with a 4 MiB cap. Typed `BoundedCommandError`. 5 unit tests, including a hung helper and a grandchild that holds the pipe. |
| `echo_cancel.rs` `devices`, `source_offered`, `stop_leftovers` | `pactl … .output()` and `kill .status()` waited with no bound. `pactl` waits on the PipeWire server; a wedged server held the caller forever, also inside the 4 s start loop. | 3 s bound per call; the start loop gives each call at most what is left of its 4 s. |
| `pipewire_monitor.rs` `resolve_target_serial` | `pw-dump .output()` with no bound, at capture start. | 5 s bound; a timeout is `LiveCaptureError::Spawn`. |
| `narrivyd` `meetings.rs` `pactl` | `pactl get-default-* .output()` with no bound, on a request path. | 3 s bound. |
| `meetings.rs` remember-voice thread | Detached thread did `child.wait()` with no bound. | Killed after 5 minutes; the bound is what ends the thread. |
| `sounds.rs` `play` | Detached thread did `pw-play .status()` with no bound. | 5 s bound. |
| `voice.rs` `record` | `read_to_end` on `pw-record` stdout with no bound (voice training). A recorder stuck on a missing source never ended. | Reader on its own thread; recorder killed at the asked length + 10 s; typed `voice_recorder_failed`. |
| `assistant_builtin.rs` `run_command` | After the 120 s kill, `join()` on the pipe readers waited for every process still holding the pipe (a background job the command started). Only the shell was killed. | Kill the whole process group at the bound; readers hand back through channels with a 2 s drain bound, as the browser tool already did. |
| `assistant_builtin.rs` screen-once | `status()` on the screen helper with no bound; it waits for the user to answer the portal dialog. | 120 s bound. |
| `main.rs` signal listener start | On a start timeout, `join()` with no bound. | `join_thread_bounded` (now generic over the thread's result) with 2 s. |
| `policy.rs` test `capability_check_mic_never_promotes_helper_presence_to_signal` | Failed on any host without `pw-record`: "trusted pw-record helper missing" matched the check for an available helper. | The test now checks for "trusted pw-record helper at ". Test-only. |

## Compute discovery before `serve` (F-H7 leftover)

Kept in its place, and documented at the call in `main.rs`. The protocol has
no daemon-wide `starting` state: `StatusDto` has no readiness field and the
compute capability is only `compute.device=resolved` or
`compute.device=unavailable:no_qualified_plan`. Serving first would make the
first status claim "no plan" when one may still resolve, or need a new wire
token.

Clients are not shut out: the socket is bound first, so `connect` succeeds and
the handshake is answered when discovery ends. New test
`slow_compute_discovery_still_accepts_a_client`
(`services/narrivyd/tests/daemon_shutdown_signals.rs`) runs the real binary
with a 3 s discovery sidecar and connects in the middle of it.

Limit: each probe may take up to `PROBE_TIMEOUT` (10 s). A probe that runs to
that bound can outlast one client's 5 s handshake.

## `unwrap()` / `expect()`

None in production paths. The workspace denies `clippy::unwrap_used` and warns
on `expect_used`, which `-D warnings` makes an error. The three `.expect(b'…')`
calls in `adapters/platform-linux/src/capture.rs` are the JSON parser's own
method, not `Option::expect`.

## Kept on purpose

| Where | Why it stays |
|---|---|
| `kill` (1) calls in `meetings.rs` (`signal`, `stop_children`, `finish_live`) and `assistant_builtin.rs` (group kill) | `kill` sends a signal and exits; it waits on nothing. |
| `date` in `assistant_library.rs` and `assistant_tools.rs` | Reads the clock and exits; waits on nothing. |
| `curl` in `assistant_builtin.rs` `fetch_page` | `--max-time 25` bounds it; curl starts no children that keep the pipe. |
| `download.rs` | `--max-time` plus the cancellable `wait(child, cancel)`. |
| `meetings.rs` `work` (meeting processing) | Runs as long as transcription takes (meetings up to 4 h). Cancelled by stop, which kills the worker by pid. A time bound would cut long meetings. |
| Recorder pump `join()` in `meetings.rs` | Runs after `stop_children`, which kills each recorder within 5 s; the pipe then closes. |
| `voice.rs` `Worker::start` / `send` | Run on a `with_worker` thread; the caller waits with `recv_timeout(bound)`. A stuck worker keeps the lock and later callers give up on `try_lock` at their bound. |
| `voice.rs` `PlainMic::finish` join | The recorder is killed first, which closes the pipe the reader is on. |
| `reply.recv()` on assistant replies (`assistant_library.rs`, `assistant_tools.rs`, `meetings.rs`) | The assistant thread answers every job or drops its sender. It bounds the model load (900 s) and each streamed piece (120 s). |
| `live_preview.rs` `finish` join | The thread checks `stop` before and after each decode, so the join waits for at most the decode in progress (one short window). A deadline here would need `session.rs` changes, which were out of scope. |
| `refine.rs` holder `released.recv()` | That wait is the thread's lifetime: it ends when the refiner is dropped. |
| `wayland_selection.rs` `Drop` join | The worker checks for shutdown every 250 ms. |
| `engines/transcribe-cpp` `resident.rs` actor loops (`pending.recv()`) | Actor loops that end when every sender is dropped; their joins go through `shutdown_until` and only join finished threads. |
| `engines/openvino` worker binaries | Separate processes; their lifetime is their stdin. The daemon owns and kills them. |
| `#[cfg(test)]` pause hooks (`portal.rs`, `session.rs`, `resident.rs`, `recovery.rs`) | Test-only. |
| Compute probes (`compute_probe.rs`) | Already bounded and drained (F-H7 fix, 4b2c651). |

## Threads not joined at shutdown

Joined within the shared shutdown deadline (ADR-0069): the signal listener,
the portal actor and its event pump, supervised session workers, the resident
ASR owner, and the event pump.

Not joined, by design: process-lifetime threads that hold no state that must
be flushed — the assistant thread, assistant routines, echo/voice/refine
warm-up and holder threads, the hold-key reader, the sound cue thread and the
helper pipe readers. They end with the process. The cue thread, the
remember-voice thread and the pipe readers now also end on their own bound.

Open (not fixed here): a meeting being recorded when the daemon stops is not
stopped. `pw-record` writes into a daemon pipe, so it ends on `EPIPE` once
the daemon is gone; the live worker and screen helper are not reaped by the
daemon. A bounded meeting stop in the shutdown sequence is a follow-up.

## Audio capture threads

No blocking calls found. The streaming producer in `recovery.rs` reads a
non-blocking pipe and hands chunks over with `try_send` (a full queue is a
typed `Backpressure` terminal). `pipewire_monitor.rs` `parse_loop` takes the
ring and jitter locks with `try_lock` only, and a test forbids `eprintln!` in
it. The short `CAPTURE_READ_POLL` sleep on `WouldBlock` is a poll, not a wait
on another component.
