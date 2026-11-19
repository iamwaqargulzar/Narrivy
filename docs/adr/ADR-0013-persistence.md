# ADR-0013 — Persistence

**Status:** Accepted (P1, 2026-08-21)  
**Context:** the legacy P1 planning amendment persistence must specify actual storage technology, locking, and recovery, distinguishing active-session recovery from optional user history.

> **Recovery audit note (2026-08-30):** Recovery 4N-A moves new journal writes
> to the private XDG state/data location, enforces the 15-minute TTL and count
> cap at startup, and prevents History commands from reading recovery data.
> Legacy runtime-path migration, automatic startup Inbox materialization, and
> the separately retained optional History implementation remain blocked.

## Decision

### Technology

- **SQLite** via `rusqlite` (bundled, no external daemon) for structured stores: `config`, `history`, `learning/dictionary/snippets`, and technical cache metadata. One database per store under XDG locations (`$XDG_CONFIG_HOME/narrivy/config.db`, `$XDG_DATA_HOME/narrivy/history.db`, etc.) — not a single monolithic DB, to keep backup/clear granularity and avoid locking contention.
- **File-per-transaction JSON + atomic rename** for the **active-session recovery journal** at `$XDG_STATE_HOME/narrivy/journal/<txid>.json` (or `$XDG_DATA_HOME/narrivy/journal` if `STATE_HOME` unset). The journal is the only place that holds in-flight final text; it is **not** History. Each transaction writes `pending → committed → delivered` with `fsync`+`rename` atomicity.
- **Plain files + checksums** for model artifacts and compiled caches under `$XDG_CACHE_HOME/narrivy/models/<model>/<hash>/` and `compiled/<device>/<hash>/`.

### Locking & transactions

- SQLite opened with `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000 ms`. All writes go through a single `narrivyd` connection pool (size 1 for config/history to serialize migrations); retries on `SQLITE_BUSY` with bounded backoff.
- For multi-file operations (e.g., install model: download → verify → move → update DB), we write to a staging dir, `fsync`, then `rename` into place and finally update SQLite in one transaction. Readers never see a partially downloaded model as `installed`.

### Recovery after interrupted write/migration

- **Migrations:** `schema_version` table + `migrations` log. Each migration runs inside a SQLite transaction; on crash, the transaction rolls back and the next start re-applies from the recorded version. The migration directory `migrations/<version>.sql` is hashed; mismatch aborts startup with actionable error and does not silently reset.
- **Journal:** On startup, daemon scans `journal/`; any recoverable entry whose
  creation-time-derived 15-minute TTL has not elapsed may be surfaced as
  **recoverable** (Inbox). The current record schema has no persisted
  `expires_at`; adding one requires a versioned migration. Journal entries are
  `fsync`'d before risky delivery. `fsync`+`rename` ensures never a partial JSON.
- **History:** Corrupt history rows are quarantined (`history_corrupt_<timestamp>.db`), not deleted, and `doctor` reports the issue. The user can `--recover` or `--clear`.

### Backup / rollback

- Before any destructive migration or `purge`, an automatic backup `backup/<store>-<timestamp>.db` is created via SQLite backup API (not file copy while open). Rollback restores the backup and re-creates the WAL checkpoint.
- User-initiated backup: `narrivy-cli backup --output <path>` produces a tar with config/history/journal metadata (no audio/transcript leakage beyond what user explicitly includes).

### Active-session recovery vs optional history

| Concern | Active-session recovery journal | Optional user History |
|---|---|---|
| Location | `journal/*.json` (state) | `history.db` (data) |
| Retention | TTL 15 min and strict 100-entry cap across all states; terminal rows remain available only within that bounded window | User-configurable (off / 7d / 30d / forever), respected by Privacy Zones |
| Deletion | Automatic after delivery or TTL | Explicit user action; `history --clear` |
| Privacy default | Always active, no user content beyond final text for that transaction, not subject to History off | Off respects `PRIV-002`; disabling History does NOT disable journal |
| Encryption | Plain JSON under user 0700 dir (no extra crypto for 1.0) | Same; no plaintext secrets ever (ADR-0011) |

Turning History off does not make Narrivy fragile: journal still guarantees recoverability for the active transaction window.

## Consequences

- Implementation in `crates/narrivy-core` + `services/narrivyd` must use the above XDG paths, not scattered literals.
- `narrivy-platform-api` exposes a storage capability for path resolution, not direct file I/O in core.
- P9 hardening must test migration interruption (kill -9 mid-migration), journal recovery, corrupt history, and backup/restore.

## Alternatives considered

- Single JSON config file (rejected — no atomic migrations, no query).
- Single DB for everything (rejected — harder to clear History without touching journal, larger lock scope).
- Flat files only (rejected — no efficient query for history/learning, no WAL).

## Verification

Recorded as of 2026-09-28; the originally named integration files
`tests/persistence_migration.rs` and `tests/journal_recovery.rs` were never
created. The behaviour is covered by in-crate tests at their real locations:

- `crates/narrivy-core/src/persistence.rs:298` `migration_interrupted_by_failure_recovers_on_next_open`, plus `:284` `migration_hash_tamper_aborts_with_actionable_error` and `:362` `migration_forward_compatibility_v1_to_v4_preserves_user_data`.
- `adapters/journal-unix/src/lib.rs:1127` `hundred_entry_torn_write_recovery_remains_content_correct` covers no-loss after a torn write before delivery, and `:908` `malformed_and_identity_mismatched_json_are_quarantined` covers journal corruption quarantine.

Still unimplemented against this ADR's text, recorded as gaps rather than
claims:

- `narrivy-cli backup --output <path>` does not exist. `apps/narrivy-cli/src/`
  contains only `main.rs` and `support.rs`.
- The pre-destructive-migration / pre-purge automatic SQLite backup
  (`backup/<store>-<timestamp>.db`) is not implemented; no `backup` symbol
  exists in `crates/narrivy-history` or `crates/narrivy-core`.
- History corruption quarantine to `history_corrupt_<timestamp>.db` is not
  implemented; the only quarantine in the tree is the recovery **journal**
  quarantine in `adapters/journal-unix/src/lib.rs:208`.
- Migrations are inline `MIGRATIONS` constants hashed by
  `Migration::with_hash` (`crates/narrivy-core/src/persistence.rs:24-58`), not
  a `migrations/<version>.sql` directory. The hashing decision holds; the file
  layout in the Decision section above does not.
