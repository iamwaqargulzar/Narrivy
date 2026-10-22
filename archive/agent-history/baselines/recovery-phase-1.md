# Recovery Phase 1 Baseline — 2026-08-26

Candidate HEAD: `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (dirty worktree).
This is a governance baseline, not product evidence. No product requirement or
release is passed.

## Evidence and release state

- `python3 -m unittest tests.governance.test_recovery_validators -v` — exit 0;
  twelve governance unit tests passed. They cover the semantic-success rule,
  failed/skipped/missing/malformed/nonzero evidence rejection in isolated
  temporary copies, authoritative index registration/hash checks, typed
  artifact binding, verification profiles/procedures, P0 Markdown quarantine,
  traceability quarantine, substitution policy enforcement, and non-mutating
  direct/imported P9/P11 runner and refresh-script quarantine. The real
  traceability file is not mutated by those tests.
- `python3 scripts/verify_traceability.py` — exit 0: 136 requirements mapped,
  98 Linux 1.0 gated and blocked, 38 roadmap.
- `python3 scripts/verify_evidence.py --production` — exit 1: 148
  historical/quarantined schema-v1 records; zero schema-v2 candidates.
- `python3 scripts/validate_pack.py` — exit 1 and made no filesystem changes.
  It reports an out-of-date manifest, including the two user-owned packaging
  edits, prior unsealed source changes, and Recovery Phase 1 governance files.
- `python3 scripts/release_gate.py --target linux-1.0` — exit 1 and made no
  filesystem changes. Material reasons: pack validation fails; all 98
  release-gated requirements are blocked; P11 report is failed; release
  metadata binds an older commit; and P11/P6 records are schema-v1 historical
  material rather than qualified evidence.
- The legacy `scripts/run_gate.py --phase P11` was run once before quarantine
  because the pre-existing phase instruction required it. It rewrote P2, P5,
  P11 evidence and the pack manifest; every such write was restored exactly to
  the committed historical content before Recovery Phase 1 changes proceeded.

## Sol rework verification

- Semantic success is fail-closed: `result` must be `pass`, `pass` must be the
  boolean `true`, and `exit_code` must be `0`.
- Schema-v2 evidence is accepted only if its exact evidence-file SHA-256 is
  registered in the authoritative `agent/evidence/INDEX.json` and it binds the
  exercised executable or release `.deb` through `artifact_under_test`.
- The isolated regression cases were exercised through the requirement marker,
  evidence verifier, phase validator, and release validator. Failed, skipped,
  missing, malformed, nonzero-exit, unregistered, index-hash-mismatched, and
  unrelated-file records all fail; the real traceability CSV remains unchanged.
- Substitutions are fail-closed. A controlled record may use one only when both
  its reviewed profile and procedure explicitly permit it. Native and package
  profile fixtures reject each of `mock`, `fixture`, `proxy`, `stub`, and
  `synthetic-audio` through all four validators; the exact native
  `substitutions: ["mock"]` marker exploit exits nonzero without changing the
  temporary traceability file.
- P9/P11 direct CLI and imported `main()` entry points return `2` and leave
  evidence untouched. Their former passing-evidence writers were removed; Git
  retains the historical implementations.
- Every traceability verification layer maps to exactly one reviewed profile.
  Required unit/contract rows use controlled `unit` proof without an artifact
  under test; native/package behavior requires an artifact and a hash-pinned
  reviewed runner. The committed procedure registry remains empty.

## Requested quality baseline

| Command | Exit | Material result |
| --- | ---: | --- |
| `python3 scripts/verify_dependency_boundaries.py` | 1 | `narrivy-core`, `narrivy-ipc`, and `narrivy-pipeline` transitively depend on forbidden `windows-sys`. |
| `cargo fmt --all -- --check` | 1 | Formatting diffs in Linux adapter, daemon, and test sources. |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | Completed with warnings, including dead code and an unused import. |
| `cargo test --workspace --all-features --locked` | 101 | Two `narrivy-platform-linux` PipeWire latency tests failed: histogram percentile expected 10 but got 11; p99 was 50µs at a strict `<50µs` limit. |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 101 | `unwrap`/`unwrap_err`, cloned-reference, and dead-code diagnostics became errors. |
| `cargo deny check` | 7 | `narrivy-history` is unlicensed; bans and advisories also fail. |
| `npm test -- --run` (from `apps/narrivy-desktop`) | 0 | 17 test files and 24 tests passed; Vitest reported React `act(...)` warnings. |
| `./node_modules/.bin/tsc --noEmit --incremental false` (from `apps/narrivy-desktop`) | 0 | TypeScript completed with no diagnostics. |

All Cargo output used a temporary `CARGO_TARGET_DIR` outside the repository.

## Preservation check

The two pre-existing user edits were preserved byte-for-byte. SHA-1 object
hashes before and after Recovery Phase 1 are respectively:

- `artifacts/releases/narrivy_0.2.3_amd64.deb` — `89aa38a69b2fea91bd88aaabfd2ec3d4b67fd8df`
- `packaging/deb/BUILD_LOG.md` — `66dee07cea53b7331a3e9bfd742507219dbcdca1`
