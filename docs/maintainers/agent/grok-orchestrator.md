# Grok Orchestration Entrypoint

`scripts/run_grok_task.py` is the repository-local, non-interactive way to run
one Grok 4.7 worker task and collect a machine-readable result. It wraps exactly
one Grok CLI invocation with `--prompt-file`, a grounded system-prompt override,
a schema-constrained completion receipt, pinned safe defaults, bounded
task/output sizes, a timeout that kills the child process group, and an atomic
JSON result. Implementation runs can also require an observed Git workspace
change, so prose-only answers cannot be accepted as implementation.

## Division of responsibility

| Role | Owns |
|---|---|
| **Grok** (`grok-4.7` via the Grok CLI) | Implements the task described by the task file. It may modify files inside the workspace under the selected sandbox. |
| **Codex** | Orchestrates (writes the task file, invokes this script, reads the JSON), reviews the diff, runs and keeps tests, and accepts or rejects the result. |
| **This script** | Transport and bookkeeping only. It validates local inputs, pins the worker configuration, bounds resources, and records the outcome truthfully. It never edits product code, commits, pushes, or records acceptance. |

The task file is the whole instruction: headless has no interactive follow-up.
Split multi-step work into one task per invocation.

## Usage

```sh
python3 scripts/run_grok_task.py --task-file agent/tasks/T-001.md \
    --result-file artifacts/grok-runs/T-001.json --require-change
```

The script prints exactly one line to stdout: the absolute path of the JSON
result. Worker stdout and stderr are captured into that JSON, never forwarded to
the orchestrator's stdout.

Common variations:

```sh
# bound the whole run to 15 minutes and retain at most 4 MiB per stream
python3 scripts/run_grok_task.py --task-file task.md \
    --timeout-seconds 900 --max-output-bytes 4194304

# validate inputs and print the planned invocation without running anything
python3 scripts/run_grok_task.py --task-file task.md --dry-run
```

### What is executed

From `<workspace>` (the repository root unless `--workspace` overrides it):

```text
<grok> --model grok-4.7 \
    --reasoning-effort xhigh \
    --sandbox workspace \
    --permission-mode bypassPermissions \
    --disable-web-search \
    --no-subagents \
    --no-plan \
    --system-prompt-override <grounded-worker-policy> \
    --json-schema <completion-receipt-schema> \
    --output-format json \
    --max-turns 24 \
    --cwd <workspace> \
    --prompt-file <task-file>
```

Pinned defaults:

- model `grok-4.7`
- reasoning `xhigh` (overridable: `low`, `medium`, `high`, `xhigh`)
- sandbox `workspace` (overridable: `workspace`, `read-only`)
- permission mode `bypassPermissions` (unattended; not overridable)
- web search/fetch disabled
- subagents disabled
- plan mode disabled for the already researched, bounded worker task
- the grounded system prompt requires inspection before claims, real file edits,
  truthful blocker outcomes, focused checks, and an immediate stop after success
- the final response is constrained to `COMPLETED`, `BLOCKED`, or
  `NO_CHANGE_REQUIRED` plus changed files, checks, and remaining work
- JSON output
- finite `--max-turns` (default 24)
- fresh session: `--prompt-file` starts a single-turn run; `--resume` and
  `--continue` are never passed

The task is handed over as a file path, never as process-argument text, so
secrets in the task are not visible via `ps`. The result JSON records the exact
argv with that path represented as `[TASK_FILE]`. Only the task byte count and
SHA-256 are stored — never the text.

stdin is `/dev/null`. The child runs in its own process group so a timeout can
terminate the whole group (SIGTERM, then SIGKILL after an internal grace
period).

The child environment is inherited and is never written into the result.

### Flags

| Flag | Default | Meaning |
|---|---|---|
| `--task-file PATH` | required | UTF-8 task text handed to the worker via `--prompt-file`. |
| `--result-file PATH` | `<workspace>/artifacts/grok-runs/grok-<utc>-<id>.json` | JSON result path. Use a unique path per run. |
| `--workspace PATH` | repository root | Worker cwd and `--cwd`; must be a directory. |
| `--grok PATH\|NAME` | `grok` | CLI to invoke. |
| `--timeout-seconds N` | `3600` | Whole-run bound (must be > 0). |
| `--max-task-bytes N` | `262144` | Task files larger than this are rejected. |
| `--max-output-bytes N` | `8388608` | Bytes retained per stream (stdout/stderr). Overflow is truncated; the pipe is still drained. |
| `--max-turns N` | `24` | Grok `--max-turns` bound (must be > 0). |
| `--reasoning-effort {low,medium,high,xhigh}` | `xhigh` | Reasoning effort. |
| `--sandbox {workspace,read-only}` | `workspace` | Filesystem sandbox profile. |
| `--require-change` | off | Require a real Git workspace change and a valid `COMPLETED` receipt for implementation work. |
| `--dry-run` | off | Validate and print the redacted plan; run nothing, write nothing. |

## Evidence contract

Repository files and the JSON result of a run that actually executed are the
only evidence of worker state. The script records what happened; it does not
assert that the task was implemented correctly. Mock-only success, a skipped
check, or a planned invocation is not a pass.

Treat the result JSON as evidence, not as acceptance:

- `ok` / `outcome` / `exit_code` / `timed_out` describe the guarded worker run.
- With `--require-change`, tracked diffs and untracked file contents are
  fingerprinted before and after the run. Exit zero with no change or without a
  valid `COMPLETED` receipt becomes `unverified_completion`.
- `task.sha256` and `task.bytes` identify the instruction that was run.
- `grok.argv` is the exact invocation with the prompt path elided.
- `response` and `diagnostics` are bounded, possibly truncated worker output.
- Environment values are never recorded.

## Safety boundaries

- Task text and secrets are not placed on the command line (`--prompt-file`).
- A previous Grok session is never resumed.
- Web search/fetch and subagents are disabled for the invocation.
- The sandbox is `workspace` or `read-only`; other profiles are refused.
- The workspace path must be an existing directory.
- The task must be a regular UTF-8 file, bounded, nonempty, and NUL-free.
- Numeric limits must be positive and finite.
- Child stdout/stderr are captured, never forwarded to the orchestrator.
- Timeout kills the entire child process group, not only the launcher PID.
- The result file is written atomically and chmod'd to `0600`.
- `bypassPermissions` is required for an unattended run; the sandbox is the
  remaining filesystem fence. Do not widen it from this script.

## Exit codes and outcomes

Exit codes:

| Code | Meaning |
|---|---|
| `0` | The worker completed (`outcome: completed`). |
| `1` | The worker failed; its own exit code is propagated when it is `1..255`. |
| `2` | Usage error or invalid local input (flags, workspace, task file). |
| `3` | Grok missing or unstartable. |
| `4` | Timeout. |
| `7` | The JSON result could not be written. |

`outcome` is authoritative whenever a worker exit code happens to fall in
`2..7`. The result `exit_code` field always carries the worker's real code (and
`signal` its terminating signal, if any).

Outcomes: `completed`, `unverified_completion`, `child_failed`,
`max_turns_reached`, `timed_out`, `spawn_failed`, `grok_not_found`,
`invalid_task_file`.

Usage errors raised while building the request (bad flag values, a workspace
that is not a directory) print to stderr and write no JSON. After the request is
built — including a missing task file or a missing Grok executable — a JSON
result is still written. Dry-run validates and prints a redacted planned JSON
object; it does not start Grok and does not write a result file.

A process exit 0 is `completed` unless `--require-change` finds no real workspace
change or no valid `COMPLETED` receipt; either case is
`unverified_completion`. A nonzero process exit is `child_failed`, except that
Grok's explicit turn-budget diagnostic is `max_turns_reached`. A timeout is
`timed_out`. Missing or unstartable Grok fails as `grok_not_found` or
`spawn_failed`.

## Result JSON

Fields (schema version 1, written atomically with mode `0600`):

- `ok` / `outcome` / `exit_code` / `signal` / `timed_out` / `error` — the outcome,
  with `error` carrying a one-line summary when the run failed.
- `response` — the worker's final answer (stdout), decoded as UTF-8 with
  replacement, bounded by `--max-output-bytes`.
- `receipt` — the validated schema result from Grok Build's canonical
  `structuredOutput` field (with a legacy text fallback), or `null` when invalid.
- `workspace_change` — whether proof was required and observed, before/after
  fingerprints, and the files whose contents changed during the run.
- `diagnostics` — the worker's reasoning and error text (stderr), bounded the
  same way.
- `output_overflow` — `stdout_truncated`, `stderr_truncated`, and the total
  bytes each stream produced (`stdout_bytes` / `stderr_bytes`). Truncation keeps
  draining the pipe so the child is not blocked on a full buffer.
- `stream_drain_complete` — `false` only if a pipe stayed open after the process
  group was killed; captured output may then be incomplete.
- `task` — file, byte count, and SHA-256 (never the text).
- `grok` — requested launcher, resolved path, and the exact argv with the prompt
  path represented as `[TASK_FILE]`.
- `settings` — model, reasoning, sandbox, permission mode, web/subagent/plan
  flags, rules profile, output format, max turns, and `fresh_session`.
- `workspace`, `limits`, `run_id`, `started_at`, `finished_at`,
  `duration_seconds`, `orchestrator`.

Environment values are not recorded. Result files live under `artifacts/` and
are not meant to be committed.

## Tests

`tests/governance/test_run_grok_task.py` drives a generated fake `grok`
executable; it makes no network calls and writes only inside a temporary
directory.

```sh
python3 -m unittest tests.governance.test_run_grok_task -v
```

## Codex review duties

1. Read the plan/diff produced by the worker; do not trust the summary alone.
2. Check `ok`, `outcome`, `exit_code`, `timed_out`, and `output_overflow` in the JSON.
3. Verify `task.sha256` matches the task file you intended to run, and that
   `grok.argv` used `--prompt-file [TASK_FILE]` with the pinned defaults and no
   `--resume` / `--continue`.
4. Run the focused tests and the relevant gate yourself; mock-only success is not
   acceptance.
5. Accept, fix, or reject explicitly — the script never does.
