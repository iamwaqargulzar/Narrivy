//! Dictionary (Hunspell/SymSpell local, no cloud) — FUN-041.
//!
//! Spoken aliases map to canonical forms where safe.
//! Scope: essential aliases only, no learning/memory (deferred).
//! Storage: JSON `dictionary.json` versioned {schema_version:1, aliases:{...}}.
//! Expand: whole-word aliases case-insensitive, no regex, longest match first.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 1;

/// Single entry view for compatibility (not stored directly).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub canonical: String,
    pub aliases: Vec<String>,
}

/// Persisted format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedDictionary {
    schema_version: u32,
    aliases: BTreeMap<String, String>,
}

/// Dictionary with spoken-form -> replacement mapping.
/// Spoken form is stored lowercased for case-insensitive matching.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dictionary {
    /// lowercased spoken form -> replacement (canonical)
    aliases: BTreeMap<String, String>,
}

impl Dictionary {
    /// Create empty dictionary.
    pub fn new() -> Self {
        Self {
            aliases: BTreeMap::new(),
        }
    }

    /// Number of aliases.
    pub fn len(&self) -> usize {
        self.aliases.len()
    }

    /// True if no aliases.
    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }

    /// Immutable view of inner map (lowercased spoken -> replacement).
    pub fn aliases_map(&self) -> &BTreeMap<String, String> {
        &self.aliases
    }

    /// Legacy helper: return entries as Vec<DictionaryEntry> grouping by canonical.
    /// Each canonical maps to list of aliases (reverse view).
    pub fn entries(&self) -> Vec<DictionaryEntry> {
        let mut by_canonical: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (spoken, canonical) in &self.aliases {
            by_canonical
                .entry(canonical.clone())
                .or_default()
                .push(spoken.clone());
        }
        by_canonical
            .into_iter()
            .map(|(canonical, aliases)| DictionaryEntry { canonical, aliases })
            .collect()
    }

    /// Add alias mapping spoken form -> replacement.
    /// Spoken form is lowercased and trimmed; empty spoken or replacement returns error.
    /// Overwrites existing.
    pub fn add_alias(
        &mut self,
        spoken: impl Into<String>,
        replacement: impl Into<String>,
    ) -> Result<(), String> {
        let spoken_raw: String = spoken.into();
        let replacement_raw: String = replacement.into();
        let spoken_trimmed = spoken_raw.trim().to_string();
        let replacement_trimmed = replacement_raw.trim().to_string();
        if spoken_trimmed.is_empty() {
            return Err("spoken form must not be empty".to_owned());
        }
        if replacement_trimmed.is_empty() {
            return Err("replacement must not be empty".to_owned());
        }
        let key = spoken_trimmed.to_ascii_lowercase();
        self.aliases.insert(key, replacement_trimmed);
        Ok(())
    }

    /// Convenience alias for skeleton compatibility.
    pub fn add(&mut self, entry: DictionaryEntry) {
        for alias in entry.aliases {
            let key = alias.trim().to_ascii_lowercase();
            if !key.is_empty() && !entry.canonical.trim().is_empty() {
                self.aliases.insert(key, entry.canonical.clone());
            }
        }
    }

    /// Remove alias by spoken form (case-insensitive). Returns true if removed.
    pub fn remove_alias(&mut self, spoken: &str) -> bool {
        let key = spoken.trim().to_ascii_lowercase();
        self.aliases.remove(&key).is_some()
    }

    /// Expand text by replacing whole-word aliases case-insensitively.
    /// No regex; word-boundary defined as non-alphanumeric surrounding.
    /// Longest aliases first to avoid partial overlap.
    pub fn expand(&self, text: &str) -> String {
        if self.aliases.is_empty() || text.is_empty() {
            return text.to_owned();
        }
        // Collect sorted aliases by length descending (longest first)
        let mut sorted: Vec<(&String, &String)> = self.aliases.iter().collect();
        sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(b.0)));

        let mut result = text.to_owned();
        for (spoken_lower, replacement) in sorted {
            result = replace_whole_word_case_insensitive(&result, spoken_lower, replacement);
        }
        result
    }

    /// Persist to `dictionary.json` at given directory (creates dir if needed).
    /// File format: {"schema_version":1,"aliases":{...}}
    pub fn save_to_dir(&self, dir: impl AsRef<Path>) -> Result<(), String> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|e| format!("create_dir_all: {e}"))?;
        let path = dir.join("dictionary.json");
        let persisted = PersistedDictionary {
            schema_version: SCHEMA_VERSION,
            aliases: self.aliases.clone(),
        };
        let json =
            serde_json::to_string_pretty(&persisted).map_err(|e| format!("serialize: {e}"))?;
        // Atomic write via temp file + rename
        let tmp = dir.join("dictionary.json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| format!("write tmp: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))?;
        Ok(())
    }

    /// Load from `dictionary.json` at given directory.
    /// If file missing, returns empty dictionary.
    /// Validates schema_version == 1.
    pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref();
        let path = dir.join("dictionary.json");
        if !path.exists() {
            return Ok(Self::new());
        }
        let data = std::fs::read_to_string(&path).map_err(|e| format!("read: {e}"))?;
        let persisted: PersistedDictionary =
            serde_json::from_str(&data).map_err(|e| format!("parse: {e}"))?;
        if persisted.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version {} expected {}",
                persisted.schema_version, SCHEMA_VERSION
            ));
        }
        // Ensure keys are lowercased (normalize if file had mixed case)
        let mut normalized = BTreeMap::new();
        for (k, v) in persisted.aliases {
            let lk = k.trim().to_ascii_lowercase();
            if !lk.is_empty() {
                normalized.insert(lk, v);
            }
        }
        Ok(Self {
            aliases: normalized,
        })
    }

    /// Save to exact file path (for tests). Directory must exist.
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let persisted = PersistedDictionary {
            schema_version: SCHEMA_VERSION,
            aliases: self.aliases.clone(),
        };
        let json =
            serde_json::to_string_pretty(&persisted).map_err(|e| format!("serialize: {e}"))?;
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
        }
        let tmp = path.as_ref().with_extension("tmp");
        std::fs::write(&tmp, &json).map_err(|e| format!("write tmp: {e}"))?;
        std::fs::rename(&tmp, path.as_ref()).map_err(|e| format!("rename: {e}"))?;
        Ok(())
    }

    /// Load from exact file path.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let p = path.as_ref();
        if !p.exists() {
            return Ok(Self::new());
        }
        let data = std::fs::read_to_string(p).map_err(|e| format!("read: {e}"))?;
        let persisted: PersistedDictionary =
            serde_json::from_str(&data).map_err(|e| format!("parse: {e}"))?;
        if persisted.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version {} expected {}",
                persisted.schema_version, SCHEMA_VERSION
            ));
        }
        let mut normalized = BTreeMap::new();
        for (k, v) in persisted.aliases {
            let lk = k.trim().to_ascii_lowercase();
            if !lk.is_empty() {
                normalized.insert(lk, v);
            }
        }
        Ok(Self {
            aliases: normalized,
        })
    }
}

/// Replace whole-word occurrences of `needle_lower` (already lowercased) in `haystack`
/// case-insensitively, preserving replacement case. Word boundary = non-alphanumeric or string edge.
/// No regex.
fn replace_whole_word_case_insensitive(
    haystack: &str,
    needle_lower: &str,
    replacement: &str,
) -> String {
    if needle_lower.is_empty() {
        return haystack.to_owned();
    }
    let haystack_lower = haystack.to_ascii_lowercase();
    let needle_len = needle_lower.len();
    let hay_bytes = haystack.as_bytes();
    let hay_lower_bytes = haystack_lower.as_bytes();
    let needle_bytes = needle_lower.as_bytes();

    let mut out = String::with_capacity(haystack.len());
    let mut i = 0usize;
    while i < haystack.len() {
        // Quick check: does haystack_lower[i..i+needle_len] == needle_lower ?
        if i + needle_len <= haystack.len()
            && &hay_lower_bytes[i..i + needle_len] == needle_bytes
            && is_word_boundary_before(hay_bytes, i)
            && is_word_boundary_after(hay_bytes, i + needle_len)
        {
            out.push_str(replacement);
            i += needle_len;
        } else {
            // advance by one char (preserve UTF-8 boundary)
            if let Some(ch) = haystack[i..].chars().next() {
                out.push(ch);
                i += ch.len_utf8();
            } else {
                break;
            }
        }
    }
    out
}

#[inline]
fn is_alpha_num(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn is_word_boundary_before(bytes: &[u8], pos: usize) -> bool {
    if pos == 0 {
        return true;
    }
    // Find previous char start (since we advance char-wise, pos is always char boundary)
    // Previous byte is last byte of previous char; for ASCII it's fine; for non-ASCII we treat as word char if not ascii alnum?
    // Simpler: check immediate preceding byte if ascii alnum then not boundary; else boundary.
    // For multi-byte preceding char, its last byte will be >=128, not ascii alnum => boundary (acceptable)
    let prev = bytes[pos - 1];
    !is_alpha_num(prev)
}

fn is_word_boundary_after(bytes: &[u8], pos: usize) -> bool {
    if pos >= bytes.len() {
        return true;
    }
    let next = bytes[pos];
    !is_alpha_num(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn checked<T, E: std::fmt::Display>(value: Result<T, E>) -> Result<T, std::io::Error> {
        value.map_err(|error| std::io::Error::other(error.to_string()))
    }

    fn rejected<T, E>(value: Result<T, E>, message: &'static str) -> Result<E, std::io::Error> {
        match value {
            Err(error) => Ok(error),
            Ok(_) => Err(std::io::Error::other(message)),
        }
    }

    #[test]
    fn alias_basic_expansion() -> TestResult {
        let mut d = Dictionary::new();
        checked(d.add_alias("my company", "Acme Corp"))?;
        assert_eq!(
            d.expand("I work at my company today"),
            "I work at Acme Corp today"
        );
        Ok(())
    }

    #[test]
    fn alias_case_insensitive() -> TestResult {
        let mut d = Dictionary::new();
        checked(d.add_alias("my company", "Acme Corp"))?;
        assert_eq!(d.expand("MY COMPANY is great"), "Acme Corp is great");
        assert_eq!(d.expand("My Company"), "Acme Corp");
        assert_eq!(d.expand("mY cOmPaNy"), "Acme Corp");
        Ok(())
    }

    #[test]
    fn alias_word_boundary() -> TestResult {
        let mut d = Dictionary::new();
        checked(d.add_alias("my company", "Acme Corp"))?;
        // Should NOT replace when glued to alphanumerics
        assert_eq!(d.expand("my companyX"), "my companyX");
        assert_eq!(d.expand("Xmy company"), "Xmy company");
        assert_eq!(d.expand("my company, indeed"), "Acme Corp, indeed");
        assert_eq!(d.expand("hey-my company!"), "hey-Acme Corp!");
        // Substring inside longer word should not match partial alias word
        let mut d2 = Dictionary::new();
        checked(d2.add_alias("cat", "dog"))?;
        assert_eq!(d2.expand("concatenate"), "concatenate");
        assert_eq!(d2.expand("cat catapult cat"), "dog catapult dog");
        Ok(())
    }

    #[test]
    fn persistence_round_trip() -> TestResult {
        let dir = tempdir()?;
        let mut d = Dictionary::new();
        checked(d.add_alias("my company", "Acme Corp"))?;
        checked(d.add_alias("asap", "as soon as possible"))?;
        checked(d.save_to_dir(dir.path()))?;
        // Verify file format
        let file_path = dir.path().join("dictionary.json");
        assert!(file_path.exists());
        let raw = std::fs::read_to_string(&file_path)?;
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["aliases"]["my company"], "Acme Corp");
        // Load back
        let loaded = checked(Dictionary::load_from_dir(dir.path()))?;
        assert_eq!(loaded.aliases, d.aliases);
        assert_eq!(
            loaded.expand("my company asap"),
            "Acme Corp as soon as possible"
        );
        Ok(())
    }

    #[test]
    fn remove_alias_works() -> TestResult {
        let mut d = Dictionary::new();
        checked(d.add_alias("my company", "Acme Corp"))?;
        assert_eq!(d.expand("my company"), "Acme Corp");
        assert!(d.remove_alias("my company"));
        assert_eq!(d.expand("my company"), "my company");
        // case-insensitive remove
        checked(d.add_alias("my company", "Acme Corp"))?;
        assert!(d.remove_alias("MY COMPANY"));
        assert!(d.expand("my company").contains("my company"));
        // removing non-existent returns false
        assert!(!d.remove_alias("nonexistent"));
        Ok(())
    }

    #[test]
    fn multiple_aliases_longest_first() -> TestResult {
        let mut d = Dictionary::new();
        checked(d.add_alias("my", "MY_REPL"))?;
        checked(d.add_alias("my company", "Acme Corp"))?;
        // longest first should win for overlapping
        assert_eq!(d.expand("my company"), "Acme Corp");
        Ok(())
    }

    #[test]
    fn spoken_lowercased_storage() -> TestResult {
        let mut d = Dictionary::new();
        checked(d.add_alias("My Company", "Acme Corp"))?;
        assert!(d.aliases.contains_key("my company"));
        assert!(!d.aliases.contains_key("My Company"));
        Ok(())
    }

    #[test]
    fn empty_and_error_cases() {
        let mut d = Dictionary::new();
        assert!(d.add_alias("", "x").is_err());
        assert!(d.add_alias("  ", "x").is_err());
        assert!(d.add_alias("spoken", "").is_err());
        assert!(d.add_alias("spoken", "   ").is_err());
        assert_eq!(d.expand(""), "");
    }

    #[test]
    fn schema_version_rejected() -> TestResult {
        let dir = tempdir()?;
        let bad = serde_json::json!({"schema_version": 999, "aliases": {"hi":"hello"}});
        std::fs::write(
            dir.path().join("dictionary.json"),
            serde_json::to_string(&bad)?,
        )?;
        let err = rejected(
            Dictionary::load_from_dir(dir.path()),
            "unsupported dictionary schema was accepted",
        )?;
        assert!(err.contains("unsupported schema_version"));
        Ok(())
    }

    #[test]
    fn missing_file_returns_empty() -> TestResult {
        let dir = tempdir()?;
        let d = checked(Dictionary::load_from_dir(dir.path()))?;
        assert!(d.is_empty());
        Ok(())
    }
}
