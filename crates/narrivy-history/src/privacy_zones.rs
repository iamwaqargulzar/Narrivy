//! Privacy Zones (app-category deny) — PRIV-003.
//! Supports both legacy pattern+actions and mission spec app_ids+deny.
//! Persisted via SQLite privacy_zones table in history.db.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrivacyZoneAction {
    DenyHistory,
    DenyCloud,
    DenyContext,
    DisableNarrivy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyZone {
    pub id: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub app_ids: Vec<String>,
    #[serde(default)]
    pub deny: bool,
    #[serde(default)]
    pub actions: Vec<PrivacyZoneAction>,
}

impl PrivacyZone {
    /// Create mission-spec zone with app_ids + deny
    pub fn new_deny(id: impl Into<String>, app_ids: Vec<String>, deny: bool) -> Self {
        Self {
            id: id.into(),
            pattern: "".to_string(),
            app_ids,
            deny,
            actions: if deny {
                vec![PrivacyZoneAction::DenyHistory]
            } else {
                vec![]
            },
        }
    }
    /// Legacy helper with pattern + actions
    pub fn new_with_pattern(
        id: impl Into<String>,
        pattern: impl Into<String>,
        actions: Vec<PrivacyZoneAction>,
    ) -> Self {
        Self {
            id: id.into(),
            pattern: pattern.into(),
            app_ids: Vec::new(),
            deny: actions.contains(&PrivacyZoneAction::DenyHistory)
                || actions.contains(&PrivacyZoneAction::DenyContext)
                || actions.contains(&PrivacyZoneAction::DisableNarrivy),
            actions,
        }
    }
}

/// Matcher — owns zones and persistence.
pub struct ZoneMatcher {
    zones: Vec<PrivacyZone>,
}

impl ZoneMatcher {
    pub fn new(zones: Vec<PrivacyZone>) -> Self {
        Self { zones }
    }
    pub fn zones(&self) -> &[PrivacyZone] {
        &self.zones
    }
    pub fn len(&self) -> usize {
        self.zones.len()
    }
    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }

    pub fn add_zone(&mut self, zone: PrivacyZone) {
        // replace if id exists
        if let Some(pos) = self.zones.iter().position(|z| z.id == zone.id) {
            self.zones[pos] = zone;
        } else {
            self.zones.push(zone);
        }
    }
    pub fn remove_zone(&mut self, id: &str) {
        self.zones.retain(|z| z.id != id);
    }

    /// Check if app_id is allowed: deny overrides allow. Returns false if any matching zone denies.
    pub fn is_allowed(&self, app_id: &str) -> bool {
        for zone in &self.zones {
            if Self::zone_matches(zone, app_id) && Self::zone_denies(zone) {
                return false;
            }
        }
        true
    }

    fn zone_matches(zone: &PrivacyZone, app_id: &str) -> bool {
        // mission spec: app_ids exact match
        if !zone.app_ids.is_empty() && zone.app_ids.iter().any(|a| a == app_id) {
            return true;
        }
        // legacy pattern substring case-insensitive
        if !zone.pattern.is_empty() {
            let pat = zone.pattern.trim().to_ascii_lowercase();
            if !pat.is_empty() && app_id.to_ascii_lowercase().contains(&pat) {
                return true;
            }
        }
        // if both empty, not matching (wildcard disabled)
        false
    }

    fn zone_denies(zone: &PrivacyZone) -> bool {
        if zone.deny {
            return true;
        }
        // legacy actions that deny history/context/cloud
        zone.actions.iter().any(|a| {
            matches!(
                a,
                PrivacyZoneAction::DenyHistory
                    | PrivacyZoneAction::DenyContext
                    | PrivacyZoneAction::DenyCloud
                    | PrivacyZoneAction::DisableNarrivy
            )
        })
    }

    // Persistence via history.db SQLite

    fn open_conn(path: &Path) -> Result<rusqlite::Connection, crate::HistoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| crate::HistoryError::Storage(format!("create dir: {e}")))?;
        }
        let conn = rusqlite::Connection::open(path)
            .map_err(|e| crate::HistoryError::Storage(format!("open db: {e}")))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| crate::HistoryError::Storage(format!("journal_mode: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| crate::HistoryError::Storage(format!("synchronous: {e}")))?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .map_err(|e| crate::HistoryError::Storage(format!("busy_timeout: {e}")))?;
        Ok(conn)
    }

    fn ensure_schema(conn: &rusqlite::Connection) -> Result<(), crate::HistoryError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS privacy_zones (
                id TEXT PRIMARY KEY,
                app_ids_json TEXT NOT NULL,
                pattern TEXT NOT NULL,
                deny INTEGER NOT NULL,
                actions_json TEXT NOT NULL
            );",
        )
        .map_err(|e| crate::HistoryError::Storage(format!("create privacy_zones: {e}")))?;
        Ok(())
    }

    pub fn save_to_db(&self, db_path: &Path) -> Result<(), crate::HistoryError> {
        let path = if db_path.is_dir() {
            db_path.join("history.db")
        } else {
            db_path.to_path_buf()
        };
        let conn = Self::open_conn(&path)?;
        Self::ensure_schema(&conn)?;
        // also ensure history tables for coexistence
        conn.execute_batch("CREATE TABLE IF NOT EXISTS history_entries (id TEXT PRIMARY KEY, ts INTEGER NOT NULL, pipeline_id TEXT NOT NULL, raw TEXT NOT NULL, corrected TEXT NOT NULL, delivered TEXT NOT NULL, app_id TEXT NOT NULL); CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at_ms INTEGER NOT NULL);")
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        conn.execute("DELETE FROM privacy_zones", [])
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        for z in &self.zones {
            let app_ids_json = serde_json::to_string(&z.app_ids)
                .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
            let actions_json = serde_json::to_string(&z.actions)
                .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
            conn.execute(
                "INSERT OR REPLACE INTO privacy_zones (id, app_ids_json, pattern, deny, actions_json) VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![z.id, app_ids_json, z.pattern, if z.deny {1} else {0}, actions_json],
            ).map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    pub fn load_from_db(db_path: &Path) -> Result<Self, crate::HistoryError> {
        let path = if db_path.is_dir() {
            db_path.join("history.db")
        } else {
            db_path.to_path_buf()
        };
        if !path.exists() {
            return Ok(Self::new(vec![]));
        }
        let conn = Self::open_conn(&path)?;
        Self::ensure_schema(&conn)?;
        let mut stmt = conn
            .prepare("SELECT id, app_ids_json, pattern, deny, actions_json FROM privacy_zones")
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let app_ids_json: String = row.get(1)?;
                let pattern: String = row.get(2)?;
                let deny_i: i64 = row.get(3)?;
                let actions_json: String = row.get(4)?;
                Ok((id, app_ids_json, pattern, deny_i, actions_json))
            })
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        let mut zones = Vec::new();
        for r in rows {
            let (id, app_ids_json, pattern, deny_i, actions_json) =
                r.map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
            let app_ids: Vec<String> = serde_json::from_str(&app_ids_json).unwrap_or_default();
            let actions: Vec<PrivacyZoneAction> =
                serde_json::from_str(&actions_json).unwrap_or_default();
            zones.push(PrivacyZone {
                id,
                pattern,
                app_ids,
                deny: deny_i != 0,
                actions,
            });
        }
        Ok(Self::new(zones))
    }

    pub fn clear_db(db_path: &Path) -> Result<(), crate::HistoryError> {
        let path = if db_path.is_dir() {
            db_path.join("history.db")
        } else {
            db_path.to_path_buf()
        };
        if !path.exists() {
            return Ok(());
        }
        let conn = Self::open_conn(&path)?;
        Self::ensure_schema(&conn)?;
        conn.execute("DELETE FROM privacy_zones", [])
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn privacy_zone_deny_overrides_allow() {
        // mission spec: deny overrides allow
        let allow_zone = PrivacyZone {
            id: "allow".to_string(),
            pattern: "".to_string(),
            app_ids: vec!["app_x".to_string()],
            deny: false,
            actions: vec![],
        };
        let deny_zone = PrivacyZone {
            id: "deny".to_string(),
            pattern: "".to_string(),
            app_ids: vec!["app_x".to_string()],
            deny: true,
            actions: vec![PrivacyZoneAction::DenyHistory],
        };
        let matcher = ZoneMatcher::new(vec![allow_zone.clone(), deny_zone]);
        assert!(!matcher.is_allowed("app_x"), "deny must override allow");
        // allow alone passes
        let matcher2 = ZoneMatcher::new(vec![allow_zone]);
        assert!(matcher2.is_allowed("app_x"));
        // no matching zone defaults allow
        let matcher3 = ZoneMatcher::new(vec![]);
        assert!(matcher3.is_allowed("any"));
    }

    #[test]
    fn privacy_zone_persistence_roundtrip() -> TestResult {
        let dir = tempdir()?;
        let db = dir.path().join("history.db");
        let z1 = PrivacyZone {
            id: "z1".to_string(),
            pattern: "bank".to_string(),
            app_ids: vec!["app1".to_string()],
            deny: true,
            actions: vec![PrivacyZoneAction::DenyHistory],
        };
        let z2 = PrivacyZone {
            id: "z2".to_string(),
            pattern: "".to_string(),
            app_ids: vec!["app2".to_string()],
            deny: false,
            actions: vec![],
        };
        let matcher = ZoneMatcher::new(vec![z1.clone(), z2.clone()]);
        matcher.save_to_db(&db)?;
        let loaded = ZoneMatcher::load_from_db(&db)?;
        assert_eq!(loaded.zones().len(), 2);
        assert_eq!(loaded.zones()[0], z1);
        assert_eq!(loaded.zones()[1], z2);
        assert!(!loaded.is_allowed("app1"));
        assert!(loaded.is_allowed("app2"));
        Ok(())
    }

    #[test]
    fn privacy_zone_pattern_matching() {
        let zone = PrivacyZone {
            id: "p".to_string(),
            pattern: "bank".to_string(),
            app_ids: vec![],
            deny: true,
            actions: vec![PrivacyZoneAction::DenyHistory],
        };
        let m = ZoneMatcher::new(vec![zone]);
        assert!(!m.is_allowed("com.bank.app"));
        assert!(!m.is_allowed("BANK"));
        assert!(m.is_allowed("other"));
    }

    #[test]
    fn privacy_zone_app_ids_exact() {
        let zone = PrivacyZone {
            id: "a".to_string(),
            pattern: "".to_string(),
            app_ids: vec!["app_x".to_string()],
            deny: true,
            actions: vec![],
        };
        let m = ZoneMatcher::new(vec![zone]);
        assert!(!m.is_allowed("app_x"));
        assert!(m.is_allowed("app_x_extra"));
        assert!(m.is_allowed("other"));
    }

    #[test]
    fn privacy_is_allowed_persisted_deny_overrides() -> TestResult {
        let dir = tempdir()?;
        let db = dir.path().join("history.db");
        // Save allow then deny for same app, reload should keep deny
        let allow = PrivacyZone {
            id: "allow".to_string(),
            pattern: "".to_string(),
            app_ids: vec!["app_x".to_string()],
            deny: false,
            actions: vec![],
        };
        let deny = PrivacyZone {
            id: "deny".to_string(),
            pattern: "".to_string(),
            app_ids: vec!["app_x".to_string()],
            deny: true,
            actions: vec![],
        };
        let m = ZoneMatcher::new(vec![allow, deny]);
        m.save_to_db(&db)?;
        let loaded = ZoneMatcher::load_from_db(&db)?;
        assert!(!loaded.is_allowed("app_x"));
        Ok(())
    }
}
