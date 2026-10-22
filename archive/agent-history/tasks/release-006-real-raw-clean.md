# Release task 006: real microphone Raw/Clean policy

DeepSeek V4.1 Flash at provider reasoning=max performs implementation; Codex independently reviews/tests/accepts. Read AGENTS.md, SOURCE_OF_TRUTH.md, BUILD_TARGET.json, canonical architecture/catalogs, API stability and current P11 report. Use apply_patch. No commits/pushes, host configuration/package/service changes, secret access, fake evidence, requirement promotion or security-gate weakening.

Implement actual built-in Raw and Clean modes for the real installed microphone/ASR session composition, not local UI state or mock aliases. Existing strict mode accepts only linux-mic-inbox, session admission/capture-ready timing depends on two hardcoded recovery IDs, and the real worker always constructs DaemonCorrector(FullCorrect). Raw must use CollapseOnly through the same real owned capture/ASR/durable finalization/privacy/delivery path; Clean uses existing deterministic FullCorrect. Resolve correction once from the admitted pipeline and use it consistently for batch and streaming. Keep pipeline IDs, advertised enabled state, selected pipeline, recovery journal and partial events truthful. Do not bypass installed artifact/model/live-worker readiness, strict single-session admission, delayed Listening until actual capture readiness, cancellation, bounded cleanup or Inbox recovery. Unknown and mismatched lanes still reject before capture.

Document the intended production-vs-historical mock pipeline migration in a reviewed ADR; choose a minimal explicit identity strategy consistent with canonical catalogs and portable pipeline contracts. Do not silently accept legacy mock lanes in production. No copying backend/policy into frontend: desktop task 004's advertised pipeline selector should consume the new real mode entries. Update stale comments and focused tests asserting the old single-mode limitation where the intended product behavior changes, preserving genuine fail-closed tests.

Regression-test real composition admission and exact correction outcomes for Raw/Clean, including Unicode/multiline, recovery identity, batch/streaming parity, unavailable installed model/worker rejection and active-session safety. Mock/controlled fixtures are deterministic correctness tests, not native mic or release qualification. Run focused default and resident-feature tests, strict Clippy, frontend tests/build if changed. Keep scope to modes, no cosmetic redesign or new runtime/model. Stop after concise exact checks and remaining native delivery/activation/long-capture blockers.

Codex source-review map (recheck current source rather than trust line numbers):
`daemon/policy.rs::CorrectionStrategy::resolve` already owns deterministic
mode resolution and `builtin_pipeline_defs` owns definitions. In
`daemon/session.rs`, `select_pipeline` and `advertised_pipelines` own actual
strict admission/advertising; `begin_session` hardcodes recovery IDs when
deferring Listening until capture readiness, and the real worker closure
constructs `DaemonCorrector(FullCorrect)` regardless of admitted intent.
Capture the admitted mode once before launching owned work, and audit its
use by both resident finalization and batch orchestration. Policy capability
advertising also hardcodes the recovery identity. Keep journal, capture-ready
timing, partial identity and actual selectable IDs aligned through the same
resolved daemon policy; adding display entries alone cannot complete this task.
