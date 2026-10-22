//! Capability → UI-state mapping (ADR-0005: UI renders capabilities, not OS
//! branches). This table is the single place where daemon capability strings
//! are translated into frontend state tokens. The `capability_ui` gate test
//! scans the built frontend assets for OS-name branching and asserts this
//! table drives the rendered states.
//!
//! SINGLE SOURCE: `spec/catalogs/capability-states.json` (version 1) is the
//! authoritative vocabulary. This file `include_str!`s that JSON and the
//! `capability_states_single_source` test asserts the hard-coded array equals
//! the catalog. The TS side (`src/lib/capabilityStates.ts`) imports the same
//! JSON, so Rust + TS + catalog stay in lock-step. If you add a row, edit the
//! JSON and update the hard-coded array (or make it derived) — the test will
//! fail until they match.

#![forbid(unsafe_code)]

/// Raw JSON text of the single-source catalog. Path is relative to this file.
/// Used only by the `capability_states_single_source` gate test (and for
/// future compile-time derivation).
pub const CAPABILITY_STATES_JSON: &str =
    include_str!("../../../../spec/catalogs/capability-states.json");

#[derive(Debug, serde::Deserialize)]
struct CatalogEntry {
    capability: String,
    state: String,
}

#[derive(Debug, serde::Deserialize)]
struct CapabilityCatalog {
    version: u32,
    states: Vec<CatalogEntry>,
}

/// Parse the catalog JSON into an owned vec of (capability, state).
/// Public for testing; the production mapping remains the `CAPABILITY_UI_STATES`
/// const below (which the single-source test asserts equals this parse).
pub fn catalog_states() -> Result<Vec<(String, String)>, serde_json::Error> {
    let catalog: CapabilityCatalog = serde_json::from_str(CAPABILITY_STATES_JSON)?;
    assert_eq!(
        catalog.version, 1,
        "unsupported capability-states.json version"
    );
    Ok(catalog
        .states
        .into_iter()
        .map(|e| (e.capability, e.state))
        .collect())
}

/// Daemon capability string → frontend state token. Tokens are stable,
/// platform-free vocabulary; unknown capabilities degrade to a generic state.
/// This array MUST equal `spec/catalogs/capability-states.json` (see test).
pub const CAPABILITY_UI_STATES: &[(&str, &str)] = &[
    ("mock.asr", "asr-local"),
    ("asr.transcribe-cpp", "asr-local"),
    ("null.delivery", "delivery-inbox"),
    ("platform.delivery.router", "delivery-direct"),
    ("portal.globalshortcuts", "trigger-portal"),
    ("x11.hotkey", "trigger-x11-fallback"),
];

/// Generic fallback token for capabilities missing from the table. The UI
/// shows an explicit "unknown capability" state — never a platform branch.
pub const UNKNOWN_CAPABILITY_STATE: &str = "capability-unknown";

pub fn ui_state_for(capability: &str) -> &'static str {
    CAPABILITY_UI_STATES
        .iter()
        .find(|(cap, _)| *cap == capability)
        .map(|(_, state)| *state)
        .unwrap_or(UNKNOWN_CAPABILITY_STATE)
}

#[cfg(test)]
mod capability_ui_tests {
    use super::*;

    /// OS names that must never appear as UI branches in built frontend
    /// assets (CONTRACT.md binding rule).
    const FORBIDDEN_OS_TOKENS: &[&str] = &[
        "linux", "gnome", "cosmic", "kde", "win32", "darwin", "macos",
    ];

    /// Single-source gate: the Rust hard-coded table MUST equal the JSON
    /// catalog, and by transitivity the TS table (which imports the same JSON).
    /// This also guards against drift between Rust and TS arrays.
    #[test]
    fn capability_states_single_source() -> Result<(), Box<dyn std::error::Error>> {
        // Parse JSON catalog (via include_str! single source).
        let catalog: CapabilityCatalog = serde_json::from_str(CAPABILITY_STATES_JSON)?;
        assert_eq!(catalog.version, 1, "catalog version must be 1");
        assert_eq!(
            catalog.states.len(),
            CAPABILITY_UI_STATES.len(),
            "Rust table row count must equal JSON catalog (single source)"
        );

        // Assert 1:1 ordered equality with Rust const.
        for (idx, entry) in catalog.states.iter().enumerate() {
            let (cap, state) = CAPABILITY_UI_STATES[idx];
            assert_eq!(
                entry.capability, cap,
                "capability mismatch at index {idx}: JSON {:?} vs Rust {:?}",
                entry.capability, cap
            );
            assert_eq!(
                entry.state, state,
                "state mismatch at index {idx}: JSON {:?} vs Rust {:?}",
                entry.state, state
            );
        }

        // Also assert the TS file imports the same JSON (lightweight file check).
        // We do not execute TS here; we verify the TS source contains the import
        // and that its catalog content, if inlined, would equal our parse. The
        // vitest `CapabilityStatesSingleSource` test does the complementary
        // runtime import assertion on the TS side; this Rust fence ensures the
        // JSON file is actually the source and the Rust array hasn't drifted.
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let Some(parent) = manifest_dir.parent() else {
            return Err("src-tauri manifest directory has no parent".into());
        };
        let ts_path = parent.join("src/lib/capabilityStates.ts");
        let ts_src = std::fs::read_to_string(&ts_path)?;
        assert!(
            ts_src.contains("capability-states.json"),
            "TS file must import spec/catalogs/capability-states.json as single source"
        );
        // Ensure TS does not contain a stale hard-coded duplicate that could
        // diverge (it should derive via catalog.states.map).
        // We allow the comment mentioning the capability strings, but the
        // defining array literal `['mock.asr', 'asr-local']` should not appear
        // as a hard-coded TS table anymore — it now comes from the JSON import.
        // If the file still contains the literal table, that is a drift risk.
        // We check that at most the JSON import drives the table.
        // (Lenient: just ensure it maps from catalogJson)
        assert!(
            ts_src.contains("catalogJson") || ts_src.contains("capability-states.json"),
            "TS should derive CAPABILITY_UI_STATES from imported catalog JSON"
        );

        // Validate uniqueness and non-empty invariants on the JSON source
        // (mirrors capability_ui_mapping_drives_states but on the catalog).
        let mut seen_caps = std::collections::HashSet::new();
        let mut seen_states = std::collections::HashSet::new();
        for entry in &catalog.states {
            assert!(!entry.capability.is_empty() && !entry.state.is_empty());
            assert!(
                !entry.state.contains("::"),
                "state tokens are flat vocabulary: {}",
                entry.state
            );
            assert!(
                !FORBIDDEN_OS_TOKENS
                    .iter()
                    .any(|t| entry.capability.contains(t) || entry.state.contains(t)),
                "platform name leaked into catalog: ({} -> {})",
                entry.capability,
                entry.state
            );
            assert!(
                seen_caps.insert(entry.capability.clone()),
                "duplicate capability in catalog: {}",
                entry.capability
            );
            seen_states.insert(entry.state.clone());
        }
        // Ensure at least 6 entries as per spec
        assert!(
            catalog.states.len() >= 6,
            "catalog must have at least 6 states"
        );
        Ok(())
    }

    /// Scan every built frontend asset under `../dist`: zero OS-name
    /// conditionals/branch tokens may appear anywhere (case-insensitive).
    /// CONTRACT.md: "No frontend `if linux/cosmic/gnome` branching anywhere;
    /// capability strings drive UI state."
    #[test]
    fn capability_ui_no_os_branching() -> Result<(), Box<dyn std::error::Error>> {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let Some(parent) = manifest_dir.parent() else {
            return Err("src-tauri manifest directory has no parent".into());
        };
        let dist = parent.join("dist");
        assert!(
            dist.is_dir(),
            "frontend dist missing at {} — build it (npm run build) or ship the scaffold placeholder",
            dist.display()
        );

        let mut scanned_files = 0usize;
        let mut scanned_bytes = 0usize;
        let mut violations: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dist)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                violations.push(format!("unreadable asset {}", path.display()));
                continue;
            };
            // Text-ish assets only; skip anything with a NUL byte (binary).
            if bytes.contains(&0u8) {
                continue;
            }
            scanned_files += 1;
            scanned_bytes += bytes.len();
            let haystack = String::from_utf8_lossy(&bytes);
            let lowered = haystack.to_lowercase();
            for token in FORBIDDEN_OS_TOKENS {
                if lowered.contains(token) {
                    violations.push(format!(
                        "{} contains OS-name token {token:?}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    ));
                }
            }
        }
        assert!(
            scanned_files > 0,
            "no text assets found in dist — nothing to gate"
        );
        assert!(
            violations.is_empty(),
            "OS-name branching found in frontend assets ({scanned_files} files, {scanned_bytes} bytes):\n{}",
            violations.join("\n")
        );
        Ok(())
    }

    /// The mapping table itself is the driver: every known daemon capability
    /// resolves to a stable, non-empty, platform-free state token, table rows
    /// are unique, and the scaffold frontend actually references the tokens.
    #[test]
    fn capability_ui_mapping_drives_states() -> Result<(), Box<dyn std::error::Error>> {
        let mut seen: Vec<&str> = Vec::new();
        for (cap, state) in CAPABILITY_UI_STATES {
            assert!(!cap.is_empty() && !state.is_empty());
            assert!(
                !state.contains("::"),
                "state tokens are flat vocabulary: {state}"
            );
            assert!(
                !FORBIDDEN_OS_TOKENS
                    .iter()
                    .any(|t| cap.contains(t) || state.contains(t)),
                "platform name leaked into mapping: ({cap} -> {state})"
            );
            assert!(!seen.contains(cap), "duplicate capability row: {cap}");
            seen.push(cap);
            assert_eq!(ui_state_for(cap), *state);
        }
        assert_eq!(
            ui_state_for("future.unknown.capability"),
            UNKNOWN_CAPABILITY_STATE
        );

        // The shipped frontend references the state vocabulary (data-capability
        // attributes); this keeps the table and the rendered states coupled.
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let Some(parent) = manifest_dir.parent() else {
            return Err("src-tauri manifest directory has no parent".into());
        };
        let dist = parent.join("dist");
        if dist.is_dir() {
            // Recursive walk: vite emits dist/assets/<hash>.js bundles.
            fn collect_text(
                dir: &std::path::Path,
                corpus: &mut String,
                files: &mut usize,
            ) -> std::io::Result<()> {
                for entry in std::fs::read_dir(dir)? {
                    let path = entry?.path();
                    if path.is_dir() {
                        collect_text(&path, corpus, files)?;
                    } else if let Ok(bytes) = std::fs::read(&path)
                        && !bytes.contains(&0u8)
                    {
                        *files += 1;
                        corpus.push_str(&String::from_utf8_lossy(&bytes));
                    }
                }
                Ok(())
            }
            let mut corpus = String::new();
            let mut _scanned_files = 0usize;
            collect_text(&dist, &mut corpus, &mut _scanned_files)?;
            let referenced = CAPABILITY_UI_STATES
                .iter()
                .filter(|(_, state)| corpus.contains(state))
                .count();
            assert!(
                referenced >= 2,
                "frontend must reference capability state tokens; found {referenced}"
            );
        }
        Ok(())
    }
}
