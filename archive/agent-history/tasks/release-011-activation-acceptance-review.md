# Release task 011: resolve activation acceptance findings

DeepSeek V4.1 Flash at provider reasoning=max implements; Codex independently
reviews and tests. Read authoritative repository rules and current P11 report.
No commits/pushes, host services/installations, secrets, user content in logs,
relaxed integrity/readiness checks, or requirement promotions. Use apply_patch.

Run only after task 010 terminates. Preserve its correct product work. Resolve
only remaining independently recorded activation findings; skip any that task
010 demonstrably fixed. Remove every temporary direct-spawn bypass and
unreachable-code allowance. Keep PR_SET_PDEATHSIG enabled.

The new short-lived-owner regression passed even with direct spawning restored.
Require a bounded actual worker protocol operation after the requesting thread
exits; verify the same condition for a replacement created by a temporary
recovery thread. Prove regression failure against the original direct-spawn
behavior, restore the fix, and rerun. Distinguish an asynchronous observation
race from the actual packaged exit cause; do not assert causality from kernel
documentation alone.

The persistent spawn owner must have explicit thread lifetime, termination and
failure ownership. Synchronous rendezvous send/receive cannot silently exceed
the caller's absolute deadline or ignore cancellation. Keep a child whose
response races cancellation owned until reap, and do not declare shutdown
complete while an owner thread or generation is still active. Document any
changed API/security contract in an ADR. Do not increase systemd's 3s budget.

Rerun focused default/resident tests and strict Clippy. If product binaries
changed, rebuild a fresh non-overwriting unqualified candidate, then run the
strict actual Ubuntu lifecycle checker: awaiting-model -> verified offline
publication -> surviving live worker ready -> usable advertised pipeline ->
bounded TERM/restart/remove/purge preserving the exact model. Preserve narrow
typed failure facts and remove only your owned containers. Do not claim native
capture/delivery or release qualification from this test. Stop after one
concise report with concrete evidence and remaining blockers.
