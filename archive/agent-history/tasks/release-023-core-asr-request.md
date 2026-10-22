# Release 023 — thread typed ASR request through batch recovery

Work on exactly one bounded implementation task in `<repo>`.

## Objective

In `crates/narrivy-core/src/recovery.rs`, add an explicit-request entry point for `RecoverySlice` so production composition can pass `narrivy_asr_api::AsrRequest` into the existing one-shot ASR port. Existing `run` and `run_with_timing` behavior must remain source-compatible and must use `AsrRequest::default()`.

## Required behavior

- Add the smallest public method(s) necessary to run a `RecoverySlice` with `&AsrRequest`; include a timing-capable form only if required to avoid duplication.
- Route the request through the existing `run_inner` and call `OneShotAsrPort::recognize_with`.
- Keep all current lifecycle, cancellation, journal, privacy-policy, recovery, and delivery behavior unchanged.
- Do not add a field to `RecoverySlice`; there are many callers and the request belongs to a run.
- Add one focused regression test in the same file using a small spy/fake ASR port that proves a non-default request reaches `recognize_with`. The test must not log or assert secret/context hint text. Prefer an explicit language request over context text.
- Existing callers and tests must continue to compile and use the default request.

## Scope

You may edit only:
- `crates/narrivy-core/src/recovery.rs`

Do not edit state, traceability, evidence, reports, task files, Cargo manifests, generated files, or any other source file. Do not commit.

## Focused verification

Run only:

```bash
cargo fmt --check -- crates/narrivy-core/src/recovery.rs
cargo test -p narrivy-core recovery --lib
cargo clippy -p narrivy-core --lib -- -D warnings
```

Fix failures caused by your change. Report exact commands and outcomes from tool output. Do not run workspace-wide, governance, release, packaging, or phase gates.

Read only the target file and the directly referenced `narrivy-asr-api` request/trait definitions. Begin editing early; do not spend turns surveying unrelated code. Repository files and successful tool output are the only completion evidence.
