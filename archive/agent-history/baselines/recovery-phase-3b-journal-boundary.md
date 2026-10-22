# Recovery Phase 3B — Portable recovery journal boundary

**Recorded:** 2026-08-28  
**Base HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (unchanged)  
**Status:** implementation and independent read-only review complete; non-release;
every requirement remains blocked

## Objective

Complete ADR-0030's second slice by moving concrete Unix recovery-journal
filesystem behavior out of `narrivy-core`. Preserve the existing persistent
JSON schema, transaction states, TTL/limit policy, durable intent-before-
delivery ordering, corruption handling, and privacy behavior.

## Permitted scope

- new portable `crates/narrivy-journal-api` for journal domain records, typed
  storage errors, transaction-ID validation, retention policy, and a storage
  port;
- new `adapters/journal-unix` for Unix directory/file ownership, permissions,
  no-follow I/O, atomic writes, directory durability, quarantine, and cleanup;
- `narrivy-core`: consume the journal port and use controlled in-memory test
  storage; no production filesystem implementation;
- daemon/service tests: compose the Unix adapter and use the portable listing
  API instead of inspecting journal directories directly;
- Cargo, architecture, ADR, boundary, baseline, blocker, and state records
  necessary to describe and validate the actual boundary.

## Required behavior

1. Keep serialized `JournalEntry` field names, state tags, delivery intent and
   report shapes, transaction IDs, timestamps, TTL, and maximum-entry policy
   unchanged.
2. Keep core recovery ordering unchanged: pending/raw/committed intent before
   delivery, exact report persistence, and truthful terminal uncertainty.
3. The Unix adapter owns 0700 journal directories and 0600 entry/temp files;
   it rejects symlink, wrong-type, wrong-owner, insecure, and multiply linked
   sensitive paths without following them.
4. Writes remain bounded and atomic: serialize, exclusive private temp file,
   file fsync, rename, and directory fsync. Every failure is typed; temporary
   payloads are RAII-cleaned.
5. Reads are bounded and no-follow. Malformed JSON is quarantined without
   exposing content or crashing. Listings ignore unrelated/temp files and
   return stable timestamp order.
6. In-process mutations are serialized; daemon single-instance ownership is
   the cross-process writer boundary. No transcript/audio appears in ordinary
   logs or error text.
7. Portable core contains no production `std::os`, Unix extension traits,
   platform FFI, concrete journal path, `tempfile`, or direct filesystem
   journal operations.

## Acceptance

- existing journal schema/round-trip, TTL/limits, intent ordering, delivery
  uncertainty, cancellation, and recovery tests pass through the port;
- Unix adapter tests cover mode/owner/type/symlink/hard-link/collision,
  no-follow reads, max size, atomic replace, fsync error propagation where
  controllable, torn JSON quarantine, temp cleanup, restart, and concurrent
  write/read behavior;
- daemon/CLI/desktop integration continues to pass with the real Unix adapter;
- source and dependency boundary validators plus fixtures pass;
- full workspace format/check/test/forced strict Clippy, frontend, TypeScript,
  governance, and unsuppressed supply-chain gates retain truthful outcomes;
- only installed native targets are run; Windows/macOS are left to the native
  CI matrix and are not simulated locally.

## Explicit non-goals

- no journal schema migration or history/learning-store merge;
- no IPC command authorization, portal wiring, microphone/native run, ASR or
  delivery latency feature;
- no evidence, evidence index, requirement status, manifest reseal, packaging,
  generated-output cleanup, commit, push, or release claim;
- no security/dependency advisory suppression.

## Implemented boundary

- `narrivy-journal-api` owns the unchanged persistent record shape, all seven
  state tags, transaction-ID validation, typed errors, TTL/limit policy, and
  the object-safe `RecoveryJournal` port. `Arc` forwarding preserves adapter
  overrides rather than falling back to non-atomic default methods.
- `narrivy-journal-unix` owns the concrete filesystem implementation. It
  requires an owner-matched, non-group/other-writable parent and an exact 0700
  journal directory; every operation revalidates ancestors, final-directory
  type/owner/mode, and entry type/owner/0600 mode/link count/size.
- Serialization streams through a 4 MiB bounded writer into a private
  `NamedTempFile`, then fsyncs the file, atomically persists it, validates the
  result, and fsyncs the directory. Reads and directory scans are independently
  bounded. Invalid unrelated JSON names are ignored; malformed or identity-
  mismatched records are quarantined without content disclosure.
- Startup validates and removes only adapter-owned `.journal-*` regular 0600
  single-link crash leftovers. Quarantine files are validated and bounded to
  16 files / 64 MiB, with oldest-first removal and directory fsync.
- Compound mark/recovery/retention operations hold the adapter's one mutation
  lock. Core uses only the trait and a controlled in-memory test journal;
  daemon composition owns the Unix adapter and history listing no longer reads
  the directory directly. `JournalError` variants remain typed through
  `CoreError`.
- The portable boundary policy and native/MSRV CI package lists now include
  `narrivy-journal-api`. No journal schema, recovery ordering, IPC wire shape,
  delivery semantics, or requirement status changed.

## Independent review and rework

Two read-only architecture/security reviews found and drove repair of:

1. silent chmod of an insecure existing directory and missing post-open path
   revalidation;
2. allocation-unbounded serialization/listing and missing total-directory
   scan caps;
3. non-atomic compound trait defaults and incomplete `Arc` forwarding;
4. lost 100-entry torn-write/restart coverage and weak self-roundtrip schema
   testing;
5. crash-leftover transcript tempfiles and unbounded corrupt fragments;
6. erased typed journal errors and daemon startup error disclosure coverage;
7. missing boundary/CI registration and stale architecture documentation.

The final focused suite has 4 portable API tests and 14 Unix adapter tests.
It includes a literal full-field pre-extraction JSON fixture/all state tags,
the restored 100-entry/15-torn-record restart proof, mode and ancestor-symlink
failures, hard links/types, post-open drift across every operation, bounded
write/read/listing, atomic replace, injected directory-fsync failure, temp
cleanup, quarantine collision/retention, malicious crash-temp lookalikes, and
128 concurrent round trips followed by reopen.

## Claim boundaries and remaining release blockers

- Only `x86_64-unknown-linux-gnu` is installed. The real Rust 1.85 toolchain
  compiled `narrivy-journal-api` and `narrivy-core`; Windows/macOS native CI
  has not run and is not claimed.
- Path component checks close cross-user attacks under the non-writable-parent
  invariant, but cannot atomically exclude a hostile same-UID process between
  check and path use. Directory-FD-relative I/O or explicit risk acceptance is
  deferred to B-STORAGE-013.
- Production still opens `<secure runtime>/journal`; ADR-0013 specifies XDG
  state/data durability. Logout/reboot migration and recovery remain
  B-STORAGE-013. This slice deliberately preserved existing behavior.
- `GetHistoryEntry` still uses the active recovery journal as a text source.
  Recovery/optional-History separation and privileged-command authorization
  remain B-IPC-012; Phase 3B does not accept that product behavior.
- Supply-chain advisories remain unsuppressed and release-blocking. The
  manifest is deliberately stale, all 98 Linux requirements remain blocked,
  and zero schema-v2 evidence records qualify.

## Final command results

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` / `git diff --check` | 0 / 0 | formatted; no whitespace errors |
| journal API / Unix adapter tests | 0 | 4 + 14 passed |
| targeted journal/core/daemon forced strict Clippy | 0 | no diagnostics |
| `cargo +1.85.0 check -p narrivy-journal-api -p narrivy-core --locked --offline` | 0 | installed real MSRV passes |
| `cargo check --workspace --all-targets --all-features --locked --offline` | 0 | no diagnostics |
| `cargo test --workspace --all-targets --all-features --locked --offline` | 0 | complete Rust workspace passes; core 65 |
| workspace forced strict Clippy | 0 | all targets/features, `-D warnings` |
| desktop `npm test -- --run` | 0 | 17 files / 24 tests; existing React `act(...)` warnings |
| desktop `npx tsc --noEmit` | 0 | no diagnostics |
| governance suite | 0 | 29 tests: 17 boundary + 12 recovery governance |
| dependency/source boundary validators | 0 / 0 | 9 protected portable/core packages |
| `cargo deny check bans/licenses/sources` | 0 | pass; duplicate warnings remain informational |
| `cargo deny check advisories` | 1 | unsuppressed GTK/glib/quick-xml/time/unic findings |
| `verify_traceability.py` | 0 | 136 mapped; 98 Linux requirements blocked |
| `verify_evidence.py --production` | 1 | 148 schema-v1 records quarantined; zero v2 |
| `validate_pack.py` | 1 | dirty/unsealed manifest; read-only |
| `release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `run_gate.py --phase P11` | 2 | quarantined; no writes |

Immediately before and after the seven boundary/release validators, hashes
matched exactly:

- Git status: `c8e51bfa0f1ea5ccf148533748e435dc982d3c8b31571a992586e24d1593aefe`;
- binary diff: `9a34b77f67d91947de67c02cef83424a436d4dc47f178b0e6b1db338d80a9bdd`;
- existing tracked/unignored workspace inventory and content:
  `aa78fbebc73abb9347e7bfa471328d786b02d5652cc088034d440ad4d3a6fc18`.

Protected identities stayed exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.
No evidence, requirement, manifest, package, microphone/portal action, commit,
or push occurred.

## Rollback

Rollback is limited to the new journal API/adapter, core/service composition,
tests, Cargo edges, and Phase 3B documentation. It must preserve accepted
Recovery 1/2A/2B/2C/3A work and protected user-owned artifacts/catalogs. Stop
for review if preserving the persistent schema or delivery ordering would
require a broader product change.
