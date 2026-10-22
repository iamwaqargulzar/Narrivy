# Release task 007 review fix: durable source-end truth and bounded race harness

DeepSeek V4.1 Flash at provider reasoning=max implements this narrowly researched
follow-up. Codex owns the design, source review, independent tests and acceptance.
Read `AGENTS.md`, `SOURCE_OF_TRUTH.md`, `BUILD_TARGET.json`,
`docs/11-adrs/ADR-0071-stop-controlled-bounded-dictation-capture.md`, and the
current implementations before editing. Use `apply_patch`. Do not commit, push,
touch the host, fabricate evidence, promote requirements, weaken security, or
redesign unrelated delivery code.

## Confirmed defects to repair

1. `CaptureEnd::SourceEnded` is documented as durable transaction truth which
   fails closed before ASR/correction/delivery. The production batch adapter
   currently turns it into `Err("capture_source_ended")` and clears audio before
   the portable slice can journal the reason. The streaming adapter likewise
   converts a short EOF into an error before returning its end summary. In
   contrast, the portable batch slice currently accepts a controlled
   `CapturedAudio { end: SourceEnded }` and continues into ASR. These paths
   contradict each other and ADR-0071.

   Make the boundary coherent: when at least one valid frame was captured and
   the recorder ended early, the adapter must return the typed `SourceEnded`
   result/summary to the portable slice; the slice must persist `capture_end`
   and then fail terminal with `capture_source_ended` before any new ASR,
   correction or delivery side effect. In streaming, already-fed ASR state must
   be cancelled/finished through the existing owned cleanup path and no final
   text may commit. Preserve zero-frame/invalid-frame failure behavior, sample
   accounting, cancellation precedence, and process/thread reaping. Update
   controlled fixtures that used `SourceEnded` merely as a generic successful
   end to an actually successful reason.

2. The test
   `recovery::tests::cancellation_after_receive_discards_chunk_and_reaps_child`
   can wait forever on `arrived.wait()` when the spawned recorder or worker
   thread fails before reaching `StreamControlBoundary::AfterReceive`. This was
   reproduced indirectly under a 16-thread full-suite run: a concurrent helper
   spawn failed, and this barrier design has no terminal/error wakeup. Replace
   this specific stream-boundary test coordination (or the shared test-only
   primitive if cleanly bounded) with deterministic bounded coordination that
   reports an error instead of hanging. Both sides must always be releasable;
   test failure must not strand a thread or recorder. Do not add sleeps as race
   evidence and do not change production timing to make a test pass.

## Required regressions

- Batch `SourceEnded` with valid audio journals `capture_end=source_ended`,
  reaches FailedTerminal with `capture_source_ended`, and records zero ASR,
  correction and delivery calls.
- Streaming early EOF after contiguous audio journals the same typed reason,
  fails terminal, cleans up the ASR/capture owners, and never commits final text.
- Normal FinalizationRequested, EndpointDetected and exact SafetyLimit behavior
  remains successful and byte/sample exact.
- The cancellation-after-receive race remains deterministic and process-clean,
  but any setup/spawn failure terminates within a small documented test bound
  rather than hanging indefinitely.

Run only focused checks during implementation: formatter; the affected
`narrivy-platform-linux` recovery tests; affected `narrivy-core` tests; relevant
resident/default `narrivyd` tests if fixtures change; strict Clippy for changed
Rust packages. Do not repeatedly stress the entire workspace. End with a concise
file/change/check report and any unresolved finding.
