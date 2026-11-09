//! Versioned persistence with hashed, transactional migrations (OPS-006 / ADR-0013).
//!
//! SQLite (rusqlite bundled) with WAL. `schema_version` + `migrations` log;
//! each migration runs inside one transaction; applied rows store the SQL
//! hash and any mismatch aborts startup with an actionable error — never a
//! silent destructive reset.

use crate::{CoreError, store_err, unix_now_ms};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One schema migration step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub hash: String,
    pub sql: &'static str,
}

/// Canonical production migration list. Test-only lists may be built via
/// [`Migration::with_hash`].
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "config_and_pipeline_registry",
        hash: String::new(), // computed by with_hash() at apply time
        sql: "CREATE TABLE IF NOT EXISTS config (
            key TEXT PRIMARY KEY,
            value_json TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS pipeline_registry (
            id TEXT PRIMARY KEY,
            version INTEGER NOT NULL,
            definition_json TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );",
    },
    Migration {
        version: 2,
        name: "transcript_variants",
        hash: String::new(),
        sql: "CREATE TABLE IF NOT EXISTS transcript_variants (
            txid TEXT PRIMARY KEY,
            variants_json TEXT NOT NULL,
            state TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );",
    },
];

impl Migration {
    #[must_use]
    pub fn with_hash(mut self) -> Self {
        if self.hash.is_empty() {
            self.hash = sha256_hex(self.sql.as_bytes());
        }
        self
    }
}

#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    hex::encode(digest)
}

/// Open (creating directories as needed) with ADR-0013 pragmas.
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self, CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Store(format!("create db dir: {e}")))?;
        }
        let conn = Connection::open(path).map_err(|e| CoreError::Store(format!("open: {e}")))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| CoreError::Store(format!("journal_mode: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| CoreError::Store(format!("synchronous: {e}")))?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .map_err(|e| CoreError::Store(format!("busy_timeout: {e}")))?;
        Ok(Self { conn })
    }

    /// Apply `migrations` in order inside per-step transactions.
    ///
    /// Crash safety: a crash mid-migration rolls back that step's transaction;
    /// the next open re-applies from the recorded version. Applied hashes are
    /// verified — a mismatch is an actionable error, not a reset.
    pub fn apply_migrations(&self, migrations: &[Migration]) -> Result<(), CoreError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
                 CREATE TABLE IF NOT EXISTS migrations (
                    version INTEGER PRIMARY KEY,
                    hash TEXT NOT NULL,
                    name TEXT NOT NULL,
                    applied_at_ms INTEGER NOT NULL
                 );",
            )
            .map_err(|e| store_err("bootstrap meta tables", e))?;

        let current: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM migrations",
                [],
                |r| r.get(0),
            )
            .map_err(|e| store_err("read schema version", e))?;
        // Legacy/foreign schema_version row must agree; absence is fine.
        let declared: Option<i64> = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .optional()
            .map_err(|e| store_err("read declared version", e))?
            .flatten();
        // Guard against foreign/future databases: schema_version must never be
        // AHEAD of the recorded migration log (someone bumped the marker
        // without recording the step). Behind is normal forward-application.
        if let Some(declared) = declared {
            if declared > current {
                return Err(CoreError::Migration(format!(
                    "declared schema_version {declared} is ahead of the recorded migration log ({current}); refusing to touch this database"
                )));
            }
        }

        for m in migrations {
            let m = m.clone().with_hash();
            if m.version <= current {
                // Verify recorded hash for already-applied versions.
                let recorded: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT hash FROM migrations WHERE version = ?1",
                        [m.version],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| store_err("read migration hash", e))?;
                if let Some(recorded) = recorded {
                    if recorded != m.hash {
                        return Err(CoreError::Migration(format!(
                            "migration {} ({}) hash mismatch: recorded {recorded}, expected {}",
                            m.version, m.name, m.hash
                        )));
                    }
                }
                continue;
            }
            let tx = self
                .conn
                .unchecked_transaction()
                .map_err(|e| store_err("begin migration tx", e))?;
            tx.execute_batch(m.sql)
                .map_err(|e| store_err(&format!("apply v{} {}", m.version, m.name), e))?;
            tx.execute(
                "INSERT OR REPLACE INTO migrations (version, hash, name, applied_at_ms) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![m.version, m.hash, m.name, unix_now_ms() as i64],
            )
            .map_err(|e| store_err("log migration", e))?;
            tx.execute(
                "INSERT OR REPLACE INTO schema_version (version) VALUES (?1)",
                [m.version],
            )
            .map_err(|e| store_err("update schema_version", e))?;
            tx.commit().map_err(|e| store_err("commit migration", e))?;
        }
        Ok(())
    }

    /// Raw connection access for typed stores in later phases.
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// Versioned key/value config store over the `config` table.
pub struct ConfigStore<'a> {
    store: &'a Store,
}

const CONFIG_ENVELOPE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ConfigEnvelope {
    schema_version: u32,
    value: serde_json::Value,
}

impl<'a> ConfigStore<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn set_json(&self, key: &str, value: &serde_json::Value) -> Result<(), CoreError> {
        let envelope = ConfigEnvelope {
            schema_version: CONFIG_ENVELOPE_VERSION,
            value: value.clone(),
        };
        let payload =
            serde_json::to_string(&envelope).map_err(|e| store_err("serialize config", e))?;
        self.store
            .conn()
            .execute(
                "INSERT INTO config (key, value_json, updated_at_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json,
                     updated_at_ms = excluded.updated_at_ms",
                rusqlite::params![key, payload, unix_now_ms() as i64],
            )
            .map_err(|e| store_err("write config", e))?;
        Ok(())
    }

    pub fn get_json(&self, key: &str) -> Result<Option<serde_json::Value>, CoreError> {
        let payload: Option<String> = self
            .store
            .conn()
            .query_row("SELECT value_json FROM config WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| store_err("read config", e))?;
        match payload {
            None => Ok(None),
            Some(text) => {
                let envelope: ConfigEnvelope = serde_json::from_str(&text)
                    .map_err(|e| store_err("parse config envelope", e))?;
                if envelope.schema_version != CONFIG_ENVELOPE_VERSION {
                    return Err(CoreError::Store(format!(
                        "config envelope version {} unsupported",
                        envelope.schema_version
                    )));
                }
                Ok(Some(envelope.value))
            }
        }
    }
}

/// Free-function form of `Store::apply_migrations` over the default list.
pub fn apply_migrations(store: &Store) -> Result<(), CoreError> {
    store.apply_migrations(MIGRATIONS)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn migration_fresh_open_applies_all_and_reopen_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("config.db");
        {
            let store = Store::open(&db).unwrap();
            store.apply_migrations(MIGRATIONS).unwrap();
            let v: i64 = store
                .conn()
                .query_row("SELECT MAX(version) FROM migrations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v, 2);
        }
        {
            let store = Store::open(&db).unwrap();
            // Re-applying is a no-op (hashes verified, nothing re-run).
            store.apply_migrations(MIGRATIONS).unwrap();
            let count: i64 = store
                .conn()
                .query_row("SELECT COUNT(*) FROM migrations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 2);
        }
    }

    #[test]
    fn migration_hash_tamper_aborts_with_actionable_error() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("config.db");
        let store = Store::open(&db).unwrap();
        let mut tampered: Vec<Migration> = MIGRATIONS.to_vec();
        tampered[0].sql = "CREATE TABLE evil(x);";
        // First apply with the tampered list so the hash is recorded...
        store.apply_migrations(&tampered).unwrap();
        // ...then the canonical list must be REJECTED, not silently reset.
        let err = store.apply_migrations(MIGRATIONS).unwrap_err();
        assert!(err.to_string().contains("hash mismatch"), "{err}");
    }

    #[test]
    fn migration_interrupted_by_failure_recovers_on_next_open() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("config.db");
        let failing_v3 = Migration {
            version: 3,
            name: "will_fail",
            hash: String::new(),
            sql: "CREATE TABLE should_never_exist(x); THIS IS NOT SQL;",
        }
        .with_hash();
        {
            let store = Store::open(&db).unwrap();
            store.apply_migrations(MIGRATIONS).unwrap();
            // v3 fails inside its transaction; v1/v2 remain applied and intact.
            assert!(store.apply_migrations(&[failing_v3]).is_err());
            let ok: i64 = store
                .conn()
                .query_row("SELECT COUNT(*) FROM migrations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(ok, 2);
            let ghost: i64 = store
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = 'should_never_exist'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(ghost, 0, "failed migration must roll back fully");
        }
        // A good v3 on a fresh open applies cleanly; prior data survives.
        let good_v3 = Migration {
            version: 3,
            name: "add_note",
            hash: String::new(),
            sql: "ALTER TABLE config ADD COLUMN note TEXT NOT NULL DEFAULT '';",
        }
        .with_hash();
        let store = Store::open(&db).unwrap();
        store.apply_migrations(&[good_v3]).unwrap();
        let v: i64 = store
            .conn()
            .query_row("SELECT MAX(version) FROM migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 3);
    }

    #[test]
    fn config_store_roundtrip_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("config.db");
        let store = Store::open(&db).unwrap();
        store.apply_migrations(MIGRATIONS).unwrap();
        let cfg = ConfigStore::new(&store);
        assert!(cfg.get_json("activation").unwrap().is_none());
        cfg.set_json("activation", &serde_json::json!({ "mode": "toggle" }))
            .unwrap();
        cfg.set_json("activation", &serde_json::json!({ "mode": "hold" }))
            .unwrap();
        let got = cfg.get_json("activation").unwrap().unwrap();
        assert_eq!(got["mode"], "hold");
    }

    #[test]
    fn migration_forward_compatibility_v1_to_v4_preserves_user_data() {
        // Proof: migrations v1..v3 create schema and hold user data,
        // v4 adds an additive column; re-applying must preserve all prior
        // rows and not reset hashes. This is the forward-compatibility
        // guarantee required for seamless auto-updates.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("config.db");

        // Define v3 as an additive note column on config (history would use
        // its own migrations in prod; this test uses config table as proxy
        // for "user data that must survive").
        let v3 = Migration {
            version: 3,
            name: "add_note_column",
            hash: String::new(),
            sql: "ALTER TABLE config ADD COLUMN note TEXT NOT NULL DEFAULT '';",
        }
        .with_hash();

        // v4 adds a second additive column + a new table (both additive).
        let v4 = Migration {
            version: 4,
            name: "add_flag_and_audit_table",
            hash: String::new(),
            sql: "ALTER TABLE config ADD COLUMN flag INTEGER NOT NULL DEFAULT 0;
                  CREATE TABLE IF NOT EXISTS audit_log (
                    id INTEGER PRIMARY KEY,
                    event TEXT NOT NULL,
                    at_ms INTEGER NOT NULL
                  );",
        }
        .with_hash();

        // Step 1: apply v1..v2 (canonical), then v3.
        {
            let store = Store::open(&db).unwrap();
            store.apply_migrations(MIGRATIONS).unwrap();
            store.apply_migrations(std::slice::from_ref(&v3)).unwrap();

            // Snapshot: insert user data after v3.
            let cfg = ConfigStore::new(&store);
            cfg.set_json(
                "user_pref",
                &serde_json::json!({"theme":"dark","scale":1.25}),
            )
            .unwrap();
            cfg.set_json("activation", &serde_json::json!({"mode":"toggle"}))
                .unwrap();
            // Directly use new v3 column.
            store
                .conn()
                .execute(
                    "UPDATE config SET note = ?1 WHERE key = ?2",
                    rusqlite::params!["my note", "user_pref"],
                )
                .unwrap();
            // Verify snapshot.
            let note: String = store
                .conn()
                .query_row(
                    "SELECT note FROM config WHERE key = ?1",
                    ["user_pref"],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(note, "my note");
            let v: i64 = store
                .conn()
                .query_row("SELECT MAX(version) FROM migrations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v, 3);
        }

        // Step 2: reopen (simulating new daemon version) and apply v4.
        // Must not lose user_pref / activation / note.
        {
            let store = Store::open(&db).unwrap();
            // Re-apply should verify v3 hash, then apply v4.
            store.apply_migrations(std::slice::from_ref(&v3)).unwrap();
            store.apply_migrations(std::slice::from_ref(&v4)).unwrap();

            let v: i64 = store
                .conn()
                .query_row("SELECT MAX(version) FROM migrations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v, 4, "v4 must be recorded");

            let cfg = ConfigStore::new(&store);
            let pref = cfg.get_json("user_pref").unwrap().unwrap();
            assert_eq!(pref["theme"], "dark");
            assert_eq!(pref["scale"], 1.25);
            let act = cfg.get_json("activation").unwrap().unwrap();
            assert_eq!(act["mode"], "toggle");

            // v3 column still correct.
            let note: String = store
                .conn()
                .query_row(
                    "SELECT note FROM config WHERE key = ?1",
                    ["user_pref"],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(note, "my note");

            // v4 column exists with default.
            let flag: i64 = store
                .conn()
                .query_row(
                    "SELECT flag FROM config WHERE key = ?1",
                    ["user_pref"],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(flag, 0);

            // v4 new table exists and is writable (audit).
            store
                .conn()
                .execute(
                    "INSERT INTO audit_log (event, at_ms) VALUES (?1, ?2)",
                    rusqlite::params!["test", 12345],
                )
                .unwrap();
            let count: i64 = store
                .conn()
                .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 1);

            // Migrations table has 4 rows with correct hashes.
            let count: i64 = store
                .conn()
                .query_row("SELECT COUNT(*) FROM migrations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 4);
            // Hashes for v1..v2 still match canonical.
            for m in MIGRATIONS {
                let m = m.clone().with_hash();
                let recorded: String = store
                    .conn()
                    .query_row(
                        "SELECT hash FROM migrations WHERE version = ?1",
                        [m.version],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(recorded, m.hash, "hash for v{} must still match", m.version);
            }
        }

        // Step 3: idempotent re-apply of v1..v4 is a no-op and still preserves data.
        {
            let store = Store::open(&db).unwrap();
            let mut all = MIGRATIONS.to_vec();
            all.push(v3);
            all.push(v4);
            store.apply_migrations(&all).unwrap();
            let cfg = ConfigStore::new(&store);
            assert_eq!(cfg.get_json("user_pref").unwrap().unwrap()["theme"], "dark");
            let count: i64 = store
                .conn()
                .query_row("SELECT COUNT(*) FROM migrations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 4);
        }
    }

    #[test]
    fn migration_hash_chain_sealed_after_v4() {
        // Ensure that tampering any hash after v4 is still detected.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("config.db");
        let v3 = Migration {
            version: 3,
            name: "add_note",
            hash: String::new(),
            sql: "ALTER TABLE config ADD COLUMN note TEXT NOT NULL DEFAULT '';",
        }
        .with_hash();
        let v4 = Migration {
            version: 4,
            name: "add_flag",
            hash: String::new(),
            sql: "ALTER TABLE config ADD COLUMN flag INTEGER NOT NULL DEFAULT 0;",
        }
        .with_hash();
        let store = Store::open(&db).unwrap();
        store.apply_migrations(MIGRATIONS).unwrap();
        store.apply_migrations(&[v3]).unwrap();
        store.apply_migrations(std::slice::from_ref(&v4)).unwrap();
        // Tamper v4
        let mut tampered_v4 = v4.clone();
        tampered_v4.sql = "CREATE TABLE evil(x);";
        let tampered_hash = sha256_hex(tampered_v4.sql.as_bytes());
        tampered_v4.hash = tampered_hash;
        // Re-apply with tampered v4 must error (hash mismatch for already applied v4).
        // We pass the tampered migration with version 4; it will compare against
        // recorded hash for version 4 and should reject.
        let err = store.apply_migrations(&[tampered_v4]).unwrap_err();
        assert!(err.to_string().contains("hash mismatch"), "{err}");
    }
}
