# Recovery Phase 4Q-C5B2B0 — bounded immutable model activation prerequisites

Status: bounded implementation and independent final review PASS. Release
remains blocked.

## Objective and bounded result

C5B2B0 closes the cancellation, deadline, launch, and retained-child ownership
gaps between the accepted offline import primitive and a future daemon-owned
provisioning lifecycle. It deliberately adds no IPC provisioning command or
product-visible mutation.

The Linux process adapter now provides controlled trust-anchored executable and
model snapshot operations using one `SessionCancellation` and absolute
deadline. Executable snapshot copy and revalidation use fixed 64 KiB chunks.
Controlled construction hashes the executable once while sealing it, performs
only structural validation while building the command, and performs one full
digest revalidation at the final spawn boundary. Cancellation has priority over
simultaneous expiry, and a final check prevents a late process creation.

Resident initial startup now reserves its two-second termination allowance
inside the caller's absolute deadline. The operation deadline controls launch
and handshake; the original deadline controls failure cleanup. Retained initial
workers are transferred under a brief global lock and reaped outside it.
Contending constructors observe their own deadline/cancellation, and unresolved
owners are merged back before cleanup ownership is released.

## Controlled coverage

- cancellation during trust-anchored executable digest revalidation prevents
  process creation;
- synthetic expiry during digest revalidation and immediately before spawn is
  typed and prevents launch;
- cancellation takes priority when cancellation and expiry coincide;
- model snapshot cancellation and expiry are checked during copy and after
  copy, with sealed immutable output on success;
- production digest-bound resident launch maps pre-cancellation and expiry to
  the resident startup control outcomes;
- initial handshake cancellation and timeout reap their child or retain its
  ownership with a typed cleanup failure;
- deterministic overlapping cleanup proves the global ownership mutex is free
  during process waits and an earlier second deadline is honored;
- the overlapping cleanup test passed ten consecutive independent-review runs.

All model and process fixtures are synthetic or controlled system executables.
No real model, user cache, microphone, audio, transcript, or installed package
was read or mutated.

## Validation

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` | 0 | workspace Rust formatting passes |
| `git diff --check` | 0 | no whitespace errors |
| `cargo test -p narrivy-process-linux --locked -- --test-threads=1` | 0 | 29 passed; one helper ignored |
| `cargo clippy -p narrivy-process-linux --all-targets --locked -- -D warnings` | 0 | no diagnostics |
| `cargo test -p narrivy-asr-transcribe --features resident-worker --lib --locked resident::tests -- --test-threads=1` | 0 | 34 passed; one explicit native diagnostic ignored |
| `cargo clippy -p narrivy-asr-transcribe --features resident-worker --all-targets --locked -- -D warnings` | 0 | no diagnostics |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | no diagnostics |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | complete workspace sweep passes; explicitly gated native diagnostics remain ignored |

The first independent review returned REWORK because production executable
revalidation was uncontrolled and retained-worker cleanup held a global mutex
during process waits. Both findings were corrected. Independent final re-review
returned PASS and found no residual Narrivy/ASR process or Narrivy temporary
directory.

## Changed scope

- `adapters/process-linux/src/lib.rs` adds controlled executable snapshot,
  revalidation, command, spawn, ancestry checks, typed outcomes, and tests;
- `engines/transcribe-cpp/src/resident.rs` composes those controls through
  production startup, reserves cleanup budget, transfers retained ownership,
  and adds deterministic production/overlap tests;
- the process adapter dependency metadata, ADR-0058, this baseline,
  `agent/STATE.json`, `agent/DECISIONS.md`, and `agent/RESEARCH_LOG.md` record the
  bounded design and review.

## Explicit exclusions and remaining blockers

- No daemon IPC mutation, provisioning admission, lifecycle transition, CLI,
  desktop UX, native model activation, or C5B2B feature surface occurred.
- No real model/cache import, network transport, evidence/index/traceability
  change, manifest reseal, package, installation, commit, or push occurred.
- Same-user IPC authorization policy, rate limiting, asynchronous status,
  shutdown linearization with provisioning, and awaiting-to-ready activation
  remain for separately reviewed work.
- All 98 Linux release requirements and the release remain blocked.

