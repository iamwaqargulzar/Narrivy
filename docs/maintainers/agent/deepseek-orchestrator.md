# DeepSeek Orchestration Entrypoint

`scripts/run_deepseek_task.py` is the repository-local, non-interactive way to run
one DeepSeek worker task and collect a machine-readable result. It wraps exactly
one `dsh --profile headless` invocation with the repository's pinned worker
overlay, a `workspace-write` file policy, bounded task/output sizes, a timeout, a
run lock, and an atomic JSON result.

## Division of responsibility

The worker overlay pins OpenCode Go's `deepseek-v4.1-flash` with provider
`reasoning: max`. Harness's interactive settings are disabled only for this
invocation; global user settings and credentials remain unchanged. The provider
setting is necessary because the installed model-selection plugin does not
retain a composition-level `reasoningEffort` field.

| Role | Owns |
|---|---|
| **DeepSeek** (`deepseek-v4.1-flash` via `dsh --profile headless`) | Implements the task described by the task file. It may modify files inside the workspace under the DSH file sandbox. |
| **Codex** | Orchestrates (writes the task file, invokes this script, reads the JSON), reviews the diff, runs and keeps tests, and accepts or rejects the result. |
| **This script** | Transport and bookkeeping only. It validates local inputs, pins the worker configuration and permission mode, bounds resources, serializes concurrent runs, and records the outcome truthfully. It never edits product code, commits, pushes, or records acceptance. |

The task file is the whole instruction: headless has no interactive follow-up.
Split multi-step work into one task per invocation.

## Usage

```sh
python3 scripts/run_deepseek_task.py --task-file agent/tasks/T-001.md \
    --result-file artifacts/deepseek-runs/T-001.json
```

The script prints exactly one line to stdout: the absolute path of the JSON
result. Worker stdout and stderr are captured into that JSON, never forwarded to
the orchestrator's stdout.

Common variations:

```sh
# bound the whole run to 15 minutes and abort if a stream exceeds 4 MiB
python3 scripts/run_deepseek_task.py --task-file task.md \
    --timeout-seconds 900 --max-output-bytes 4194304 \
    --on-output-overflow abort

# validate inputs and print the planned invocation without running anything
python3 scripts/run_deepseek_task.py --task-file task.md --dry-run
```

### What is executed

From `<workspace>` (the repository root unless `--workspace` overrides it):

```text
<dsh> --profile headless --patch <repo>/agent/deepseek-v4.1-flash.patch.yml -- <task text>
```

- The task file is passed as the single positional argument, so the `task` field
  receives the file's text verbatim (multiple words are joined by the headless
  parser only when more than one argument is present). It is never written into
  the result JSON — only its byte count and SHA-256 are recorded.
- The `--` separator keeps a task whose text looks like an option (for example a
  file that starts with `---` or `--help`) a positional argument for the dsh
  launcher.
- `--patch` is applied after the profile layer, so the pinned overlay overrides
  any unrelated interactive-session model choice.
- The child environment inherits the parent environment, then
  `DSH_PERMISSION_MODE=workspace-write` is set unconditionally. An ambient
  `danger-full-access` cannot widen the fence, and approval requests beyond the
  sandbox fail closed because no answerer exists in an unattended run.
- stdin is `/dev/null`; the child runs in its own process group so a timeout or
  abort can terminate the whole group (SIGTERM, then SIGKILL after
  `--kill-grace-seconds`).
  A SIGINT/SIGTERM delivered to the orchestrator reaps that same group before the
  run lock is released, so an interrupted run cannot leave an orphaned worker
  mutating the workspace behind a free lock.

### Flags

| Flag | Default | Meaning |
|---|---|---|
| `--task-file PATH` | required | UTF-8 task text handed to the worker. |
| `--result-file PATH` | `<workspace>/artifacts/deepseek-runs/deepseek-<utc>-<id>.json` | JSON result path. Use a unique path per run. |
| `--workspace PATH` | repository root | Worker cwd; the DSH `workspace-write` root. |
| `--overlay PATH` | `agent/deepseek-v4.1-flash.patch.yml` | Patch layer applied after the profile. |
| `--expect-model ID` | `deepseek-v4.1-flash` | Token the overlay must contain, else the run is refused. |
| `--expect-provider ID` | `opencode-go` | Provider token the overlay must contain. |
| `--dsh PATH\|NAME` | `dsh` | Launcher to invoke. |
| `--timeout-seconds N` | `3600` | Whole-run bound (must be > 0). |
| `--kill-grace-seconds N` | `10` | SIGTERM grace before SIGKILL on timeout/abort. |
| `--max-task-bytes N` | `262144` | Task files larger than this are rejected. |
| `--max-output-bytes N` | `8388608` | Bytes retained per stream (stdout/stderr). |
| `--on-output-overflow {truncate,abort}` | `truncate` | Keep draining and flag truncation, or terminate the run and fail. |
| `--lock-file PATH` | `<workspace>/artifacts/deepseek-runs/orchestrator.lock` | Run lock path. |
| `--lock-wait-seconds N` | `0` | Wait this long for the lock before failing. |
| `--dry-run` | off | Validate and print the plan; run nothing, write nothing. |

## Exit codes and outcomes

Exit codes:

| Code | Meaning |
|---|---|
| `0` | The worker completed (`outcome: completed`). |
| `1` | The worker failed; its own exit code is propagated when it is `1..255`. |
| `2` | Usage error or invalid local input (task, overlay). |
| `3` | `dsh` missing or unstartable. |
| `4` | Timeout. |
| `5` | Output budget exceeded with `--on-output-overflow abort`. |
| `6` | Run lock unavailable (held, unwritable, or unsupported platform). |
| `7` | The JSON result could not be written. |
| `130` | The orchestrator was interrupted (SIGINT/SIGTERM); the worker group was terminated. |

`outcome` is authoritative whenever a worker exit code happens to fall in
`2..7`. The result `exit_code` field always carries the worker's real code (and
`signal` its terminating signal, if any).

Outcomes: `completed`, `child_failed`, `timed_out`, `output_overflow`,
`spawn_failed`, `dsh_not_found`, `invalid_task_file`, `invalid_overlay`,
`lock_held`, `lock_error`, `interrupted`.

Usage errors raised while building the request (bad flag values, a missing
`--workspace`) print to stderr and write no JSON. Every outcome after that point
— including a missing task file, a bad overlay, a missing `dsh`, a held lock —
still writes a JSON result.

## Result JSON

Fields (schema version 1, written atomically with mode `0600`):

- `ok` / `outcome` / `exit_code` / `signal` / `timed_out` / `error` — the outcome,
  with `error` carrying a redacted one-line summary when the run failed.
- `response` — the worker's final answer (stdout), decoded as UTF-8 with
  replacement, bounded by `--max-output-bytes`.
- `diagnostics` — the worker's reasoning and error text (stderr), bounded the
  same way.
- `output_overflow` — `policy`, `stdout_truncated`, `stderr_truncated`, and the
  total bytes each stream produced.
- `stream_drain_complete` — `false` only in the pathological case where a pipe
  stayed open after the process group was killed; captured output may then be
  incomplete.
- `task` — file, byte count, and SHA-256 (never the text).
- `overlay` — path and SHA-256 of the patch layer actually applied.
- `dsh` — requested launcher, resolved path, profile, `permission_mode`, and the
  exact argv with the task elided.
- `workspace`, `limits`, `run_id`, `started_at`, `finished_at`,
  `duration_seconds`, `orchestrator`.

## Concurrency

Runs are serialized by an advisory `flock` on the workspace-level lock file, held
for the whole worker run and the result write. A second run fails closed with
`lock_held` (exit `6`) instead of racing on the same working tree; use
`--lock-wait-seconds` to queue instead. Use a unique `--result-file` per run: the
lock is per workspace, not per result path. An interrupt terminates the worker
process group before the lock is released, so a cancelled run never overlaps a
later one.

## Credential and content hygiene

- Credentials are resolved by dsh itself from the inherited environment or
  `$DSH_HOME/.credentials.yaml`. They are never placed on the command line and
  the child environment is never recorded.
- Before writing the JSON, every occurrence of an inherited credential value
  (environment names matching `api_key`/`token`/`secret`/`password`/`credential`,
  value length >= 8) is replaced with `[REDACTED:<NAME>]` in `response`,
  `diagnostics`, and `error`. This is a best-effort backstop, not a substitute
  for not printing secrets.
- Treat the result JSON as evidence: `response` and `diagnostics` can contain
  repository content and model reasoning. It is written under `artifacts/` and is
  not meant to be committed.
- The task text is visible to the same user via `ps` for the duration of the run
  (headless accepts the task only as a positional argument). Do not put secrets
  in task files.

## Tests

`tests/governance/test_run_deepseek_task.py` drives a generated fake `dsh`
executable; it makes no network calls and writes only inside a temporary
directory.

```sh
python3 -m unittest tests.governance.test_run_deepseek_task -v
```

## Codex review checklist

1. Read the plan/diff produced by the worker; do not trust the summary alone.
2. Check `ok`, `outcome`, `exit_code`, and `output_overflow` in the JSON.
3. Verify `task.sha256` and `overlay.sha256` match what you intended to run.
4. Run the focused tests and the relevant gate yourself; mock-only success is not
   acceptance.
5. Accept, fix, or reject explicitly — the script never does.
