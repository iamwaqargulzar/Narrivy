# narrivy-qualify

Standard scenario runner per the legacy planning amendment Part F.

- `python3 tools/narrivy-qualify/narrivy-qualify.py list` — list named scenarios from `spec/scenarios/`
- `python3 tools/narrivy-qualify/narrivy-qualify.py run <id> [--json] [--out <path>]` — run automatable portions, guide manual/native steps, emit schema-valid JSON evidence under `agent/evidence/<phase>/`
- `python3 tools/narrivy-qualify/narrivy-qualify.py verify <evidence.json>` — validate evidence against `spec/schemas/evidence.schema.json`

Evidence binds Git commit, dirty status, build ID, environment (native/VM, OS/kernel/desktop/session, CPU/GPU/NPU, mic, input backend), scenario hash/version, timestamps, exit code, pass/fail, metrics, artifact hashes, and sanitized diagnostics. Never captures transcript/audio/clipboard/context unless synthetic fixture explicitly requires it.

Scenarios cover: shortcut/toggle cycles, PTT press/release where claimed, activation after restart/suspend, mic open/record/stop/reopen, hotplug/fallback, text insertion into each required app class, Unicode/non-Latin, multiline, clipboard transaction, Inbox fallback, cancellation race, daemon crash/restart/recovery, package install/upgrade/uninstall, migration interruption, CPU fallback, accelerator tuple qualification, privacy leakage, accessibility core path, and performance protocol.

Owner-gate minima reference scenario_ids; phase gates verify that required scenarios were actually run and pass.
