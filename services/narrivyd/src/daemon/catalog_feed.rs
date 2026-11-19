//! The live model list: the catalog files the Store repository publishes
//! (`narrivy/store`, `catalog/<file>` on its `main` branch), so a model
//! published there shows up without a new Narrivy release.
//!
//! - At start the last good copy (`<state>/catalog/<file>`) is used, when
//!   there is one; else the list compiled into this build.
//! - When the user opens Models or the Store (`ListModels`, `ListStore`,
//!   `ListLanguageModels`) and the copy is older than [`REFRESH_AFTER`], the
//!   files are fetched again in the background; the next listing shows them.
//!   Nothing is fetched while nobody looks at a model list.
//! - Each file must pass the same checks as the built-in one
//!   (`narrivy_model_manager::check_live`) before it is used or kept. Every
//!   entry still names its own pinned revision and digest, and a download is
//!   verified against those, so the list can add models but cannot change
//!   the bytes of one.
//!
//! Only content-free state is reported (`catalog.live=…`).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use narrivy_core::CancelFlag;
use narrivy_model_manager::{LIVE_FILES, MAX_LIVE_FILE_BYTES, check_live, install_live};

/// Where the Store repository publishes its catalog.
const FEED_BASE: &str = "https://huggingface.co/narrivy/store/resolve/main/catalog";
/// How old the copy in use may get before a model list fetches it again.
pub const REFRESH_AFTER: Duration = Duration::from_secs(60 * 60);
/// How long a failed fetch waits before the next try.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    BuiltIn,
    Cached,
    Fresh,
}

struct Feed {
    source: Source,
    last_try: Option<Instant>,
    last_ok: Option<Instant>,
    failure: Option<&'static str>,
}

static FEED: Mutex<Feed> = Mutex::new(Feed {
    source: Source::BuiltIn,
    last_try: None,
    last_ok: None,
    failure: None,
});
static RUNNING: AtomicBool = AtomicBool::new(false);

fn cache_dir() -> Option<PathBuf> {
    super::host::state_dir().ok().map(|dir| dir.join("catalog"))
}

/// Use the last good copy kept on disk, if any. Called once at start.
pub fn load_cached() {
    let Some(dir) = cache_dir() else {
        return;
    };
    if read_dir_into_live(&dir) > 0
        && let Ok(mut feed) = FEED.lock()
    {
        feed.source = Source::Cached;
    }
}

fn read_dir_into_live(dir: &Path) -> usize {
    let files: Vec<(&str, String)> = LIVE_FILES
        .iter()
        .filter_map(|name| {
            let path = dir.join(name);
            let meta = std::fs::symlink_metadata(&path).ok()?;
            if !meta.is_file() || meta.len() > MAX_LIVE_FILE_BYTES as u64 {
                return None;
            }
            std::fs::read_to_string(&path)
                .ok()
                .map(|text| (*name, text))
        })
        .collect();
    install_live(&files)
}

/// Fetch the live list in the background when the copy in use is stale.
/// Returns at once.
pub fn refresh_if_stale() {
    let due = FEED.lock().is_ok_and(|feed| {
        let fresh = feed.last_ok.is_some_and(|at| at.elapsed() < REFRESH_AFTER);
        let retrying =
            feed.failure.is_some() && feed.last_try.is_some_and(|at| at.elapsed() < RETRY_AFTER);
        !fresh && !retrying
    });
    if !due || RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("narrivy-catalog-feed".into())
        .spawn(|| {
            let outcome = refresh();
            if let Ok(mut feed) = FEED.lock() {
                feed.last_try = Some(Instant::now());
                match outcome {
                    Ok(()) => {
                        feed.source = Source::Fresh;
                        feed.last_ok = Some(Instant::now());
                        feed.failure = None;
                    }
                    Err(code) => feed.failure = Some(code),
                }
            }
            RUNNING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Fetch every file, keep the ones that pass their checks, use them.
fn refresh() -> Result<(), &'static str> {
    let dir = cache_dir().ok_or("state_unavailable")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .create(&dir)
        .map_err(|_| "disk_write_failed")?;
    super::host::fs::set_mode(&dir, 0o700).map_err(|_| "disk_write_failed")?;
    let cancel = CancelFlag::new();
    let mut fetched = Vec::new();
    let mut failure = None;
    for name in LIVE_FILES {
        let part = dir.join(format!("{name}.part"));
        let result = super::download::fetch_small(
            &format!("{FEED_BASE}/{name}"),
            &part,
            MAX_LIVE_FILE_BYTES as u64,
            &cancel,
        )
        .and_then(|()| std::fs::read_to_string(&part).map_err(|_| "unreadable"))
        .and_then(|text| check_live(name, &text).map(|_| text));
        match result {
            Ok(text) => {
                let kept = super::host::fs::set_mode(&part, 0o600)
                    .and_then(|()| std::fs::rename(&part, dir.join(name)));
                if kept.is_err() {
                    let _ = std::fs::remove_file(&part);
                }
                fetched.push((name, text));
            }
            Err(code) => {
                let _ = std::fs::remove_file(&part);
                failure.get_or_insert(code);
            }
        }
    }
    if fetched.is_empty() {
        return Err(failure.unwrap_or("download_failed"));
    }
    install_live(&fetched);
    Ok(())
}

/// `catalog.live=<built_in|cached|fresh>` with the last failure, if any.
pub fn capability() -> String {
    let Ok(feed) = FEED.lock() else {
        return "catalog.live=unavailable".to_owned();
    };
    let source = match feed.source {
        Source::BuiltIn => "built_in",
        Source::Cached => "cached",
        Source::Fresh => "fresh",
    };
    match feed.failure {
        Some(code) => format!("catalog.live={source}:failed:{code}"),
        None => format!("catalog.live={source}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cached_copy_that_fails_its_checks_is_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("published-packs.json"), "not json")?;
        std::fs::write(dir.path().join("unrelated.json"), "{}")?;
        assert_eq!(read_dir_into_live(dir.path()), 0);
        Ok(())
    }

    #[test]
    fn the_feed_reads_only_the_store_repository() {
        assert!(narrivy_model_manager::download::url_allowed(FEED_BASE));
        assert!(FEED_BASE.starts_with("https://huggingface.co/narrivy/store/"));
    }
}
