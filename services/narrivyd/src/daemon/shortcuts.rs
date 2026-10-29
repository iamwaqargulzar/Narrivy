//! Keyboard shortcuts Narrivy manages in the desktop's own shortcut settings
//! (owner decision D24, `docs/roadmap/design/shortcut-capture.md`).
//!
//! The platform adapter knows how one desktop stores shortcuts. This module
//! owns the product rules around it:
//! - a shortcut runs exactly `CLI toggle --pipeline ID [--language TAG]
//!   [--output-language TAG]`, built only from validated parts;
//! - an entry is Narrivy's only if its command parses back into that shape,
//!   and only Narrivy's entries are listed, updated or removed;
//! - a combination in use is reported, never replaced without the caller
//!   asking; a replaced custom entry is kept and put back when the Narrivy
//!   shortcut is removed.
//!
//! All work is bounded local file I/O on small files and runs on the request
//! path; a mutex serialises read-modify-write cycles within this daemon.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use narrivy_ipc::{
    MAX_SHORTCUT_KEY_BYTES, MAX_SHORTCUT_MODIFIERS, RecognitionTagDto, ShortcutBindingDto,
    ShortcutSetDto, ShortcutsDto,
};
use narrivy_platform_linux::desktop_shortcuts::{
    Action, AddOutcome, Conflict, DesktopShortcuts, KeyCombo, Modifier, ShortcutError,
};

use super::policy::CorrectionMode;

/// Rejection detail when the daemon was composed without shortcut management.
pub const SHORTCUTS_NOT_COMPOSED_DETAIL: &str = "desktop shortcuts are not composed";
/// Installed location of the CLI, used when no sibling binary is present.
const INSTALLED_CLI: &str = "/usr/bin/narrivy-cli";
const CLI_NAME: &str = "narrivy-cli";
/// Custom entries replaced on the user's request, kept so they can be put back.
const REPLACED_FILE: &str = "replaced-shortcuts.json";
/// Upper bound on the name reported for whatever holds a combination.
const MAX_CONFLICT_NAME_BYTES: usize = 48;

/// A binding for this name, instead of a pipeline, cancels the dictation
/// in progress (`CLI cancel`).
pub const CANCEL_TARGET: &str = "cancel";

pub struct ShortcutService {
    desktop: Result<DesktopShortcuts, &'static str>,
    cli: String,
    replaced_path: PathBuf,
    lock: Mutex<()>,
}

impl ShortcutService {
    /// Compose from the running session: detect the desktop's shortcut store
    /// and resolve the CLI that sits beside this daemon.
    pub fn compose(state_dir: &Path) -> Self {
        let desktop = DesktopShortcuts::detect().map_err(|unmanaged| unmanaged.reason());
        Self::new(desktop, resolve_cli(), state_dir)
    }

    pub fn new(
        desktop: Result<DesktopShortcuts, &'static str>,
        cli: String,
        state_dir: &Path,
    ) -> Self {
        Self {
            desktop,
            cli,
            replaced_path: state_dir.join(REPLACED_FILE),
            lock: Mutex::new(()),
        }
    }

    /// `managed`, or `manual:<reason>` when the user has to bind the command
    /// in the desktop's settings themselves.
    pub fn support(&self) -> String {
        match &self.desktop {
            Ok(_) => "managed".to_owned(),
            Err(reason) => format!("manual:{reason}"),
        }
    }

    /// Content-free capability token for Status.
    pub fn capability(&self) -> String {
        format!("activation.shortcuts={}", self.support())
    }

    fn manual_command(&self) -> String {
        format!("{} toggle", self.cli)
    }

    /// The shortcuts Narrivy created. A store that cannot be read lists none;
    /// `support` then tells the client to fall back to the manual route.
    pub fn list(&self) -> ShortcutsDto {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (support, bindings) = match &self.desktop {
            Ok(desktop) => match desktop.list() {
                Ok(entries) => (
                    self.support(),
                    entries
                        .iter()
                        .filter_map(|entry| {
                            let combo = entry.combo.as_ref()?;
                            let Action::Spawn(command) = &entry.action else {
                                return None;
                            };
                            let target = self.parse_command(command)?;
                            Some(ShortcutBindingDto {
                                modifiers: combo
                                    .modifiers()
                                    .iter()
                                    .map(|m| m.as_str().to_owned())
                                    .collect(),
                                key: combo.key().to_owned(),
                                pipeline_id: target.pipeline_id,
                                spoken_language: target.spoken_language,
                                output_language: target.output_language,
                            })
                        })
                        .collect(),
                ),
                Err(err) => (format!("manual:{}", store_reason(&err)), Vec::new()),
            },
            Err(_) => (self.support(), Vec::new()),
        };
        ShortcutsDto {
            support,
            manual_command: self.manual_command(),
            bindings,
        }
    }

    /// Bind a combination to a pipeline.
    pub fn set(&self, binding: &ShortcutBindingDto, replace: bool) -> ShortcutSetDto {
        let Ok(desktop) = &self.desktop else {
            return outcome("unsupported");
        };
        let (Some(combo), Some(command)) = (
            combo_from(&binding.modifiers, &binding.key),
            self.command_for(binding),
        ) else {
            return outcome("invalid");
        };
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Re-binding keys Narrivy already holds is an update, not a conflict.
        let ours = match desktop.list() {
            Ok(entries) => entries.iter().any(|entry| {
                entry.combo.as_ref() == Some(&combo)
                    && matches!(&entry.action, Action::Spawn(cmd) if self.parse_command(cmd).is_some())
            }),
            Err(_) => return outcome("unsupported"),
        };
        match desktop.add(&combo, &command, replace || ours) {
            Ok(AddOutcome::Added { replaced }) => {
                let foreign = replaced.filter(|_| !ours);
                if let Some(raw) = &foreign {
                    // Losing the saved entry only costs the later restore;
                    // the binding the user asked for is already in place.
                    let _ = self.remember_replaced(&combo, raw);
                }
                outcome(if foreign.is_some() {
                    "replaced"
                } else {
                    "created"
                })
            }
            Ok(AddOutcome::Conflict(conflict)) => {
                let (kind, name) = match conflict {
                    Conflict::Custom(command) => ("shortcut", program_name(&command)),
                    Conflict::DesktopDefault(action) => ("desktop", safe_name(&action)),
                };
                ShortcutSetDto {
                    outcome: "conflict".to_owned(),
                    conflict_kind: Some(kind.to_owned()),
                    conflict_name: Some(name),
                }
            }
            Err(ShortcutError::InvalidCombination | ShortcutError::InvalidCommand) => {
                outcome("invalid")
            }
            Err(_) => outcome("unsupported"),
        }
    }

    /// Remove one shortcut Narrivy created, then put back the custom entry it
    /// replaced, if any. `Ok(false)` means there was no such Narrivy shortcut.
    pub fn remove(&self, modifiers: &[String], key: &str) -> Result<bool, RemoveError> {
        let desktop = self
            .desktop
            .as_ref()
            .map_err(|_| RemoveError::Unsupported)?;
        let combo = combo_from(modifiers, key).ok_or(RemoveError::Invalid)?;
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = desktop
            .remove(&combo, |command| self.parse_command(command).is_some())
            .map_err(|_| RemoveError::Store)?;
        if removed {
            if let Some(raw) = self.take_replaced(&combo) {
                // Best effort: the keys may already be in use again.
                let _ = desktop.restore(&raw);
            }
        }
        Ok(removed)
    }

    fn command_for(&self, binding: &ShortcutBindingDto) -> Option<String> {
        if binding.pipeline_id == CANCEL_TARGET {
            // Cancelling takes no language.
            return (binding.spoken_language.is_none() && binding.output_language.is_none())
                .then(|| format!("{} cancel", self.cli));
        }
        if !CorrectionMode::ALL
            .iter()
            .any(|mode| mode.pipeline_id() == binding.pipeline_id)
        {
            return None;
        }
        let mut command = format!("{} toggle --pipeline {}", self.cli, binding.pipeline_id);
        if let Some(tag) = &binding.spoken_language {
            RecognitionTagDto::parse(tag.clone()).ok()?;
            command.push_str(" --language ");
            command.push_str(tag);
        }
        if let Some(tag) = &binding.output_language {
            RecognitionTagDto::parse(tag.clone()).ok()?;
            command.push_str(" --output-language ");
            command.push_str(tag);
        }
        Some(command)
    }

    /// Parse a store command back into a Narrivy target. Anything that is not
    /// exactly the shape [`Self::command_for`] writes is not Narrivy's.
    fn parse_command(&self, command: &str) -> Option<Target> {
        let mut parts = command.split(' ');
        if parts.next()? != self.cli {
            return None;
        }
        match parts.next()? {
            "toggle" => {}
            "cancel" if parts.next().is_none() => {
                return Some(Target {
                    pipeline_id: CANCEL_TARGET.to_owned(),
                    ..Target::default()
                });
            }
            _ => return None,
        }
        let mut target = Target::default();
        let mut pipeline = None;
        while let Some(flag) = parts.next() {
            let value = parts.next()?.to_owned();
            match flag {
                "--pipeline" if pipeline.is_none() => pipeline = Some(value),
                "--language" if target.spoken_language.is_none() => {
                    target.spoken_language = Some(value)
                }
                "--output-language" if target.output_language.is_none() => {
                    target.output_language = Some(value);
                }
                _ => return None,
            }
        }
        target.pipeline_id = pipeline?;
        CorrectionMode::ALL
            .iter()
            .any(|mode| mode.pipeline_id() == target.pipeline_id)
            .then_some(target)
    }

    fn load_replaced(&self) -> BTreeMap<String, String> {
        fs::read(&self.replaced_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn store_replaced(&self, map: &BTreeMap<String, String>) -> std::io::Result<()> {
        let temp = self.replaced_path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(map).map_err(std::io::Error::other)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &self.replaced_path)
    }

    fn remember_replaced(&self, combo: &KeyCombo, raw: &str) -> std::io::Result<()> {
        let mut map = self.load_replaced();
        map.insert(combo_key(combo), raw.to_owned());
        self.store_replaced(&map)
    }

    fn take_replaced(&self, combo: &KeyCombo) -> Option<String> {
        let mut map = self.load_replaced();
        let raw = map.remove(&combo_key(combo))?;
        let _ = self.store_replaced(&map);
        Some(raw)
    }
}

/// Why a shortcut could not be removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveError {
    /// Shortcuts are not managed on this desktop.
    Unsupported,
    /// The named keys are not a valid combination.
    Invalid,
    /// The desktop's shortcut store could not be read or written.
    Store,
}

#[derive(Default)]
struct Target {
    pipeline_id: String,
    spoken_language: Option<String>,
    output_language: Option<String>,
}

fn outcome(value: &str) -> ShortcutSetDto {
    ShortcutSetDto {
        outcome: value.to_owned(),
        conflict_kind: None,
        conflict_name: None,
    }
}

fn store_reason(err: &ShortcutError) -> &'static str {
    match err {
        ShortcutError::UnrecognisedStore => "store_not_recognised",
        ShortcutError::StoreTooLarge => "store_too_large",
        _ => "store_unreadable",
    }
}

fn combo_from(modifiers: &[String], key: &str) -> Option<KeyCombo> {
    if modifiers.len() > MAX_SHORTCUT_MODIFIERS || key.len() > MAX_SHORTCUT_KEY_BYTES {
        return None;
    }
    let parsed = modifiers
        .iter()
        .map(|name| Modifier::parse(name))
        .collect::<Option<Vec<_>>>()?;
    KeyCombo::new(&parsed, key).ok()
}

fn combo_key(combo: &KeyCombo) -> String {
    let mut parts: Vec<&str> = combo.modifiers().iter().map(|m| m.as_str()).collect();
    if !combo.key().is_empty() {
        parts.push(combo.key());
    }
    parts.join("+")
}

/// The program a custom shortcut runs, as a short content-free name: the
/// file name of its first word, never its arguments.
fn program_name(command: &str) -> String {
    let first = command.split_whitespace().next().unwrap_or_default();
    safe_name(first.rsplit('/').next().unwrap_or_default())
}

fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(MAX_CONFLICT_NAME_BYTES)
        .collect();
    if cleaned.is_empty() {
        "another shortcut".to_owned()
    } else {
        cleaned
    }
}

/// The CLI beside this daemon, or the installed one. A path the shortcut
/// store could not hold safely (for example one with spaces) falls back to
/// the installed location.
fn resolve_cli() -> String {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CLI_NAME)))
        .filter(|path| path.is_file())
        .and_then(|path| path.to_str().map(str::to_owned))
        .filter(|path| {
            path.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'))
        });
    sibling.unwrap_or_else(|| INSTALLED_CLI.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const OTHER: &str =
        "{\n    (modifiers: [Ctrl], key: \"space\"): Spawn(\"/usr/bin/other --toggle\"),\n}\n";
    const DEFAULTS: &str = "{\n    (modifiers: [Super], key: \"q\"): Close,\n}\n";

    struct Fixture {
        dir: tempfile::TempDir,
        service: ShortcutService,
    }

    impl Fixture {
        fn new() -> Result<Self, std::io::Error> {
            let dir = tempfile::tempdir()?;
            fs::write(dir.path().join("custom"), OTHER)?;
            fs::write(dir.path().join("defaults"), DEFAULTS)?;
            let desktop =
                DesktopShortcuts::at(dir.path().join("custom"), dir.path().join("defaults"));
            let service =
                ShortcutService::new(Ok(desktop), "/usr/bin/narrivy-cli".to_owned(), dir.path());
            Ok(Self { dir, service })
        }

        fn custom(&self) -> Result<String, std::io::Error> {
            fs::read_to_string(self.dir.path().join("custom"))
        }
    }

    fn binding(modifiers: &[&str], key: &str, pipeline: &str) -> ShortcutBindingDto {
        ShortcutBindingDto {
            modifiers: modifiers.iter().map(|m| (*m).to_owned()).collect(),
            key: key.to_owned(),
            pipeline_id: pipeline.to_owned(),
            spoken_language: None,
            output_language: None,
        }
    }

    #[test]
    fn a_new_shortcut_is_created_listed_and_removed() -> TestResult {
        let fx = Fixture::new()?;
        let mut wanted = binding(&["Super", "Alt"], "d", "clean");
        wanted.spoken_language = Some("en".to_owned());
        assert_eq!(fx.service.set(&wanted, false).outcome, "created");
        assert!(fx.custom()?.contains(
            "(modifiers: [Super, Alt], key: \"d\"): Spawn(\"/usr/bin/narrivy-cli toggle --pipeline clean --language en\"),"
        ));
        let listed = fx.service.list();
        assert_eq!(listed.support, "managed");
        assert_eq!(listed.manual_command, "/usr/bin/narrivy-cli toggle");
        // Only Narrivy's own entry is listed; the other app's is not.
        assert_eq!(listed.bindings, vec![wanted]);
        assert_eq!(
            fx.service.remove(&["Super".into(), "Alt".into()], "d"),
            Ok(true)
        );
        assert_eq!(fx.custom()?, OTHER);
        Ok(())
    }

    #[test]
    fn keys_in_use_are_reported_with_a_content_free_name() -> TestResult {
        let fx = Fixture::new()?;
        let taken = fx.service.set(&binding(&["Ctrl"], "space", "clean"), false);
        assert_eq!(taken.outcome, "conflict");
        assert_eq!(taken.conflict_kind.as_deref(), Some("shortcut"));
        // The program's name only, never its arguments.
        assert_eq!(taken.conflict_name.as_deref(), Some("other"));
        let desktop = fx.service.set(&binding(&["Super"], "q", "clean"), false);
        assert_eq!(desktop.conflict_kind.as_deref(), Some("desktop"));
        assert_eq!(desktop.conflict_name.as_deref(), Some("Close"));
        assert_eq!(fx.custom()?, OTHER);
        Ok(())
    }

    #[test]
    fn a_confirmed_replacement_is_undone_when_the_shortcut_is_removed() -> TestResult {
        let fx = Fixture::new()?;
        assert_eq!(
            fx.service
                .set(&binding(&["Ctrl"], "space", "raw"), true)
                .outcome,
            "replaced"
        );
        assert!(!fx.custom()?.contains("/usr/bin/other"));
        assert_eq!(fx.service.remove(&["Ctrl".into()], "space"), Ok(true));
        assert_eq!(fx.custom()?, OTHER);
        Ok(())
    }

    #[test]
    fn rebinding_narrivys_own_keys_updates_without_a_conflict() -> TestResult {
        let fx = Fixture::new()?;
        assert_eq!(
            fx.service
                .set(&binding(&["Super"], "F9", "clean"), false)
                .outcome,
            "created"
        );
        assert_eq!(
            fx.service
                .set(&binding(&["Super"], "F9", "raw"), false)
                .outcome,
            "created"
        );
        let listed = fx.service.list().bindings;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].pipeline_id, "raw");
        Ok(())
    }

    #[test]
    fn another_apps_shortcut_is_never_removed() -> TestResult {
        let fx = Fixture::new()?;
        assert_eq!(fx.service.remove(&["Ctrl".into()], "space"), Ok(false));
        assert_eq!(fx.custom()?, OTHER);
        Ok(())
    }

    #[test]
    fn a_cancel_shortcut_runs_cancel() -> TestResult {
        let fx = Fixture::new()?;
        assert_eq!(
            fx.service
                .set(&binding(&["Super", "Alt"], "x", "cancel"), false)
                .outcome,
            "created"
        );
        assert!(
            fx.custom()?
                .contains("Spawn(\"/usr/bin/narrivy-cli cancel\")")
        );
        let listed = fx.service.list();
        assert!(
            listed
                .bindings
                .iter()
                .any(|b| b.pipeline_id == "cancel" && b.key == "x")
        );
        Ok(())
    }

    #[test]
    fn invalid_requests_change_nothing() -> TestResult {
        let fx = Fixture::new()?;
        let mut injected = binding(&["Super"], "F9", "clean");
        injected.spoken_language = Some("en; rm -rf ~".to_owned());
        for bad in [
            binding(&[], "d", "clean"),
            binding(&["Hyper"], "d", "clean"),
            binding(&["Super"], "F9", "not-a-pipeline"),
            injected,
        ] {
            assert_eq!(fx.service.set(&bad, false).outcome, "invalid");
        }
        assert_eq!(fx.custom()?, OTHER);
        Ok(())
    }

    #[test]
    fn an_unknown_desktop_reports_the_manual_route() -> TestResult {
        let dir = tempfile::tempdir()?;
        let service = ShortcutService::new(
            Err("desktop_not_supported"),
            INSTALLED_CLI.to_owned(),
            dir.path(),
        );
        assert_eq!(
            service.capability(),
            "activation.shortcuts=manual:desktop_not_supported"
        );
        assert_eq!(
            service
                .set(&binding(&["Super"], "F9", "clean"), false)
                .outcome,
            "unsupported"
        );
        assert!(service.list().bindings.is_empty());
        assert_eq!(
            service.remove(&["Super".into()], "F9"),
            Err(RemoveError::Unsupported)
        );
        Ok(())
    }
}
