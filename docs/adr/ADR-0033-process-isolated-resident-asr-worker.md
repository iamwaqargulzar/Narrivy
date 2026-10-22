# ADR-0033 — Process-isolated resident ASR worker

**Status:** Recovery 4B controlled implementation complete (2026-08-28);
non-release and not native microphone qualification

## Context

Recovery 4A removed the fixed capture window and capture-file round trip, but
the strict path still launched `transcribe-cli` and loaded the GGUF model for
every utterance. Content-free measurements showed five cold sidecar decodes of
six seconds of silence at 356.9–520.0 ms, while repeat runs in one loaded CLI
process averaged 267.2 ms. The largest avoidable latency was model/process
startup.

Linking native inference directly into `narrivyd` would keep the model resident,
but a wedged C/C++ call could not be force-terminated within Narrivy's bounded
shutdown contract. A permanently resident sidecar protocol provides both
model reuse and an OS-enforced cancellation boundary.

## Decision

1. `narrivy-transcribe-worker` is a separate daemon-owned child process. It
   links the exact `transcribe-cpp`/`transcribe-cpp-sys` 0.2.2 safe-wrapper/sys
   pair with default features disabled and a static CPU build.
2. The worker loads one explicit GGUF once with strict `Backend::Cpu`. Before
   readiness it verifies native version/ABI through the upstream wrapper,
   concrete CPU backend, 16 kHz native sample rate, and cancellation support.
3. stdin/stdout carry a private versioned binary protocol: fixed 28-byte
   headers, opaque request identity, at most 30 seconds of mono f32 PCM, and at
   most 1 MiB of UTF-8 result. Reserved bytes, unknown operations/statuses,
   non-finite samples, identity mismatch, truncation, and oversize fail closed.
   PCM crosses each process boundary through a fixed 16 KiB transfer buffer;
   no additional utterance-sized byte representation is constructed.
   Protocol v3 readiness includes one bounded capability byte populated from
   the loaded model. Unknown/malformed capabilities fail closed, and restart
   must reproduce the initial capability set.
4. Native logging is disabled before model load. stderr is null. Neither side
   writes audio or transcript payload files or includes payload in errors/logs.
5. The parent owns child and pipes, makes them nonblocking, applies one absolute
   decode deadline, and observes the transaction cancellation signal during
   both write and read. Cancellation/timeout/protocol/I/O failure kill and reap
   the worker within two seconds; cleanup failure supersedes a clean-cancel
   claim.
6. One resident worker admits one request. Contention fails immediately as
   `asr_busy`; no unbounded audio queue is permitted. After cancellation or an
   unhealthy worker, the next request may spawn a fresh worker from the same
   explicit executable/model pair.
7. `NARRIVY_TRANSCRIBE_RUNTIME=resident` is explicit and requires a daemon
   compiled with `asr-transcribe-resident` plus a non-symlink executable named
   by `NARRIVY_TRANSCRIBE_WORKER`. `sidecar` remains the explicit rollback.
   There is no mock or sidecar fallback on resident failure.
8. The daemon loads and handshakes its resident worker before accepting IPC.
   Status reports `runtime=resident-worker`; readiness is never inferred only
   from a pathname at production startup.

## Supply-chain decision

Published version 0.2.1 was rejected during implementation because its bundled
native code reported 0.2.2, causing the wrapper's pre-1.0 version gate to fail.
Narrivy did not weaken that gate. The matching published 0.2.2 packages are
exact-pinned in `Cargo.lock`, MIT licensed, and their crates.io checksums are
recorded in `BUILD-NOTES.md` and `THIRD_PARTY_NOTICES.md`. CI compiles/tests the
optional worker and daemon feature explicitly.

## Consequences

- Model startup moves to daemon startup rather than the Stop-to-delivery path.
- Three controlled one-second silent requests through one real resident model
  decoded in 189.5, 199.8, and 243.5 ms after a 90.5 ms startup handshake on
  this host. Only byte counts and hashes were observed. These measurements are
  exploratory, not release latency evidence.
- This remains final-only ASR after Stop. No partial transcript, live streaming,
  VAD, editor insertion, microphone qualification, or release claim is added.
- Upstream 0.2.2 streaming is model-dependent. The current Whisper tiny model
  is final-only, and Narrivy has not implemented the upstream stream API for
  streaming-capable Moonshine/Parakeet variants. It must not simulate partials
  or advertise streaming before that end-to-end contract exists.
- Daemon status distinguishes native model support from product composition:
  `model-streaming=supported|unsupported` is model attestation, while
  `streaming=disabled:not_composed` remains the product truth.
- ADR-0035 extends the private worker contract with model-gated native stream
  operations while retaining this process-isolation and cleanup boundary.
- Packaging must install both binaries together, configure the worker path,
  validate clean-machine static/native dependencies, include licenses/SBOM,
  and prove upgrades/rollback before release.
- Unexpected daemon death closes the worker's stdin; the worker exits on EOF.
  Normal daemon ownership additionally sends shutdown and boundedly reaps.

## Required verification

- protocol round-trip plus malformed/oversize rejection;
- repeated requests reuse one worker and create no payload files;
- concurrent request is a typed immediate busy failure;
- cancellation kills/reaps and a later request creates a healthy worker;
- symlink worker and non-finite/wrong-layout audio are rejected;
- real-model content-free startup/decode/shutdown produces no child leak;
- strict daemon status names the resident runtime and remains behaviorally
  unqualified;
- focused tests, strict Clippy, workspace checks/tests, boundary validators,
  license/source/bans policy, and fail-closed release governance remain intact.

## Deferred decisions

- model-worker pools for real concurrent sessions;
- streaming-capable model selection, worker protocol, orchestration, and
  partial-result UX;
- VAD/endpointer ownership and latency/accuracy qualification;
- release packaging, systemd/process supervision, crash-loop backoff, and
  worker executable integrity binding.

## Upstream references

- Safe Rust lifecycle/threading contract:
  <https://github.com/handy-computer/transcribe.cpp/tree/v0.2.2/bindings/rust/transcribe-cpp>
- Public C API/version contract:
  <https://github.com/handy-computer/transcribe.cpp/blob/v0.2.2/include/transcribe.h>
- Published Rust package:
  <https://crates.io/crates/transcribe-cpp/0.2.2>

## 2026-09-21 dependency refresh

The exact wrapper/sys pair is now 0.2.3 at upstream commit
`63a44d9239d610b3908e8a66b384924cd4a77217`. The public C header retains the
0.2 API surface. Narrivy keeps the same process-isolation and fail-closed
version checks and must regenerate native/package evidence for this new source.
