//! Theme persistence (System/Light/Dark) for the desktop shell.
//!
//! Storage is a tiny versioned JSON file so the preference survives restarts
//! and is inspectable by qualification tooling:
//! - production: `<app config dir>/theme.json`;
//! - test/qualification: path override via `NARRIVY_THEME_FILE` (env-safe:
//!   unset means the env var simply does not participate).

#![forbid(unsafe_code)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ThemeFile {
    version: u32,
    mode: ThemeMode,
}

/// Read/write the persisted theme preference.
pub struct ThemeStore {
    path: PathBuf,
}

impl ThemeStore {
    /// Test-mode resolution: `NARRIVY_THEME_FILE` overrides the config dir.
    pub fn resolve(config_dir: Option<PathBuf>) -> Self {
        if let Some(path) = std::env::var_os("NARRIVY_THEME_FILE")
            && !path.is_empty()
        {
            return Self {
                path: PathBuf::from(path),
            };
        }
        let base = config_dir.unwrap_or_else(|| PathBuf::from("."));
        Self {
            path: base.join("theme.json"),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Load the persisted mode. Missing/corrupt files degrade to System
    /// (never panic; a corrupt file is left in place for diagnostics).
    pub fn load(&self) -> ThemeMode {
        let data = match std::fs::read(&self.path) {
            Ok(data) => data,
            Err(_) => return ThemeMode::System,
        };
        match serde_json::from_slice::<ThemeFile>(&data) {
            Ok(file) if matches!(file.version, 1) => file.mode,
            _ => ThemeMode::System,
        }
    }

    /// Persist the mode atomically (tmp + rename), creating parent dirs.
    pub fn save(&self, mode: ThemeMode) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create theme dir: {e}"))?;
        }
        let file = ThemeFile { version: 1, mode };
        let data =
            serde_json::to_vec_pretty(&file).map_err(|e| format!("theme serialize failed: {e}"))?;
        let tmp = self
            .path
            .with_extension(format!("json.tmp.{}", std::process::id()));
        {
            let mut f =
                std::fs::File::create(&tmp).map_err(|e| format!("theme tmp create: {e}"))?;
            use std::io::Write;
            f.write_all(&data)
                .map_err(|e| format!("theme tmp write: {e}"))?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("theme rename: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_store_roundtrip_and_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("nested/theme.json");
        let store = ThemeStore { path: path.clone() };
        assert_eq!(
            store.load(),
            ThemeMode::System,
            "missing file degrades to system"
        );
        store
            .save(ThemeMode::Light)
            .map_err(std::io::Error::other)?;
        assert_eq!(ThemeStore { path }.load(), ThemeMode::Light);
        Ok(())
    }

    #[test]
    fn theme_store_corrupt_file_degrades_to_system() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("theme.json");
        std::fs::write(&path, b"{ not json")?;
        assert_eq!(ThemeStore { path }.load(), ThemeMode::System);
        Ok(())
    }

    #[test]
    fn reusable_theme_store_has_no_tauri_api_dependency() {
        let forbidden = ["tauri", "::"].concat();
        assert!(!include_str!("theme.rs").contains(&forbidden));
    }
}
