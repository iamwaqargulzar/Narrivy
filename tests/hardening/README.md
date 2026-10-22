# P9 Hardening — Failure-Injection Harnesses

> Owned by P9 HARDENING PREP (spec 09-hardening). Skeletons — not yet gated.

Four harnesses attack the already-working vertical slice under stress.
Every harness is **fail-closed**: it never fabricates a pass, leaks transcript/audio, or panics on expected failure.

## Harnesses

| Harness | File | What it attacks | Success condition |
|---|---|---|---|
| **process kill -9 mid-migration** | `harness_kill_mid_migration.py` | daemon SIGKILL during hashed SQLite migration (WAL, tmp→fsync→rename) | restart verifies hashes, replays or rolls back, no lost recoverable text, journal ordered |
| **journal torn-write** | `harness_journal_torn_write.py` | power-loss: truncated JSON + stray `.tmp` + `.corrupt` quarantine | `recoverable_entries()` returns only TTL-fresh, non-delivered, time-sorted survivors; torn files are quarantined |
| **fd exhaustion** | `harness_fd_exhaustion.py` | `EMFILE`/`ENFILE` via soft-RLIMIT_NOFILE clamp and many open fds | typed errors, no panic, bounded fallback, transcript preserved, Inbox fallback honored |
| **OOM via cgroup limit stub** | `harness_oom_cgroup_stub.py` | memory pressure without host OOM (cgroup v2 `memory.max` stub + `RLIMIT_AS` fallback) | allocation rejected with typed error, no unwrap, bounded, no transcript in logs |

## Layout

```
tests/hardening/
  README.md
  common.py                 — shared helpers (tmp dirs, journal helpers, fd helpers, redaction checks)
  harness_kill_mid_migration.py
  harness_journal_torn_write.py
  harness_fd_exhaustion.py
  harness_oom_cgroup_stub.py
  test_p9_hardening_skeleton.py — pytest smoke: imports + dry-run of each harness in stub mode
tools/narrivy-qualify/p9_runner.py — single entry point that runs any harness and emits schema-valid evidence under agent/evidence/P9/
  p9_runner.py --scenario p9-daemon-loss        # kill-9 mid-migration + recovery
  p9_runner.py --scenario p9-journal-torn        # torn-write quarantine
  p9_runner.py --scenario p9-fd-exhaustion       # fd exhaustion
  p9_runner.py --scenario p9-oom-stub            # cgroup limit stub
  p9_runner.py --scenario p9-security-hardening  # archive traversal + deny-by-default stub
  p9_runner.py --scenario p9-a11y-core           # a11y stub (delegates to existing a11y checks)
  p9_runner.py --scenario p9-perf-regression     # perf regression vs P6 baselines stub
  p9_runner.py --list
  p9_runner.py --all --json                      # run all harnesses sequentially
```

## Evidence

Harnesses are invoked through `tools/narrivy-qualify/p9_runner.py` which writes one JSON evidence file per scenario under `agent/evidence/P9/` bound to Git commit, dirty flag, build ID, native/VM environment, scenario hash, timestamps, and sanitized diagnostics (no transcript/audio/clipboard). It validates against `spec/schemas/evidence.schema.json` via `jsonschema` if available.

## Privacy

No harness logs transcript text, audio, clipboard/selection/context, tokens, or secrets. Leaks are asserted via `common.assert_no_transcript_leak`.

## Running (skeleton)

```bash
# dry-run all harnesses without needing a live daemon (stub mode)
python3 tools/narrivy-qualify/p9_runner.py --all --stub --json
# single harness verbose
python3 tools/narrivy-qualify/p9_runner.py --scenario p9-journal-torn --stub --verbose
# pytest skeleton smoke
python3 -m pytest tests/hardening/test_p9_hardening_skeleton.py -v
```

Real-daemon mode (requires `target/debug/narrivyd` built) will replace `--stub`
only after the harness supplies separate private `XDG_RUNTIME_DIR` and 0700
`XDG_STATE_HOME` roots. Skeletons currently fail closed in real mode.
