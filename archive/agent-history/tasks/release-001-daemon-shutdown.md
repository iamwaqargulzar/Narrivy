# Release task 001: verify packaged daemon shutdown correctness

You are the primary implementation worker, DeepSeek V4.1 Flash at maximum reasoning. Codex reviews and accepts your work independently.

Read AGENTS.md, SOURCE_OF_TRUTH.md, BUILD_TARGET.json, and the active P11 report. Do not weaken gates, modify historical artifacts/evidence, push, commit, read credentials, or change user configuration. Use apply_patch. Do not spawn other agents.

Bounded scope: review the newly added TerminationSignalListener in services/narrivyd/src/main.rs against the actual installed Tokio API. Add real subprocess integration tests to services/narrivyd/tests that launch the daemon in isolated temporary XDG runtime/state/cache directories, wait for actual CLI or authenticated IPC readiness, deliver SIGTERM and SIGINT, assert bounded successful exit and socket removal, and restart on the same socket/state. Ensure all test children are reaped on failures. No microphone/audio/transcript or user's daemon/service is touched. Use safe std APIs and existing dependencies; no new unsafe in narrivyd. Correct any demonstrated bug in this signal bridge, including unexpected listener failure leaving the serve loop alive. Keep changes narrowly scoped and tests meaningful. Inspect whether the packaged TimeoutStopSec=3s is consistent with existing cumulative shutdown bounds; report this without silently weakening service policy.

Run focused tests and strict Clippy for narrivyd, default and resident feature if practical. Do not run repeated broad suites. Stop after this assignment; final report: exact changed files, commands/results, demonstrated defects fixed, unresolved risks. Do not spend extra rounds on cosmetic cleanup.
