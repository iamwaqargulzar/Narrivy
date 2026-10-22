# Recovery Phase 4Q-C5B2B1 — offline provisioning IPC contract

Status: bounded protocol implementation PASS by principal review. The requested
independent reviewer exhausted its account allowance before returning a
verdict; no independent PASS is claimed. Release remains blocked.

## Result

Protocol minor 5 now defines bounded start/cancel commands and a typed,
content-free asynchronous status response for daemon-owned offline model
provisioning. The client supplies only a local source path. Model identity,
digest, destination, timeout, and network policy are absent by design.

Both outbound and inbound serialization reject empty, oversized, control-
character, or non-opaque fields. Debug representations redact source paths and
operation IDs. New lifecycle/status enums preserve unknown future values, and
old status payloads without `model_provisioning` remain valid.

The daemon handler explicitly rejects both commands with
`DaemonUnavailable` until a provisioning service is composed. Unix transport
test status fixtures set the optional field to absent, and the CLI treats an
unsolicited provisioning response as unexpected. No mutation path exists in
this work item.

## Validation

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` | 0 | workspace Rust formatting passes |
| `git diff --check` | 0 | no whitespace errors |
| `cargo test -p narrivy-ipc --locked` | 0 | 30 tests pass |
| `cargo clippy -p narrivy-ipc --all-targets --locked -- -D warnings` | 0 | no diagnostics |
| focused narrivyd uncomposed-command test | 0 | start and cancel both reject without disclosing path or operation ID |
| `cargo clippy -p narrivy-ipc -p narrivyd --all-targets --all-features --locked -- -D warnings` | 0 | no diagnostics |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | no diagnostics |

## Scope and exclusions

Changed code is limited to the portable IPC protocol/re-exports, protocol
specification, and explicit compatibility/fail-closed matches in the daemon,
Unix transport tests, and CLI response renderer. ADR-0059, this baseline,
state, decisions, and research record the bounded contract.

No daemon provisioning worker, authorization policy, rate limit, lifecycle
transition, model/cache mutation, CLI command, desktop UI, event, package,
evidence/index/traceability change, manifest reseal, commit, or push occurred.
All 98 Linux release requirements remain blocked.

