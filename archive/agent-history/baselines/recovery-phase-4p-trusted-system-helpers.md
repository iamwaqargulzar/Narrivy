# Recovery Phase 4P — trusted strict Linux system helpers

Status: controlled non-release implementation independently reviewed PASS.
Release remains blocked.

## Objective

Remove ambient-`PATH` discovery, inherited-environment execution, and
direct-child-only cleanup from the strict `pw-record` and `wtype` boundaries
without changing transcript/delivery semantics or claiming native behavior.

## Audit finding

The repository has ten logical subprocess families. The active strict
platform helpers were selected by inherited `PATH`, reopened later by name,
inherited the daemon environment, and terminated only their direct PID. The
development host's legitimate `/usr/bin/pw-record -> pw-cat` alias means the
canonical target must be validated without replacing the alias used for
execution.

## Implemented bounded scope

- Added `adapters/process-linux`, a Linux-only mechanics adapter with no
  capture, delivery, ASR, daemon, or portable-domain policy.
- Strict helpers search only `/usr/bin`, then `/bin`. A name must be one path
  component. The root-owned launch alias and canonical executable plus their
  non-writable root-owned ancestry are validated; alias and target identities
  are snapshotted and revalidated at the one-shot spawn boundary.
- The child environment is cleared. PipeWire receives only
  `XDG_RUNTIME_DIR`, `PIPEWIRE_RUNTIME_DIR`, `PIPEWIRE_REMOTE`, and forced
  `PIPEWIRE_NO_CONFIG=true`; wtype receives only `XDG_RUNTIME_DIR` and
  `WAYLAND_DISPLAY`. The child cwd is `/`; unset stdio defaults to null.
- Every migrated helper receives a new process group, parent-death SIGKILL,
  expected-parent check, umask 0077, and close-range CLOEXEC. Required kernel
  support is preflighted and fails readiness closed.
- Batch/streaming strict PipeWire and strict wtype success, cancellation,
  timeout, error, and drop paths use bounded group cleanup. A leader which
  exits while a same-group descendant remains is cleaned and reported as a
  typed failure.
- Wtype retains exact Phase 2B semantics: stdin-only payload, clean
  pre-positive-write cancellation only after cleanup, uncertain
  post-positive-write cancellation, and exactly one mandatory Inbox recovery.
- Daemon StartSession readiness, explicit capability checks, and status strings
  share the trusted resolver. Status reports helper trust separately from
  unverified microphone signal and unverified target retention. The diagnostic
  named `mic` cannot pass solely because the helper exists.
- Updated AGENTS/architecture boundaries, ADR-0048, blockers, decision log,
  research log, and truthful state. No catalog entry was added because this is
  infrastructure rather than a product Feature, Module, Runtime, or Pipeline.

## Controlled verification

- `cargo test -p narrivy-process-linux --all-targets --locked`: exit 0, 9
  tests. Coverage includes invalid names, unsafe ancestry/set-ID/anchor escape,
  alias identity drift at spawn, environment scrubbing, process-tree
  termination, and residual-descendant failure; the consuming spawn API
  statically prevents command reuse.
- `cargo test -p narrivy-platform-linux --lib recovery --locked --
  --test-threads=1`: exit 0, 47 matching tests. Both new controlled helpers
  prove a successful leader with a live descendant fails closed; a regression
  proves ordinary status polling cannot consume the later cleanup deadline;
  existing capture/wtype cancellation, timeout, stdin, and drop behavior
  remains green.
- `cargo test -p narrivyd --locked`: exit 0; 11 + 75 + 78 + 79 tests passed.
  Typed readiness and capability assertions include every trust failure and do
  not claim microphone signal or target retention.
- `cargo check --workspace --all-targets --all-features --locked`: exit 0.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D
  warnings`: exit 0.
- `cargo fmt --all -- --check`, `git diff --check`,
  `scripts/verify-boundaries.py`, and
  `scripts/verify_dependency_boundaries.py`: exit 0.
- Desktop Vitest: exit 0, 17 files / 28 tests; TypeScript: exit 0. Existing
  React `act(...)` warnings remain visible and were not suppressed.

The full `cargo test --workspace --all-targets --all-features --locked --
--test-threads=1` exits 101 at the untouched
`pipewire_monitor::tests::latency_histogram_p99_under_load_stays_under_50us`
microbenchmark. In both failed sweeps its measured p99 was 22 microseconds,
below the named 50-microsecond bound, but an additional p50 assertion observed
16 microseconds and failed. The exact isolated rerun exited 0 at p50 5
microseconds / p99 6 microseconds. This is recorded as a pre-existing flaky
quality gate, not a Phase 4P pass or native latency qualification.

`cargo deny check` exits 1 at advisories. Bans, licenses, and sources pass;
desktop/Tauri transitive vulnerabilities and unmaintained packages remain
unsuppressed. No exception or dependency-gate weakening was added.

## Current-host content-free observation

`/usr/bin/pw-record` is a root-owned symlink to root-owned mode-0755
`/usr/bin/pw-cat`; `/usr/bin/wtype` is root-owned mode 0755. Dpkg reports
`pipewire-bin 1.5.85-1ubuntu1pop1~1769189762~24.04~601113b` and `wtype 0.4-3`;
the kernel is `7.1.5-76070105-generic`. These observations validate only the
controlled resolver on this development host. No microphone, transcript,
target application, or native dictation path was exercised.

## Explicit residual risk and exclusions

- Pathname revalidation is not FD-bound execution and does not resist a
  root-authorized in-place race or prove package/SBOM/release provenance.
- Process groups do not contain an adversarial helper which calls `setsid`;
  systemd/cgroup abrupt-death qualification is still required.
- Resident ASR worker/model binding and shutdown, legacy sidecar, generic
  delivery, PipeWire monitor, trigger/audio-policy/compute helpers, and other
  subprocess families remain for dedicated work.
- The broad `Cargo.lock` delta synchronizes many pre-existing dirty-workspace
  dependency entries as well as adding `narrivy-process-linux`; it is not
  represented as a process-crate-only lock change.
- No native observation, evidence/index or requirement status change,
  manifest reseal, package build, protected-file edit, commit, push, Phase 4Q,
  or release claim occurred.

## Governance and non-mutation

| Command | Exit | Material result |
| --- | ---: | --- |
| governance unit suite | 0 | 12 passed |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements remain blocked |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| `python3 scripts/validate_pack.py` | 1 | dirty/unsealed manifest rejected read-only |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no files changed |

The validator window used `PYTHONDONTWRITEBYTECODE=1`. Its pre/post hashes
matched exactly. The tracked/unignored inventory includes explicit markers for
tracked files currently absent from the working tree and excludes only this
self-referential baseline:

- Git status:
  `4c8eb625405b5e2b88c619cc22916698a46890b4af55da7dc7e45d6ffdd91bca`;
- Git binary diff:
  `3826f8999997c2689585e8629efd2508f827b058c1bfae293f3705bc6e781d5e`;
- tracked/unignored inventory/content:
  `a3dc154329daed3bb4331f042e1b4b5a363d01ac27e3e2eef76fbc03533c6c21`.

Generated Python bytecode was removed after validation; no `__pycache__` or
`.pyc` remains outside ignored build/dependency output.

Protected and governance identities remain exact:

- `artifacts/releases/narrivy_0.2.3_amd64.deb`:
  `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `packaging/deb/BUILD_LOG.md`:
  `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- `spec/requirements-traceability.csv`:
  `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- `agent/evidence/INDEX.json`:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Independent-review disposition

PASS for Recovery 4P only. The reviewer found and required two corrections:
normal batch status polling had prematurely armed its later cleanup deadline,
and the user-facing microphone check could pass from helper presence without
signal observation. Both were fixed, received deterministic regression tests,
and passed focused reruns. The reviewer found no remaining production direct-
child wait/kill in strict PipeWire or wtype paths and no remaining Phase 4P
correctness finding.
