//! Desktop-managed keyboard shortcuts: read and edit the desktop's own
//! custom-shortcut store so a key combination runs one command.
//!
//! This is the "native desktop binding" route of
//! `docs/architecture/shortcut-router.md`, used where the desktop offers no
//! global-shortcuts portal. One store shape is supported: the COSMIC
//! custom-shortcut file, a RON map of
//! `(modifiers: [Super], key: "d"): Spawn("command"),` entries.
//!
//! Invariants:
//! - Edits are text splices. Every entry this module does not add or remove
//!   stays byte-for-byte as it was.
//! - A file whose shape is not recognised is never written; the caller
//!   reports the manual route instead.
//! - Writes are atomic (temporary file beside the target, then rename).
//! - This module owns no product policy: which commands belong to Narrivy,
//!   and whether a conflict may be replaced, is the caller's decision.

#![forbid(unsafe_code)]

use std::fmt;
use std::fs;
use std::io::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

const STORE_DIR: &str = "cosmic/com.system76.CosmicSettings.Shortcuts/v1";
const SYSTEM_DEFAULTS: &str = "/usr/share/cosmic/com.system76.CosmicSettings.Shortcuts/v1/defaults";
const DESKTOP_TOKEN: &str = "COSMIC";

/// Upper bound on a shortcut store this module will read.
const MAX_STORE_BYTES: u64 = 256 * 1024;
/// Upper bound on a command written into the store.
pub const MAX_COMMAND_BYTES: usize = 512;

/// A modifier key, named as the store names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Super,
    Ctrl,
    Alt,
    Shift,
}

impl Modifier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Super => "Super",
            Self::Ctrl => "Ctrl",
            Self::Alt => "Alt",
            Self::Shift => "Shift",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Super" => Some(Self::Super),
            "Ctrl" => Some(Self::Ctrl),
            "Alt" => Some(Self::Alt),
            "Shift" => Some(Self::Shift),
            _ => None,
        }
    }
}

/// A key combination: a set of modifiers plus one key name, or modifiers
/// alone (an empty key), which the desktop runs when they are released
/// without another key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    modifiers: Vec<Modifier>,
    key: String,
}

impl KeyCombo {
    /// Build a combination a user may bind: one to four modifiers alone, a
    /// key that types nothing on its own (a function key, Pause, Insert and
    /// so on), or an allowlisted key with a modifier other than Shift. A key
    /// that types (a letter, a digit, Space, punctuation) is refused on its
    /// own or with only Shift, because the desktop would take it from every
    /// application.
    pub fn new(modifiers: &[Modifier], key: &str) -> Result<Self, ShortcutError> {
        let mut modifiers = modifiers.to_vec();
        modifiers.sort();
        modifiers.dedup();
        if key.is_empty() {
            if modifiers.is_empty() {
                return Err(ShortcutError::InvalidCombination);
            }
            return Ok(Self {
                modifiers,
                key: String::new(),
            });
        }
        let key = canonical_key(key).ok_or(ShortcutError::InvalidCombination)?;
        let strong = modifiers.iter().any(|m| *m != Modifier::Shift);
        if !strong && !is_standalone_key(&key) {
            return Err(ShortcutError::InvalidCombination);
        }
        Ok(Self { modifiers, key })
    }

    fn parsed(mut modifiers: Vec<Modifier>, key: String) -> Self {
        modifiers.sort();
        modifiers.dedup();
        Self { modifiers, key }
    }

    pub fn modifiers(&self) -> &[Modifier] {
        &self.modifiers
    }

    /// The key name, empty for a modifiers-only combination.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Letters compare without case: the desktop treats `d` and `D` alike.
    fn same_keys(&self, other: &Self) -> bool {
        self.modifiers == other.modifiers && self.key.eq_ignore_ascii_case(&other.key)
    }
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

/// Keys that type nothing, so binding one on its own takes no text input
/// away from other applications.
fn is_standalone_key(key: &str) -> bool {
    is_function_key(key) || matches!(key, "Pause" | "Insert" | "Scroll_Lock" | "Print" | "Menu")
}

/// The key names a user may bind, in the spelling the store uses.
fn canonical_key(key: &str) -> Option<String> {
    const NAMED: &[&str] = &[
        "space",
        "Insert",
        "Home",
        "End",
        "Prior",
        "Next",
        "Pause",
        "Scroll_Lock",
        "Print",
        "Menu",
        "comma",
        "period",
        "slash",
        "semicolon",
        "apostrophe",
        "grave",
        "minus",
        "equal",
        "bracketleft",
        "bracketright",
        "backslash",
    ];
    if key.len() == 1 {
        let c = key.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(c.to_ascii_lowercase().to_string());
        }
        if c.is_ascii_digit() {
            return Some(c.to_string());
        }
        return None;
    }
    if is_function_key(key) {
        return Some(key.to_owned());
    }
    NAMED
        .iter()
        .find(|name| **name == key)
        .map(|name| (*name).to_owned())
}

/// What one store entry does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Runs a command.
    Spawn(String),
    /// Anything else (a window-manager action, a system action). Only the
    /// leading identifier is kept, as a content-free label.
    Other(String),
}

/// One entry of a shortcut store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `None` when the entry binds a raw key code rather than a key name.
    pub combo: Option<KeyCombo>,
    pub action: Action,
    span: Range<usize>,
}

/// Something that already uses a key combination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Another custom shortcut; carries its command.
    Custom(String),
    /// A shortcut the desktop ships; carries the action's name.
    DesktopDefault(String),
}

/// Result of [`DesktopShortcuts::add`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddOutcome {
    Added {
        /// The custom entry that was replaced, verbatim, so it can be
        /// restored later. `None` when nothing was replaced.
        replaced: Option<String>,
    },
    Conflict(Conflict),
}

#[derive(Debug)]
pub enum ShortcutError {
    /// The combination is not one a user may bind.
    InvalidCombination,
    /// The command holds characters this module will not write.
    InvalidCommand,
    /// The store exists but is not in the recognised shape; nothing was
    /// written.
    UnrecognisedStore,
    /// The store is larger than this module will read.
    StoreTooLarge,
    Io(std::io::Error),
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCombination => f.write_str("key combination not allowed"),
            Self::InvalidCommand => f.write_str("command not allowed"),
            Self::UnrecognisedStore => f.write_str("shortcut store not recognised"),
            Self::StoreTooLarge => f.write_str("shortcut store too large"),
            Self::Io(err) => write!(f, "shortcut store I/O: {err}"),
        }
    }
}

impl std::error::Error for ShortcutError {}

impl From<std::io::Error> for ShortcutError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Why the desktop's shortcuts cannot be managed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmanaged {
    /// The session is not a desktop this module knows how to edit.
    UnknownDesktop,
    /// No user configuration directory could be resolved.
    NoConfigDir,
}

impl Unmanaged {
    pub fn reason(self) -> &'static str {
        match self {
            Self::UnknownDesktop => "desktop_not_supported",
            Self::NoConfigDir => "config_dir_unavailable",
        }
    }
}

/// The desktop's custom-shortcut store.
#[derive(Debug, Clone)]
pub struct DesktopShortcuts {
    custom: PathBuf,
    defaults: PathBuf,
}

impl DesktopShortcuts {
    /// Find the store for the running session, from the session's
    /// environment (`XDG_CURRENT_DESKTOP`, `XDG_CONFIG_HOME`, `HOME`).
    pub fn detect() -> Result<Self, Unmanaged> {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        if !desktop
            .split(':')
            .any(|part| part.eq_ignore_ascii_case(DESKTOP_TOKEN))
        {
            return Err(Unmanaged::UnknownDesktop);
        }
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or(Unmanaged::NoConfigDir)?;
        Ok(Self::at(
            config.join(STORE_DIR).join("custom"),
            PathBuf::from(SYSTEM_DEFAULTS),
        ))
    }

    /// A store at explicit paths (tests, and callers that resolved them).
    pub fn at(custom: PathBuf, defaults: PathBuf) -> Self {
        Self { custom, defaults }
    }

    /// Every entry of the user's custom store. A missing store is empty.
    pub fn list(&self) -> Result<Vec<Entry>, ShortcutError> {
        parse_store(&read_store(&self.custom)?)
    }

    /// What already uses `combo`, if anything. Custom entries are checked
    /// before the desktop's defaults.
    pub fn conflict(&self, combo: &KeyCombo) -> Result<Option<Conflict>, ShortcutError> {
        if let Some(entry) = find(&self.list()?, combo) {
            return Ok(Some(match &entry.action {
                Action::Spawn(command) => Conflict::Custom(command.clone()),
                Action::Other(name) => Conflict::Custom(name.clone()),
            }));
        }
        // An unreadable or unrecognised defaults file is not evidence of a
        // conflict; the desktop itself resolves overlaps with its defaults.
        let defaults = read_store(&self.defaults)
            .and_then(|text| parse_store(&text))
            .unwrap_or_default();
        Ok(find(&defaults, combo).map(|entry| match &entry.action {
            Action::Spawn(command) => Conflict::DesktopDefault(command.clone()),
            Action::Other(name) => Conflict::DesktopDefault(name.clone()),
        }))
    }

    /// Bind `combo` to `command`. With `replace` false, an existing use of
    /// the combination is reported and nothing is written. With `replace`
    /// true, a custom entry on the same keys is removed and returned; a
    /// desktop default is overridden by the new custom entry.
    pub fn add(
        &self,
        combo: &KeyCombo,
        command: &str,
        replace: bool,
    ) -> Result<AddOutcome, ShortcutError> {
        validate_command(command)?;
        if !replace {
            if let Some(conflict) = self.conflict(combo)? {
                return Ok(AddOutcome::Conflict(conflict));
            }
        }
        let mut text = read_store(&self.custom)?;
        let entries = parse_store(&text)?;
        let mut replaced = None;
        if let Some(existing) = find(&entries, combo) {
            replaced = Some(text[existing.span.clone()].trim().to_owned());
            text.replace_range(existing.span.clone(), "");
        }
        let close = text.rfind('}').ok_or(ShortcutError::UnrecognisedStore)?;
        let mut line = String::new();
        if !text[..close].ends_with('\n') {
            line.push('\n');
        }
        line.push_str(&render_entry(combo, command));
        text.insert_str(close, &line);
        // The result must still parse and must hold the new binding.
        let check = parse_store(&text)?;
        if !check.iter().any(|entry| {
            entry.combo.as_ref().is_some_and(|c| c.same_keys(combo))
                && entry.action == Action::Spawn(command.to_owned())
        }) {
            return Err(ShortcutError::UnrecognisedStore);
        }
        write_store(&self.custom, &text)?;
        Ok(AddOutcome::Added { replaced })
    }

    /// Remove the entry on `combo`, but only if `owns` accepts its command.
    /// Returns whether an entry was removed.
    pub fn remove(
        &self,
        combo: &KeyCombo,
        owns: impl Fn(&str) -> bool,
    ) -> Result<bool, ShortcutError> {
        let mut text = read_store(&self.custom)?;
        let entries = parse_store(&text)?;
        let Some(entry) = find(&entries, combo) else {
            return Ok(false);
        };
        match &entry.action {
            Action::Spawn(command) if owns(command) => {}
            _ => return Ok(false),
        }
        text.replace_range(entry.span.clone(), "");
        parse_store(&text)?;
        write_store(&self.custom, &text)?;
        Ok(true)
    }

    /// Put back an entry previously returned by [`Self::add`] as `replaced`,
    /// unless its keys are in use again. Returns whether it was restored.
    pub fn restore(&self, raw_entry: &str) -> Result<bool, ShortcutError> {
        let wrapped = format!("{{\n{raw_entry}\n}}");
        let parsed = parse_store(&wrapped)?;
        let [entry] = parsed.as_slice() else {
            return Err(ShortcutError::UnrecognisedStore);
        };
        let mut text = read_store(&self.custom)?;
        let entries = parse_store(&text)?;
        if let Some(combo) = &entry.combo {
            if find(&entries, combo).is_some() {
                return Ok(false);
            }
        }
        let close = text.rfind('}').ok_or(ShortcutError::UnrecognisedStore)?;
        let mut line = String::new();
        if !text[..close].ends_with('\n') {
            line.push('\n');
        }
        line.push_str("    ");
        line.push_str(raw_entry.trim().trim_end_matches(','));
        line.push_str(",\n");
        text.insert_str(close, &line);
        parse_store(&text)?;
        write_store(&self.custom, &text)?;
        Ok(true)
    }
}

fn find<'a>(entries: &'a [Entry], combo: &KeyCombo) -> Option<&'a Entry> {
    entries
        .iter()
        .find(|entry| entry.combo.as_ref().is_some_and(|c| c.same_keys(combo)))
}

fn validate_command(command: &str) -> Result<(), ShortcutError> {
    let ok = !command.is_empty()
        && command.len() <= MAX_COMMAND_BYTES
        && command.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b' ' | b'/' | b'-' | b'_' | b'.' | b'+' | b'=' | b':')
        });
    if ok {
        Ok(())
    } else {
        Err(ShortcutError::InvalidCommand)
    }
}

fn render_entry(combo: &KeyCombo, command: &str) -> String {
    let modifiers = combo
        .modifiers
        .iter()
        .map(|m| m.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    // `validate_command` admits neither quotes nor backslashes, so the
    // command needs no escaping. A modifiers-only combination has no key
    // field, as the desktop's own Super-alone binding is written.
    if combo.key.is_empty() {
        format!("    (modifiers: [{modifiers}]): Spawn(\"{command}\"),\n")
    } else {
        format!(
            "    (modifiers: [{modifiers}], key: \"{}\"): Spawn(\"{command}\"),\n",
            combo.key
        )
    }
}

fn read_store(path: &Path) -> Result<String, ShortcutError> {
    match fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_STORE_BYTES => return Err(ShortcutError::StoreTooLarge),
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok("{\n}\n".to_owned()),
        Err(err) => return Err(err.into()),
    }
    Ok(fs::read_to_string(path)?)
}

fn write_store(path: &Path, text: &str) -> Result<(), ShortcutError> {
    let dir = path.parent().ok_or(ShortcutError::UnrecognisedStore)?;
    fs::create_dir_all(dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(text.as_bytes())?;
    temp.as_file().sync_all()?;
    // Keep the existing file's mode; a new store is readable like the
    // desktop's own files.
    let mode = fs::metadata(path)
        .map(|meta| meta.permissions())
        .unwrap_or_else(|_| std::os::unix::fs::PermissionsExt::from_mode(0o644));
    fs::set_permissions(temp.path(), mode)?;
    temp.persist(path)
        .map_err(|err| ShortcutError::Io(err.error))?;
    Ok(())
}

// ------------------------------------------------------------------ parsing

/// Parse a store into entries with their byte spans. Anything outside the
/// recognised shape is an error, so an unfamiliar file is never edited.
fn parse_store(text: &str) -> Result<Vec<Entry>, ShortcutError> {
    let bytes = text.as_bytes();
    let mut pos = skip_blank(bytes, 0);
    if bytes.get(pos) != Some(&b'{') {
        return Err(ShortcutError::UnrecognisedStore);
    }
    pos += 1;
    let mut entries = Vec::new();
    loop {
        pos = skip_blank(bytes, pos);
        // An entry's span is its own line: its indentation through its line
        // ending. Comments and blank lines around it are left alone.
        let mut span_start = pos;
        while span_start > 0 && matches!(bytes[span_start - 1], b' ' | b'\t') {
            span_start -= 1;
        }
        match bytes.get(pos) {
            Some(b'}') => {
                if skip_blank(bytes, pos + 1) != bytes.len() {
                    return Err(ShortcutError::UnrecognisedStore);
                }
                return Ok(entries);
            }
            Some(b'(') => {}
            _ => return Err(ShortcutError::UnrecognisedStore),
        }
        let key_end = matching(bytes, pos)?;
        let binding = &text[pos + 1..key_end];
        pos = skip_blank(bytes, key_end + 1);
        if bytes.get(pos) != Some(&b':') {
            return Err(ShortcutError::UnrecognisedStore);
        }
        pos = skip_blank(bytes, pos + 1);
        let action_start = pos;
        pos = action_end(bytes, pos)?;
        let action = parse_action(text[action_start..pos].trim())?;
        // Consume the separating comma and the rest of that line.
        let mut end = skip_inline_space(bytes, pos);
        if bytes.get(end) == Some(&b',') {
            end = skip_inline_space(bytes, end + 1);
        } else if bytes.get(skip_blank(bytes, end)) != Some(&b'}') {
            return Err(ShortcutError::UnrecognisedStore);
        }
        if bytes.get(end) == Some(&b'\n') {
            end += 1;
        }
        entries.push(Entry {
            combo: parse_binding(binding)?,
            action,
            span: span_start..end,
        });
        pos = end;
    }
}

/// Skip whitespace and `//` line comments.
fn skip_blank(bytes: &[u8], mut pos: usize) -> usize {
    loop {
        while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        if bytes.get(pos) == Some(&b'/') && bytes.get(pos + 1) == Some(&b'/') {
            while bytes.get(pos).is_some_and(|b| *b != b'\n') {
                pos += 1;
            }
        } else {
            return pos;
        }
    }
}

fn skip_inline_space(bytes: &[u8], mut pos: usize) -> usize {
    while matches!(bytes.get(pos), Some(b' ' | b'\t' | b'\r')) {
        pos += 1;
    }
    pos
}

/// Index just past a string literal that starts at `pos`.
fn string_end(bytes: &[u8], pos: usize) -> Result<usize, ShortcutError> {
    let mut i = pos + 1;
    while let Some(b) = bytes.get(i) {
        match b {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err(ShortcutError::UnrecognisedStore)
}

/// Index of the bracket that closes the one at `open`.
fn matching(bytes: &[u8], open: usize) -> Result<usize, ShortcutError> {
    let mut depth = 0usize;
    let mut i = open;
    while let Some(b) = bytes.get(i) {
        match b {
            b'"' => {
                i = string_end(bytes, i)?;
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or(ShortcutError::UnrecognisedStore)?;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(ShortcutError::UnrecognisedStore)
}

/// Index where an action ends: the top-level comma or the closing brace.
fn action_end(bytes: &[u8], start: usize) -> Result<usize, ShortcutError> {
    let mut depth = 0usize;
    let mut i = start;
    while let Some(b) = bytes.get(i) {
        match b {
            b'"' => {
                i = string_end(bytes, i)?;
                continue;
            }
            b'(' | b'[' => depth += 1,
            b')' | b']' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or(ShortcutError::UnrecognisedStore)?
            }
            b',' | b'}' if depth == 0 => return Ok(i),
            b'{' | b'}' => return Err(ShortcutError::UnrecognisedStore),
            _ => {}
        }
        i += 1;
    }
    Err(ShortcutError::UnrecognisedStore)
}

fn parse_string(literal: &str) -> Result<String, ShortcutError> {
    let inner = literal
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or(ShortcutError::UnrecognisedStore)?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                _ => return Err(ShortcutError::UnrecognisedStore),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn parse_action(action: &str) -> Result<Action, ShortcutError> {
    if let Some(rest) = action.strip_prefix("Spawn(") {
        let literal = rest
            .strip_suffix(')')
            .ok_or(ShortcutError::UnrecognisedStore)?;
        return Ok(Action::Spawn(parse_string(literal.trim())?));
    }
    let name: String = action
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return Err(ShortcutError::UnrecognisedStore);
    }
    Ok(Action::Other(name))
}

/// Parse the inside of `(modifiers: [...], key: "...")`. Other fields (a
/// description, a raw key code) are tolerated and ignored.
fn parse_binding(binding: &str) -> Result<Option<KeyCombo>, ShortcutError> {
    let bytes = binding.as_bytes();
    let mut modifiers = None;
    let mut key = None;
    let mut keycode = false;
    let mut pos = 0;
    while pos < bytes.len() {
        pos = skip_blank(bytes, pos);
        let name_start = pos;
        while bytes
            .get(pos)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            pos += 1;
        }
        let name = &binding[name_start..pos];
        pos = skip_blank(bytes, pos);
        if name.is_empty() || bytes.get(pos) != Some(&b':') {
            return Err(ShortcutError::UnrecognisedStore);
        }
        pos = skip_blank(bytes, pos + 1);
        let value_start = pos;
        let mut depth = 0usize;
        while let Some(b) = bytes.get(pos) {
            match b {
                b'"' => {
                    pos = string_end(bytes, pos)?;
                    continue;
                }
                b'(' | b'[' => depth += 1,
                b')' | b']' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or(ShortcutError::UnrecognisedStore)?
                }
                b',' if depth == 0 => break,
                _ => {}
            }
            pos += 1;
        }
        let value = binding[value_start..pos].trim();
        pos += 1;
        match name {
            "modifiers" => {
                let inner = value
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(']'))
                    .ok_or(ShortcutError::UnrecognisedStore)?;
                let mut list = Vec::new();
                for part in inner
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                {
                    list.push(Modifier::parse(part).ok_or(ShortcutError::UnrecognisedStore)?);
                }
                modifiers = Some(list);
            }
            "key" => key = Some(parse_string(value)?),
            "keycode" => keycode = true,
            _ => {}
        }
    }
    let modifiers = modifiers.ok_or(ShortcutError::UnrecognisedStore)?;
    match key {
        Some(key) => Ok(Some(KeyCombo::parsed(modifiers, key))),
        // Modifiers alone, such as the desktop's Super for the launcher.
        None if !keycode && !modifiers.is_empty() => {
            Ok(Some(KeyCombo::parsed(modifiers, String::new())))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const EXISTING: &str =
        "{\n    (modifiers: [Ctrl], key: \"space\"): Spawn(\"/usr/bin/other --toggle\"),\n}\n";
    const DEFAULTS: &str = "{\n    (modifiers: [Super], key: \"q\"): Close,\n    (modifiers: [Super], key: \"t\"): System(Terminal),\n}\n";
    const COMMAND: &str = "/usr/bin/narrivy-cli toggle --pipeline clean";

    fn store(
        custom: Option<&str>,
    ) -> Result<(tempfile::TempDir, DesktopShortcuts), std::io::Error> {
        let dir = tempfile::tempdir()?;
        let custom_path = dir.path().join("v1").join("custom");
        let defaults_path = dir.path().join("defaults");
        fs::write(&defaults_path, DEFAULTS)?;
        if let Some(text) = custom {
            fs::create_dir_all(dir.path().join("v1"))?;
            fs::write(&custom_path, text)?;
        }
        Ok((dir, DesktopShortcuts::at(custom_path, defaults_path)))
    }

    fn combo(modifiers: &[Modifier], key: &str) -> Result<KeyCombo, ShortcutError> {
        KeyCombo::new(modifiers, key)
    }

    #[test]
    fn a_bare_printable_key_is_refused_and_a_function_key_is_allowed() -> TestResult {
        assert!(matches!(
            combo(&[], "d"),
            Err(ShortcutError::InvalidCombination)
        ));
        assert!(matches!(
            combo(&[Modifier::Shift], "d"),
            Err(ShortcutError::InvalidCombination)
        ));
        assert!(matches!(
            combo(&[Modifier::Ctrl], "Escape"),
            Err(ShortcutError::InvalidCombination)
        ));
        assert!(matches!(
            combo(&[], "space"),
            Err(ShortcutError::InvalidCombination)
        ));
        assert!(matches!(
            combo(&[], ""),
            Err(ShortcutError::InvalidCombination)
        ));
        assert_eq!(combo(&[], "F9")?.key(), "F9");
        assert_eq!(combo(&[Modifier::Ctrl, Modifier::Alt], "D")?.key(), "d");
        Ok(())
    }

    #[test]
    fn single_keys_and_modifiers_alone_are_allowed() -> TestResult {
        for key in [
            "F13",
            "F24",
            "Pause",
            "Insert",
            "Scroll_Lock",
            "Print",
            "Menu",
        ] {
            assert_eq!(combo(&[], key)?.key(), key);
        }
        assert!(matches!(
            combo(&[], "F25"),
            Err(ShortcutError::InvalidCombination)
        ));
        let alone = combo(&[Modifier::Ctrl], "")?;
        assert_eq!(
            (alone.modifiers(), alone.key()),
            (&[Modifier::Ctrl][..], "")
        );
        let four = combo(
            &[
                Modifier::Super,
                Modifier::Ctrl,
                Modifier::Alt,
                Modifier::Shift,
            ],
            "k",
        )?;
        assert_eq!(four.modifiers().len(), 4);
        Ok(())
    }

    #[test]
    fn a_modifiers_only_binding_is_written_without_a_key_and_meets_the_desktops_own() -> TestResult
    {
        let (dir, shortcuts) = store(Some(EXISTING))?;
        let alone = combo(&[Modifier::Alt], "")?;
        assert_eq!(
            shortcuts.add(&alone, COMMAND, false)?,
            AddOutcome::Added { replaced: None }
        );
        let text = fs::read_to_string(dir.path().join("v1/custom"))?;
        assert!(text.contains("(modifiers: [Alt]): Spawn("));
        assert!(!text.contains("key: \"\""));
        let entries = parse_store(&text)?;
        assert!(
            entries
                .iter()
                .any(|entry| entry.combo.as_ref() == Some(&alone))
        );

        let launcher = parse_store(
            "{\n    (modifiers: [Super]): System(Launcher),\n    (modifiers: [], keycode: Some(123)): System(VolumeLower),\n}\n",
        )?;
        let combos: Vec<_> = launcher.iter().map(|entry| entry.combo.clone()).collect();
        assert_eq!(combos, vec![Some(combo(&[Modifier::Super], "")?), None]);
        Ok(())
    }

    #[test]
    fn adding_keeps_every_other_entry_byte_for_byte() -> TestResult {
        let (dir, shortcuts) = store(Some(EXISTING))?;
        let keys = combo(&[Modifier::Alt, Modifier::Super], "d")?;
        assert_eq!(
            shortcuts.add(&keys, COMMAND, false)?,
            AddOutcome::Added { replaced: None }
        );
        let text = fs::read_to_string(dir.path().join("v1/custom"))?;
        assert_eq!(
            text,
            "{\n    (modifiers: [Ctrl], key: \"space\"): Spawn(\"/usr/bin/other --toggle\"),\n    (modifiers: [Super, Alt], key: \"d\"): Spawn(\"/usr/bin/narrivy-cli toggle --pipeline clean\"),\n}\n"
        );
        assert_eq!(shortcuts.list()?.len(), 2);
        Ok(())
    }

    #[test]
    fn a_missing_store_is_created_with_the_one_binding() -> TestResult {
        let (_dir, shortcuts) = store(None)?;
        assert!(shortcuts.list()?.is_empty());
        let keys = combo(&[Modifier::Super], "F9")?;
        shortcuts.add(&keys, COMMAND, false)?;
        let entries = shortcuts.list()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::Spawn(COMMAND.to_owned()));
        Ok(())
    }

    #[test]
    fn keys_in_use_are_reported_and_nothing_is_written() -> TestResult {
        let (dir, shortcuts) = store(Some(EXISTING))?;
        let taken = combo(&[Modifier::Ctrl], "space")?;
        assert_eq!(
            shortcuts.add(&taken, COMMAND, false)?,
            AddOutcome::Conflict(Conflict::Custom("/usr/bin/other --toggle".to_owned()))
        );
        let desktop = combo(&[Modifier::Super], "Q")?;
        assert_eq!(
            shortcuts.add(&desktop, COMMAND, false)?,
            AddOutcome::Conflict(Conflict::DesktopDefault("Close".to_owned()))
        );
        assert_eq!(fs::read_to_string(dir.path().join("v1/custom"))?, EXISTING);
        Ok(())
    }

    #[test]
    fn replacing_returns_the_old_entry_and_restore_puts_it_back() -> TestResult {
        let (dir, shortcuts) = store(Some(EXISTING))?;
        let taken = combo(&[Modifier::Ctrl], "space")?;
        let AddOutcome::Added {
            replaced: Some(old),
        } = shortcuts.add(&taken, COMMAND, true)?
        else {
            return Err("expected a replaced entry".into());
        };
        assert_eq!(
            old,
            "(modifiers: [Ctrl], key: \"space\"): Spawn(\"/usr/bin/other --toggle\"),"
        );
        assert_eq!(shortcuts.list()?.len(), 1);

        // While Narrivy still holds the keys the old entry is not restored.
        assert!(!shortcuts.restore(&old)?);
        assert!(shortcuts.remove(&taken, |command| command == COMMAND)?);
        assert!(shortcuts.restore(&old)?);
        assert_eq!(fs::read_to_string(dir.path().join("v1/custom"))?, EXISTING);
        Ok(())
    }

    #[test]
    fn remove_only_touches_a_binding_the_caller_owns() -> TestResult {
        let (dir, shortcuts) = store(Some(EXISTING))?;
        let other = combo(&[Modifier::Ctrl], "space")?;
        assert!(!shortcuts.remove(&other, |command| {
            command.starts_with("/usr/bin/narrivy-cli ")
        })?);
        assert_eq!(fs::read_to_string(dir.path().join("v1/custom"))?, EXISTING);
        Ok(())
    }

    #[test]
    fn an_unrecognised_store_is_never_written() -> TestResult {
        for text in [
            "[1, 2]",
            "{ (modifiers: [Hyper], key: \"d\"): Close, }",
            "{ oops }",
            "{ (key: \"d\"): Close, } trailing",
        ] {
            let (dir, shortcuts) = store(Some(text))?;
            let keys = combo(&[Modifier::Super], "F9")?;
            assert!(matches!(
                shortcuts.add(&keys, COMMAND, false),
                Err(ShortcutError::UnrecognisedStore)
            ));
            assert_eq!(fs::read_to_string(dir.path().join("v1/custom"))?, text);
        }
        Ok(())
    }

    #[test]
    fn comments_descriptions_and_key_codes_are_tolerated() -> TestResult {
        let text = "{\n    // my terminal\n    (modifiers: [Super], key: \"t\", description: Some(\"Terminal, mine\")): Spawn(\"term --x \\\"y\\\"\"),\n    (modifiers: [], keycode: Some(123)): System(VolumeLower)\n}\n";
        let (_dir, shortcuts) = store(Some(text))?;
        let entries = shortcuts.list()?;
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].action,
            Action::Spawn("term --x \"y\"".to_owned())
        );
        assert!(entries[1].combo.is_none());
        assert_eq!(entries[1].action, Action::Other("System".to_owned()));
        Ok(())
    }

    #[test]
    fn a_command_with_shell_characters_is_refused() -> TestResult {
        let (_dir, shortcuts) = store(Some(EXISTING))?;
        let keys = combo(&[Modifier::Super], "F9")?;
        for command in [
            "narrivy-cli toggle; rm -rf ~",
            "a \"b\"",
            "a\\b",
            "a\nb",
            "$(x)",
            "",
        ] {
            assert!(matches!(
                shortcuts.add(&keys, command, false),
                Err(ShortcutError::InvalidCommand)
            ));
        }
        Ok(())
    }
}
