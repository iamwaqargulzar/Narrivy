//! The files the assistant changed in a chat, so the user can put them all
//! back (D55's coding harness). Before `write_file`, `append_file` or
//! `edit_file` first touches a file in a chat, the file is copied into
//! `assistant/changes/<chat>/` (or noted as new). **Undo all changes**
//! restores the copies and removes the files it made; **Keep** forgets
//! them. What commands change is not tracked (the app says so).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Files above this size are not copied (and the change cannot be undone).
const MAX_COPY: u64 = 20 * 1024 * 1024;
/// The most files one chat tracks.
const MAX_FILES: usize = 500;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Entry {
    path: String,
    /// The copy's name in the chat's folder; `None` when the file was new.
    backup: Option<String>,
    /// Too large to copy: listed, not restorable.
    #[serde(default)]
    too_large: bool,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn root() -> Option<PathBuf> {
    super::host::state_dir()
        .ok()
        .map(|dir| dir.join("assistant").join("changes"))
}

fn dir_of(root: &Path, chat: &str) -> Option<PathBuf> {
    valid_id(chat).then(|| root.join(chat))
}

fn read_index(dir: &Path) -> Vec<Entry> {
    std::fs::read(dir.join("index.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_index(dir: &Path, entries: &[Entry]) -> bool {
    serde_json::to_vec(entries)
        .ok()
        .is_some_and(|bytes| std::fs::write(dir.join("index.json"), bytes).is_ok())
}

/// Note `path` before the assistant changes it in `chat` (the first time
/// only). Returns false when the change could not be made undoable.
pub fn before_change(chat: &str, path: &Path) -> bool {
    root().is_some_and(|root| before_change_in(&root, chat, path))
}

fn before_change_in(root: &Path, chat: &str, path: &Path) -> bool {
    let Some(dir) = dir_of(root, chat) else {
        return false;
    };
    if super::host::fs::dir_builder(0o700).create(&dir).is_err() {
        return false;
    }
    let key = path.display().to_string();
    let mut entries = read_index(&dir);
    if entries.iter().any(|e| e.path == key) {
        return true;
    }
    if entries.len() >= MAX_FILES {
        return false;
    }
    let mut entry = Entry {
        path: key,
        ..Entry::default()
    };
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_COPY || !meta.is_file() {
            entry.too_large = true;
        } else {
            let name = format!("b{}", entries.len());
            if std::fs::copy(path, dir.join(&name)).is_err() {
                return false;
            }
            entry.backup = Some(name);
        }
    }
    entries.push(entry);
    write_index(&dir, &entries)
}

/// What undoing did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Undone {
    pub restored: usize,
    pub removed: usize,
    pub failed: usize,
}

fn undo_in(root: &Path, chat: &str) -> Result<Undone, &'static str> {
    let dir = dir_of(root, chat).ok_or("changes_bad_id")?;
    let mut done = Undone::default();
    for entry in read_index(&dir) {
        let path = PathBuf::from(&entry.path);
        match (&entry.backup, entry.too_large) {
            (_, true) => done.failed += 1,
            (Some(backup), _) => {
                if std::fs::copy(dir.join(backup), &path).is_ok() {
                    done.restored += 1;
                } else {
                    done.failed += 1;
                }
            }
            (None, _) => {
                if !path.exists() || std::fs::remove_file(&path).is_ok() {
                    done.removed += 1;
                } else {
                    done.failed += 1;
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(done)
}

fn forget_in(root: &Path, chat: &str) {
    if let Some(dir) = dir_of(root, chat) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Forget a deleted chat's copies (its files stay as they are).
pub fn delete_chat(chat: &str) {
    if let Some(root) = root() {
        forget_in(&root, chat);
    }
}

/// An `AssistantLibrary` request (`kind` `changes`, id: the chat): `list`
/// (the changed files), `undo` (put them all back), `keep` (forget them).
///
/// # Errors
/// A short code.
pub fn handle(op: &str, chat: Option<&str>) -> Result<String, &'static str> {
    let root = root().ok_or("changes_unavailable")?;
    handle_in(&root, op, chat)
}

fn handle_in(root: &Path, op: &str, chat: Option<&str>) -> Result<String, &'static str> {
    let chat = chat.ok_or("changes_bad_id")?;
    let dir = dir_of(root, chat).ok_or("changes_bad_id")?;
    match op {
        "list" => {
            let files: Vec<serde_json::Value> = read_index(&dir)
                .into_iter()
                .map(|e| {
                    serde_json::json!({"path": e.path, "new": e.backup.is_none() && !e.too_large,
                        "undoable": !e.too_large})
                })
                .collect();
            Ok(serde_json::json!({ "files": files }).to_string())
        }
        "undo" => {
            let done = undo_in(root, chat)?;
            Ok(
                serde_json::json!({"restored": done.restored, "removed": done.removed,
                "failed": done.failed})
                .to_string(),
            )
        }
        "keep" => {
            forget_in(root, chat);
            Ok("{}".to_owned())
        }
        _ => Err("assistant_library_bad_request"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_puts_changed_files_back_and_removes_new_ones() -> std::io::Result<()> {
        let state = tempfile::tempdir()?;
        let work = tempfile::tempdir()?;
        let (root, chat) = (state.path(), "chat-code");
        let old = work.path().join("main.rs");
        let new = work.path().join("new.rs");
        std::fs::write(&old, "fn main() {}\n")?;
        assert!(before_change_in(root, chat, &old));
        std::fs::write(&old, "broken")?;
        // A second change keeps the first copy.
        assert!(before_change_in(root, chat, &old));
        std::fs::write(&old, "more broken")?;
        assert!(before_change_in(root, chat, &new));
        std::fs::write(&new, "x")?;
        let list: serde_json::Value = serde_json::from_str(
            &handle_in(root, "list", Some(chat)).map_err(std::io::Error::other)?,
        )?;
        assert_eq!(list["files"].as_array().map(Vec::len), Some(2));
        let undone = undo_in(root, chat).map_err(std::io::Error::other)?;
        assert_eq!(
            undone,
            Undone {
                restored: 1,
                removed: 1,
                failed: 0
            }
        );
        assert_eq!(std::fs::read_to_string(&old)?, "fn main() {}\n");
        assert!(!new.exists());
        // Nothing left to undo.
        let list: serde_json::Value = serde_json::from_str(
            &handle_in(root, "list", Some(chat)).map_err(std::io::Error::other)?,
        )?;
        assert_eq!(list["files"].as_array().map(Vec::len), Some(0));
        Ok(())
    }

    #[test]
    fn keep_forgets_and_bad_ids_are_refused() -> std::io::Result<()> {
        let state = tempfile::tempdir()?;
        let work = tempfile::tempdir()?;
        let file = work.path().join("a.txt");
        std::fs::write(&file, "a")?;
        assert!(before_change_in(state.path(), "c1", &file));
        handle_in(state.path(), "keep", Some("c1")).map_err(std::io::Error::other)?;
        assert_eq!(undo_in(state.path(), "c1"), Ok(Undone::default()));
        assert!(!before_change_in(state.path(), "../x", &file));
        assert_eq!(
            handle_in(state.path(), "undo", Some("a/b")),
            Err("changes_bad_id")
        );
        Ok(())
    }
}
