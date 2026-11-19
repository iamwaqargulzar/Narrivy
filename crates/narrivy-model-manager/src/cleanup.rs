//! Cache cleanup (PERF-008): remove staging leftovers and unreferenced,
//! non-pinned variants while always preserving the protected set — the
//! source-of-selected entry, the CPU-fallback entry, and user-pinned entries.
//!
//! Idempotent and journal-less: every run re-derives truth from the directory
//! tree, so a restart reconciles without replay.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;
use crate::registry::RegistryEntry;
use crate::store::{validate_cache_root, validate_cache_tree};

/// What a cleanup pass did (or would do, in dry-run mode).
#[derive(Debug, Default, Clone, Serialize)]
pub struct CleanupReport {
    pub dry_run: bool,
    /// Top-level paths removed (or that would be removed).
    pub removed: Vec<PathBuf>,
    /// Protected entry ids that currently exist on disk.
    pub kept_protected: Vec<String>,
    pub removed_bytes: u64,
}

/// Remove everything under `<cache_root>/staging` plus installed variant dirs
/// whose id is neither in `keep_ids` nor registry-pinned.
///
/// `keep_ids` must include the selected source model and the CPU fallback;
/// pinned entries are additionally protected from the registry itself.
pub fn remove_unreferenced(
    entries: &[RegistryEntry],
    cache_root: &Path,
    keep_ids: &[&str],
    dry_run: bool,
) -> Result<CleanupReport> {
    validate_cache_root(cache_root)?;
    for entry in entries {
        entry.validate()?;
    }
    validate_cache_tree(cache_root, &cache_root.join("staging"))?;
    validate_cache_tree(cache_root, &cache_root.join("models"))?;

    let mut report = CleanupReport {
        dry_run,
        ..CleanupReport::default()
    };

    let mut protected: Vec<&str> = keep_ids.to_vec();
    for entry in entries {
        if entry.pinned && !protected.contains(&entry.id.as_str()) {
            protected.push(&entry.id);
        }
    }

    // 1. Staging leftovers always go.
    let staging = cache_root.join("staging");
    for child in children_of(&staging) {
        report.removed_bytes += tree_bytes(&child);
        if !dry_run {
            remove_path(&child)?;
        }
        report.removed.push(child);
    }

    // 2. Installed variants not in the protected set go, family by family.
    let models = cache_root.join("models");
    let families = children_of(&models);
    for family_dir in families {
        for variant_dir in children_of(&family_dir) {
            let Some(id) = variant_dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if protected.contains(&id) {
                if !report.kept_protected.iter().any(|k| k == id) {
                    report.kept_protected.push(id.to_string());
                }
                continue;
            }
            report.removed_bytes += tree_bytes(&variant_dir);
            if !dry_run {
                remove_path(&variant_dir)?;
            }
            report.removed.push(variant_dir);
        }
        // Drop family dirs left empty so the tree stays tidy.
        if !dry_run && is_empty_dir(&family_dir) {
            remove_path(&family_dir)?;
        }
    }

    Ok(report)
}

fn children_of(dir: &Path) -> Vec<PathBuf> {
    let Ok(readings) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = readings.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    out.sort();
    out
}

fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut it| it.next().is_none())
}

fn remove_path(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(crate::error::ModelManagerError::UnsafeCacheRoot);
    }
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else if metadata.is_file() {
        std::fs::remove_file(path)?;
    } else {
        return Err(crate::error::ModelManagerError::UnsafeCacheRoot);
    }
    Ok(())
}

fn tree_bytes(path: &Path) -> u64 {
    if path.is_file() {
        return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    children_of(path).iter().map(|c| tree_bytes(c)).sum()
}
