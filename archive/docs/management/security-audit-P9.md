# Security Audit P9

**Head:** `7db645d` (P8 gated) — P9 hardening audit. Date: 2026-08-24.

## 1. Dependency advisories — `cargo audit` / `cargo deny`

- `cargo audit` not installed in this lane (`cargo audit --help` → `no such command`). Verdict: **fallback to manual advisory check**.
- Manual advisory check: `cargo deny check advisories --locked`
  - Config: `deny.toml` `[advisories] ignore = []` (no blanket skip), `[sources] unknown-registry = deny`, `[bans] wildcards = deny`.
  - Workspace lockfile committed and CI uses `--locked` per `docs/security/supply-chain.md`.
  - Execution: `cargo deny check advisories` run; advisories database would require network fetch — offline lane reports `advisory-db fetch failed (offline)`. No advisory is silently ignored; update lane will fetch and must be clean before `cargo deny check` passes in CI. Licenses/bans already enforced via same deny config.
  - Result: **no known advisory silently suppressed; `cargo deny` advisory lane not bypassed** (offline placeholder, CI must re-run with network).

- `cargo deny check bans licenses sources` — policy enforced: `bans.multiple-versions = warn`, `bans.wildcards = deny`, `licenses.allow` strictly enumerated, private crates ignored.

## 2. `unsafe` containment scan

Allowed `unsafe` sites per AGENTS.md / crate docs:

| Crate | File | Site | Justification |
|---|---|---|---|
| `narrivy-ipc-unix` | `adapters/ipc-unix/src/security.rs` `getsockopt(SO_PEERCRED)` | `unsafe { getsockopt }` | Linux kernel-verified peer credentials ADR-0011; `unsafe_code = allow` is declared for this crate only (`adapters/ipc-unix/Cargo.toml:9`), contained with a SAFETY comment. |
| `narrivy-ipc-unix` | `adapters/ipc-unix/src/security.rs` `getpeereid`, `geteuid` | `unsafe { getpeereid / geteuid }` | Non-Linux fallback + uid query; same containment. |
| `narrivy-ipc-unix` | `adapters/ipc-unix/src/server.rs:840` `flock(LOCK_EX\|LOCK_NB)` | `unsafe { flock }` | Single-instance lock ADR-0012; contained with SAFETY. `adapters/ipc-unix/src/client.rs:8` is separately `#![forbid(unsafe_code)]`. |
| `adapters/platform-linux` | `src/instance.rs` `getuid`/`flock` | `unsafe { libc::getuid / flock }` | Platform-specific instance guard; crate `unsafe_code = allow` (`adapters/platform-linux/Cargo.toml:13`) with invariant docs. |
| `adapters/platform-linux` | `src/pipewire_monitor.rs` `fcntl`, `clock_gettime` | `unsafe { libc::fcntl / clock_gettime }` | Non-blocking PipeWire stream flags and CPU-time sampling. This file contains **no** `env::set_var`/`remove_var`; the P9 row claiming that is stale. |
| `engines/transcribe-cpp/transcribe-sys` | `src/lib.rs` FFI | `unsafe { tcpp_* }` + `unsafe extern "C"` | Isolated FFI to `transcribe.cpp` C ABI; the **ONLY** allowed `unsafe` for that engine per AGENTS.md. |

Scan: `grep -R -n "unsafe" crates/ adapters/ services/ engines/ --include=*.rs` (excluding `target`, `code/vendor`, `code/narrivy-v0.4` legacy)

- Portable crates (`narrivy-core`, `narrivy-history`, `narrivy-diagnostics`, `narrivy-pipeline`, `narrivy-policy`, `narrivy-compute`, `narrivy-platform-api`, `narrivy-types`, `narrivy-model-manager`, `narrivy-asr-api`, `narrivy-ipc`, `narrivy-journal-api`, `services/narrivyd`, `apps/narrivy-desktop/src-gtk`, `engines/asr-mock`, `engines/transcribe-cpp` non-sys) inherit the workspace `unsafe_code = "forbid"` lint and contain **no `unsafe {`**. `crates/narrivy-ipc` additionally carries `#![forbid(unsafe_code)]` (`crates/narrivy-ipc/src/lib.rs:15`).
- Workspace root `Cargo.toml` denies `unsafe_code` workspace-wide; overrides are **explicit** in `narrivy-ipc-unix` (`adapters/ipc-unix/Cargo.toml:9`), `platform-linux` (`adapters/platform-linux/Cargo.toml:13`) and `transcribe-sys` with documented invariants. `narrivy-ipc` no longer needs an override.

Result: **PASS — no `unsafe` outside allowed adapters/engines.**

## 3. IPC socket permissions `0700`/`0600` and same-user authorization

Implementation: `adapters/ipc-unix/src/security.rs` + `adapters/ipc-unix/src/server.rs`.
The P9 text naming `crates/narrivy-ipc/src/security.rs` is stale: peer
credential and endpoint-permission mechanics moved to the Unix transport
adapter ADR-0011 requires; `crates/narrivy-ipc` keeps only the portable wire
contract and is `unsafe_code`-free.

- `EndpointPermissions::is_secure()` enforces `(dir_mode & 0o777) == 0o700 && (sock_mode & 0o777) == 0o600`.
- `EndpointPermissions::validate(expected_uid)` additionally checks `owner_uid == expected_uid` else `PermissionDenied`.
- `SocketPathValidation::should_remove_stale()` conservative conjunction: `actual == expected && !is_symlink && is_socket && owner_uid == expected_uid` — symlink attack, cross-user race, path replacement blocked.
- Server bind order (ADR-0012): `prepare_runtime_dir(0700)` → `acquire_instance_lock(flock)` → `remove_stale_socket_if_safe(validation)` → `bind` → `chmod 0600` → `verify EndpointPermissions` → `fsync(dir)`.
- Peer auth: `peer_credentials(stream)` via `SO_PEERCRED` (Linux) / `getpeereid` (other unix); `PeerCredentials::validate(expected_uid)` denies foreign uid before any privileged handling.
- Tests: `adapters/ipc-unix/src/security.rs` unit tests cover `ipc_peer_credentials_same_user_validate_ok`, `ipc_peer_credentials_mismatch_denied`, `ipc_endpoint_permissions_secure_and_insecure_matrix`, `ipc_stale_socket_decision_matrix` and `ipc_peer_credentials_real_socket_reports_self_user`. `adapters/ipc-unix/src/server.rs:1195` (`ipc_end_to_end_status_and_event_over_unix_socket`) exercises the bound path end to end.

Evidence: `cargo test -p narrivy-ipc-unix` green (peer + endpoint + stale-path + end-to-end suites). The P9 command `cargo test -p narrivy-ipc -- security` names the wrong package; those tests no longer exist in `narrivy-ipc`.

Result: **PASS — 0700/0600 + same-user enforced, stale-path FSM safe.**

## 4. No transcript in ordinary logs — `grep -r "transcript" logs/`

- Repo `logs/` directory **does not exist** at `ROOT/logs` nor `services/narrivyd/logs` — ordinary logs are not persisted as files in this lane; runtime diagnostics go through `narrivy-diagnostics::Redactor`.
- Direct grep: `grep -R "transcript" services/narrivyd/src --include=*.rs` hits only **typed transcript handling** (`AsrEvent::Partial { transcript }`, `AsrEvent::Final { transcript }`, `daemon/session.rs` stage notes) — never a `log!`/`eprintln!`/`tracing::info!` carrying transcript.
- Diagnostics redactor (`crates/narrivy-diagnostics/src/redactor.rs`) deny-list `DEFAULT_DENIED_KEYS` is **22 keys**, not the 10 recorded by P9: `transcript`, `text`, `audio`, `samples`, `clipboard`, `token`, `secret`, `password`, `authorization`, `cookie`, `last_delivered`, `delivered`, `words`, `spoken`, `utterance`, `tokens`, `context`, `passphrase`, `api_key`, `apikey`, `access_key`, `private_key`. Matching is not exact-key-only: `Redactor::key_is_denied` (`redactor.rs:134`) first tries the lowercased whole key, then falls back to `key_tokens()` (`redactor.rs:243`), which splits on non-alphanumerics **and on camelCase boundaries**, and denies when any token is on the list. So `apiKey`, `transcriptText` and `LastDelivered` are denied, while `texture` does not falsely match `text`. `Redactor::redact_json` replaces matching keys with `[REDACTED:<len>]` and scrubs `key=value` inside strings; `SupportReport` is redacted by construction. Test `redacted_object_value_never_survives` proves it.
- Historical P8 proof: `code/narrivy-v0.4/crates/narrivy-core/tests/p8_privacy.rs::p8_logs_redact_content` and `crates/narrivy-diagnostics/src/support.rs::redactor_scrubs_transcript_fields_in_serialized_report` — now **re-proven** in P9 by `crates/narrivy-ipc/src/security_audit.rs::no_transcript_in_ordinary_logs`.

Result: **PASS — `grep -r "transcript" logs/` empty; diagnostics path is content-free by redactor.**

## 5. History vs recovery split (recovery correction 2026-08-30; History composition update 2026-09-28)

Wire surface: `crates/narrivy-ipc/src/protocol.rs`

```rust
pub enum Command {
    /// Optional retained History only.
    GetHistory,
    /// One optional retained History entry.
    GetHistoryEntry { txid: String },
    /// One exact short-lived recovery transaction; no listing form.
    GetRecoveryEntry { txid: String },
}
pub struct HistoryEntryDto { pub txid, pub state, pub pipeline_id, pub has_delivered_text, pub updated_unix_ms }
pub struct HistoryEntryDetailDto { pub txid, pub state, pub raw, pub corrected, pub delivered, pub pipeline_id }
pub enum ResponsePayload { History(Vec<HistoryEntryDto>), HistoryEntry(HistoryEntryDetailDto), RecoveryEntry(RecoveryEntryDetailDto), ... }
```

- Optional History **is composed**. `narrivyd` opens a real History store
  below its durable state root with its own database, retention policy and
  privacy-zone evaluation (`services/narrivyd/src/daemon/retained_history.rs`).
  `GetHistory`/`GetHistoryEntry` are served when an open store exists and return
  the typed, content-free not-composed rejection only when composition failed.
  Neither command ever reads recovery storage.
- `GetRecoveryEntry { txid }` validates and reads only the exact transaction
  identity reported by the session event. There is no recovery listing.
- Status advertises `history.optional=enabled` plus content-free
  `history.retention=<policy>` and `history.max_entries=200` only when a store
  is really open, and `history.optional=disabled:not_composed` otherwise. The
  desktop must gate on the capability and render the required disabled
  explanation when it is absent.
- The recovery journal is never mirrored into History as a listing: only one
  finalized result per transaction is recorded, and the journal remains the
  only holder of in-flight text (ADR-0009, ADR-0013, ADR-0045).

Result: **historical P9 claim quarantined.** Recovery 4N-A supplies regression
tests for the corrected boundary, but creates no release evidence and leaves
all requirements blocked.

## 6. Additional hardening notes (P9 scope)

- Plugin permissions explicit/enforced, network deny-by-default per `docs/roadmap/design/plugin-permissions.md` and `resource-network-policy.md`; archive handling rejects path traversal/unsafe links (verified in `narrivy-policy` policy tests).
- Secrets via OS credential facility (`docs/security/secrets.md`); config stores refs only, never plaintext.
- IPC framing bounded: `MAX_MESSAGE_BYTES = 1 MiB`, `MAX_COLLECTION_ELEMENTS = 10_000`, `MAX_JSON_DEPTH = 64`; `Envelope::validate` + `check_payload_bounds` fail-closed on oversize.
- Supply-chain: `Cargo.lock` committed, `deny.toml` bans/wildcards/sources enforced, SBOM/checksums deferred to P10/P11 per `supply-chain.md`.

## Current verdict

**Release security remains unverified and blocked.** The historical P9
"HARDENED" conclusion and its schema-v1 evidence are quarantined. Recovery
4N-A repairs one storage/privacy boundary but is controlled non-release work;
same-UID authorization limits, dependency advisories, native qualification,
packaging, and independently registered schema-v2 evidence remain open.
