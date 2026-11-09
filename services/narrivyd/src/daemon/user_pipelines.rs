//! The user's pipelines: the list the Pipelines page shows, in its order.
//!
//! Each pipeline has an id (what shortcuts, `--pipeline` and its own model and
//! Voice settings are keyed by), a name the user can change, and a Refine
//! mode: `clean` (the full corrector) or `raw` (whitespace only). A fresh
//! install has one, "Pipeline 1", with the id `clean` so that shortcuts and
//! settings made before pipelines could be created keep working. The user can
//! create more and delete any but the last.
//!
//! The two built-in mode ids, `clean` and `raw`, stay selectable by id even
//! when no listed pipeline has them, so a shortcut or script made for them
//! still dictates in that mode with the default settings.
//!
//! Kept in `pipelines.json` in the state directory.

#![forbid(unsafe_code)]

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::policy::{CLEAN_PIPELINE_ID, CorrectionMode, RAW_PIPELINE_ID};

/// A pipeline name is at most this many characters.
pub const MAX_NAME_CHARS: usize = 40;
/// At most this many pipelines.
pub const MAX_PIPELINES: usize = 20;

/// Serializes read-modify-write of the file across IPC handler threads.
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refine {
    Clean,
    Raw,
}

impl Refine {
    #[must_use]
    pub const fn mode(self) -> CorrectionMode {
        match self {
            Self::Clean => CorrectionMode::Clean,
            Self::Raw => CorrectionMode::Raw,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Raw => "raw",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "clean" => Some(Self::Clean),
            "raw" => Some(Self::Raw),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPipeline {
    pub id: String,
    pub name: String,
    pub refine: Refine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Store {
    pipelines: Vec<UserPipeline>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            pipelines: vec![UserPipeline {
                id: CLEAN_PIPELINE_ID.to_owned(),
                name: "Pipeline 1".to_owned(),
                refine: Refine::Clean,
            }],
        }
    }
}

/// The user's pipelines, in order; never empty.
#[must_use]
pub fn list() -> Vec<UserPipeline> {
    read().pipelines
}

/// The mode a dictation with `id` runs in: a listed pipeline's Refine mode,
/// else the built-in mode with that id, else `None` (not selectable).
#[must_use]
pub fn mode_of(id: &str) -> Option<CorrectionMode> {
    list()
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| p.refine.mode())
        .or_else(|| CorrectionMode::from_pipeline_id(id))
}

/// The pipeline a start without an id uses: the first one listed.
#[must_use]
pub fn default_id() -> String {
    list()
        .into_iter()
        .next()
        .map_or_else(|| CLEAN_PIPELINE_ID.to_owned(), |p| p.id)
}

/// The pipeline dictating now, or the last one that did.
static ACTIVE: Mutex<Option<String>> = Mutex::new(None);

/// The pipeline whose options a change without a pipeline applies to (the
/// CLI without `--pipeline`, the tray's Language menu): the one dictating
/// now or last, else [`default_id`].
#[must_use]
pub fn active() -> String {
    ACTIVE
        .lock()
        .ok()
        .and_then(|active| active.clone())
        .filter(|id| is_builtin_mode_id(id) || list().iter().any(|p| &p.id == id))
        .unwrap_or_else(default_id)
}

/// A dictation with `id` started: its options apply from now on.
pub fn set_active(id: &str) {
    if let Ok(mut active) = ACTIVE.lock() {
        id.clone_into(active.get_or_insert_with(String::new));
    }
}

/// Add a pipeline named "Pipeline N" (the lowest N no pipeline uses) at the
/// end, refining like the first one.
///
/// # Errors
/// `pipelines_full` at [`MAX_PIPELINES`], or `pipelines_unavailable` when it
/// cannot be saved.
pub fn create() -> Result<UserPipeline, &'static str> {
    let _guard = LOCK.lock().map_err(|_| "pipelines_unavailable")?;
    let mut store = read();
    if store.pipelines.len() >= MAX_PIPELINES {
        return Err("pipelines_full");
    }
    let taken = |n: usize| {
        store
            .pipelines
            .iter()
            .any(|p| p.id == format!("pipeline-{n}") || p.name == format!("Pipeline {n}"))
    };
    // Pipeline 1 is the first install's; a new one starts at 2.
    let n = (2..).find(|&n| !taken(n)).unwrap_or(2);
    let refine = store.pipelines.first().map_or(Refine::Clean, |p| p.refine);
    let created = UserPipeline {
        id: format!("pipeline-{n}"),
        name: format!("Pipeline {n}"),
        refine,
    };
    store.pipelines.push(created.clone());
    write(&store)?;
    Ok(created)
}

/// Remove a pipeline. The last one cannot be removed.
///
/// # Errors
/// `pipeline_unknown`, `pipeline_last`, or `pipelines_unavailable`.
pub fn delete(id: &str) -> Result<(), &'static str> {
    let _guard = LOCK.lock().map_err(|_| "pipelines_unavailable")?;
    let mut store = read();
    let Some(index) = store.pipelines.iter().position(|p| p.id == id) else {
        return Err("pipeline_unknown");
    };
    if store.pipelines.len() == 1 {
        return Err("pipeline_last");
    }
    store.pipelines.remove(index);
    write(&store)
}

/// Rename a pipeline; the name is trimmed and must not be empty.
///
/// # Errors
/// `pipeline_unknown`, `pipeline_name_invalid`, or `pipelines_unavailable`.
pub fn rename(id: &str, name: &str) -> Result<(), &'static str> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err("pipeline_name_invalid");
    }
    update(id, |p| p.name = name.to_owned())
}

/// Set how a pipeline refines its text.
///
/// # Errors
/// `pipeline_unknown`, or `pipelines_unavailable`.
pub fn set_refine(id: &str, refine: Refine) -> Result<(), &'static str> {
    update(id, |p| p.refine = refine)
}

/// Put the pipelines in this order. Every listed id must appear exactly once.
///
/// # Errors
/// `pipeline_order_invalid`, or `pipelines_unavailable`.
pub fn reorder(ids: &[String]) -> Result<(), &'static str> {
    let _guard = LOCK.lock().map_err(|_| "pipelines_unavailable")?;
    let store = read();
    if ids.len() != store.pipelines.len() {
        return Err("pipeline_order_invalid");
    }
    let mut ordered = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(p) = store.pipelines.iter().find(|p| &p.id == id) else {
            return Err("pipeline_order_invalid");
        };
        if ordered.iter().any(|o: &UserPipeline| o.id == p.id) {
            return Err("pipeline_order_invalid");
        }
        ordered.push(p.clone());
    }
    write(&Store { pipelines: ordered })
}

fn update(id: &str, change: impl FnOnce(&mut UserPipeline)) -> Result<(), &'static str> {
    let _guard = LOCK.lock().map_err(|_| "pipelines_unavailable")?;
    let mut store = read();
    let Some(pipeline) = store.pipelines.iter_mut().find(|p| p.id == id) else {
        return Err("pipeline_unknown");
    };
    change(pipeline);
    write(&store)
}

/// The stored list, or the default when there is none or it cannot be read.
/// A list that is empty or has a repeated id is not trusted either.
fn read() -> Store {
    let store = path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Store>(&bytes).ok());
    match store {
        Some(store) if valid(&store) => store,
        _ => Store::default(),
    }
}

fn valid(store: &Store) -> bool {
    !store.pipelines.is_empty()
        && store.pipelines.len() <= MAX_PIPELINES
        && store
            .pipelines
            .iter()
            .enumerate()
            .all(|(i, p)| valid_id(&p.id) && store.pipelines[..i].iter().all(|q| q.id != p.id))
}

/// The same id rule the Voice settings use, so every pipeline's settings can
/// be stored.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn write(store: &Store) -> Result<(), &'static str> {
    let path = path().ok_or("pipelines_unavailable")?;
    let bytes = serde_json::to_vec_pretty(store).map_err(|_| "pipelines_unavailable")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|_| "pipelines_unavailable")?;
    std::fs::rename(&tmp, &path).map_err(|_| "pipelines_unavailable")
}

#[cfg(not(test))]
fn path() -> Option<std::path::PathBuf> {
    super::host::state_dir()
        .ok()
        .map(|dir| dir.join("pipelines.json"))
}

/// Tests never read or write the user's own pipelines.
#[cfg(test)]
fn path() -> Option<std::path::PathBuf> {
    TEST_PATH.with(|p| p.borrow().clone())
}

#[cfg(test)]
thread_local! {
    static TEST_PATH: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Give this test thread its own file; without one every test sees the
/// default list.
#[cfg(test)]
pub fn use_test_file(path: std::path::PathBuf) {
    TEST_PATH.with(|p| *p.borrow_mut() = Some(path));
}

/// Whether `id` is one of the two built-in mode ids.
#[must_use]
pub fn is_builtin_mode_id(id: &str) -> bool {
    id == CLEAN_PIPELINE_ID || id == RAW_PIPELINE_ID
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn fresh() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        use_test_file(dir.path().join("pipelines.json"));
        dir
    }

    #[test]
    fn a_fresh_install_has_one_pipeline_called_pipeline_1() {
        let _dir = fresh();
        let list = list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "clean");
        assert_eq!(list[0].name, "Pipeline 1");
        assert_eq!(list[0].refine, Refine::Clean);
        assert_eq!(default_id(), "clean");
    }

    #[test]
    fn create_numbers_new_pipelines_and_delete_keeps_the_last_one() {
        let _dir = fresh();
        let two = create().unwrap();
        assert_eq!(
            (two.id.as_str(), two.name.as_str()),
            ("pipeline-2", "Pipeline 2")
        );
        let three = create().unwrap();
        assert_eq!(three.name, "Pipeline 3");
        delete("pipeline-2").unwrap();
        // The lowest free number is used again.
        assert_eq!(create().unwrap().name, "Pipeline 2");
        delete("clean").unwrap();
        delete("pipeline-2").unwrap();
        assert_eq!(delete("pipeline-3"), Err("pipeline_last"));
        assert_eq!(delete("nope"), Err("pipeline_unknown"));
        assert_eq!(default_id(), "pipeline-3");
    }

    #[test]
    fn refine_rename_and_order_are_kept() {
        let _dir = fresh();
        let two = create().unwrap();
        set_refine(&two.id, Refine::Raw).unwrap();
        assert_eq!(mode_of(&two.id), Some(CorrectionMode::Raw));
        rename(&two.id, "  Emails  ").unwrap();
        assert_eq!(rename(&two.id, "   "), Err("pipeline_name_invalid"));
        reorder(&[two.id.clone(), "clean".to_owned()]).unwrap();
        let list = list();
        assert_eq!(list[0].name, "Emails");
        assert_eq!(default_id(), two.id);
        assert_eq!(
            reorder(&["clean".to_owned()]),
            Err("pipeline_order_invalid")
        );
        assert_eq!(
            reorder(&["clean".to_owned(), "clean".to_owned()]),
            Err("pipeline_order_invalid")
        );
    }

    #[test]
    fn built_in_mode_ids_stay_selectable_after_their_pipeline_is_gone() {
        let _dir = fresh();
        create().unwrap();
        delete("clean").unwrap();
        assert_eq!(mode_of("clean"), Some(CorrectionMode::Clean));
        assert_eq!(mode_of("raw"), Some(CorrectionMode::Raw));
        assert_eq!(mode_of("unknown"), None);
    }

    #[test]
    fn a_damaged_file_falls_back_to_the_default() {
        let dir = fresh();
        std::fs::write(dir.path().join("pipelines.json"), br#"{"pipelines":[]}"#).unwrap();
        assert_eq!(list().len(), 1);
        std::fs::write(dir.path().join("pipelines.json"), b"not json").unwrap();
        assert_eq!(list()[0].name, "Pipeline 1");
    }
}
