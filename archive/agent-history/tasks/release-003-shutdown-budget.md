# Release task 003: one bounded daemon shutdown budget

DeepSeek V4.1 Flash at maximum reasoning implements; Codex independently reviews and accepts. Read AGENTS.md, SOURCE_OF_TRUTH.md, BUILD_TARGET.json, architecture map, component boundaries, current P11 report and scripts/README.md. Use apply_patch. No commits/pushes, user-service changes, secret access, global changes, fabricated evidence or requirement promotions.

Repair the release-blocking cumulative shutdown deadline in narrivyd. Its packaged systemd service has TimeoutStopSec=3s, but IPC connection cleanup, portal cleanup, supervised sessions, resident engine cleanup, retries and pump joins currently use independent budgets or unbounded joins. Preserve cleanup ownership, admission closure, cancellation and truthful failure; do not merely enlarge TimeoutStopSec or detach live work and declare success. Introduce one monotonic total budget, established when shutdown begins (including IPC/signal handling, not only after serve returns). Each cleanup operation must spend the remaining shared budget and still close/cancel every owner even if exhausted. Avoid sequential waits preventing resident cancellation while a session waits on that resident owner. Inspect existing bounded shutdown APIs and use the smallest correct architectural change; no new dependencies or unsafe. Any unavoidable API change must be documented in an ADR and its callers migrated.

Add meaningful deterministic regressions for cumulative exhaustion and owner cancellation and real daemon subprocess regressions for active supervised work plus TERM and INT. Existing idle signal/socket restart tests must keep passing. If active resident integration needs a controlled fixture, explicitly distinguish it from real model/platform qualification. Keep failure sticky for ordinary cleanup failures; final confirmed reap may clear a timeout only under the existing semantics. Do not hide outstanding live workers or process generations. Run focused default and asr-transcribe-resident tests and strict Clippy. Stop after a concise report, changed files, exact checks and unresolved risks; do not repeatedly review unchanged code.

Recheck task 011's new `engines/transcribe-cpp/src/resident.rs::SpawnAnchor`.
Skip findings its final source demonstrably fixed: current rc3 activation
passes actual strict Ubuntu provisioning/ready worker/TERM/restart/remove/purge,
and current source retains abandoned children and bounds join completion.
Still audit concurrent `reap_retained_until` calls: its blocking mutex acquisition
can spend another caller's cleanup window before checking this deadline.
Audit Drop after exhausted retirement/reap: live child/thread handles must
remain under an explicit fallback owner and failure must not be concealed.
Independent review found abandoned-child cleanup discards the `Child` on
`terminate_group_and_reap` failure, then decrements in_flight and returns to
its parked loop. Parent-death is not a cleanup proof while that owner remains
alive; shutdown can falsely see idle with an unreaped process. Retain each
failed child under an explicit cleanup owner and sticky typed failure until
reap is confirmed. Add a deterministic forced-reap-failure regression and
verify recovery/final cleanup does not lose or conceal the child.

The retirement latch is marked just before the owner returns; the following
`JoinHandle::join` remains unbounded. Prove actual thread completion before
joining, preserve the handle on deadline exhaustion, and propagate truthful
failure. `stop`/Drop retries and abandoned-reap waits currently establish
fresh two-second budgets; bring them under the same service shutdown deadline
without detaching owners or relying on systemd to hide failed cleanup. A
normal-case package TERM pass alone does not prove these failure paths.
