# Token-efficient engineering workflow

This workflow reduces repeated context and verification without weakening
Narrivy's release requirements.

## Live context

Start with `SOURCE_OF_TRUTH.md`, `BUILD_TARGET.json`, the compact
`agent/STATE.json`, the active phase specification, and the files named by the
current blocker. Read `agent/DECISIONS.md`, `agent/RESEARCH_LOG.md`, phase
reports, and `agent/history/` by targeted search only. Never dump those files
into context wholesale.

Use `rg` to locate a symbol or decision, then read the smallest useful line
range. Use `git diff --stat` and path-scoped diffs before a full diff. Successful
commands should be quiet; redirect verbose logs to ignored artifacts and return
only the command, status, counts, and failure excerpt.

## One work item

1. Name one observable blocker and its acceptance condition.
2. Trace only the affected call path and existing reusable mechanisms.
3. Reproduce with the smallest meaningful check.
4. Make the smallest correct diff. Preserve security, privacy, accessibility,
   recovery, and evidence requirements.
5. Rerun the reproducer. Add one regression check per distinct behavior; do not
   duplicate implementation details in tests.
6. Review the bounded diff. Run the affected-subsystem checks once.
7. Update the compact state and create a local checkpoint when authorized. Do
   not push without the owner's explicit push-time signal.

Do not widen into unrelated cleanup. Record a newly discovered independent
problem as the next work item.

## External worker evidence contract

For delegated Grok tasks, repository files and successful tool results from the
current run are the only evidence of state. The worker must not claim a file was
read or changed, a command ran, a test passed, or an action completed unless the
corresponding tool call succeeded in that run. Planned, narrated, skipped,
blocked, mocked, partial, and failed actions are not completion evidence.

Give the worker one bounded item with explicit files, acceptance conditions, and
focused checks. Use a fresh session, disable web and subagents unless the task
requires them, and keep the sandbox at `workspace` or `read-only`. Afterward,
Codex inspects the actual diff and reruns the focused checks independently. The
worker summary is a navigation aid, never acceptance evidence. Restart with a
clean task package when the objective changes or the session begins to drift.

Use `scripts/run_grok_task.py` and
`docs/maintainers/agent/grok-orchestrator.md` for unattended tasks. Grok 4.7 with
`xhigh` reasoning is the current worker default. Prefer fewer, bounded turns
over broad prompts because maximum reasoning can consume substantial time and
tokens before editing. The 24-turn ceiling is a safety margin: task prompts
must direct the worker to edit early, run only the focused checks, and finish
as soon as those checks pass.

## Verification cadence

- **Edit loop:** one focused reproducer and touched-language formatting.
- **Checkpoint:** affected packages, direct integration boundary, strict lint,
  and manifest verification when shipped files changed.
- **Phase:** broad workspace/frontend/Python/security checks once, followed by
  the phase gate if its inputs changed.
- **Release:** build once from a clean identity, then qualify that exact artifact
  and run the release gate. Development-build checks cannot substitute for it.

Never rerun a known-blocked gate merely to reconfirm the same blocker. Rerun it
after the relevant source, package, evidence, environment, or manifest identity
changes.

## Ponytail policy

Ponytail defaults to `lite` for routine implementation. Use `review` on a small
diff when looking for unnecessary code or reuse opportunities. `full` is an
explicit choice for a bounded feature with a clear over-building risk. Do not
use `ultra` for Narrivy release work. Turn Ponytail `off` while executing an
exact qualification or evidence procedure.

Ponytail never overrides repository rules. Minimal means the least code and
verification that fully satisfy the current requirement, including mandated
failure behavior, diagnostics, accessibility, documentation, security, native
evidence, and packaging.

## State discipline

`agent/STATE.json` is a current snapshot, not an append-only journal. Keep only:

- active phase and gate status;
- current work item and acceptance condition;
- latest accepted checkpoint;
- open blockers and exact next action;
- last gate identity/result.

Move completed chronology to dated files under `agent/history/`. Handoffs should
fit on one screen and point to history rather than repeating it.
