//! The shortcut model, mapped to Win32.
//!
//! A shortcut is the same thing the Linux side binds: a set of modifiers
//! (`Super`, `Ctrl`, `Alt`, `Shift`) and one key, named as the Linux store
//! names keys (`d`, `F9`, `space`, `comma`, `Insert`, ...), so a setting
//! written on one platform means the same keys on the other. The rule for
//! what may be bound is the same too: a key that types (a letter, a digit,
//! Space, punctuation) needs a modifier other than Shift, because a global
//! shortcut takes the combination away from every application.
//!
//! Toggle shortcuts with a key are registered with `RegisterHotKey`, which
//! reports presses only. Modifiers alone (such as `Super+Ctrl`, which Linux
//! desktops bind too) are something `RegisterHotKey` cannot take; the
//! low-level keyboard hook watches them with [`ChordTracker`], firing when
//! they are released with no other key pressed in between, as desktops do. Press-and-hold uses one of the same keys the Linux hold
//! reader offers, watched for press and release by a low-level keyboard hook
//! that ignores every other key.
//!
//! Pure mapping; no OS calls.

#![forbid(unsafe_code)]

use std::fmt;

/// A modifier key, named as the Linux shortcut store names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Super,
    Ctrl,
    Alt,
    Shift,
}

impl Modifier {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Super => "Super",
            Self::Ctrl => "Ctrl",
            Self::Alt => "Alt",
            Self::Shift => "Shift",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Super" => Some(Self::Super),
            "Ctrl" => Some(Self::Ctrl),
            "Alt" => Some(Self::Alt),
            "Shift" => Some(Self::Shift),
            _ => None,
        }
    }

    /// `RegisterHotKey` modifier flag.
    #[must_use]
    pub fn win32_flag(self) -> u32 {
        match self {
            Self::Alt => MOD_ALT,
            Self::Ctrl => MOD_CONTROL,
            Self::Shift => MOD_SHIFT,
            Self::Super => MOD_WIN,
        }
    }
}

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
/// Holding the keys sends one press, not auto-repeat.
pub const MOD_NOREPEAT: u32 = 0x4000;

/// Why a combination cannot be a Windows global shortcut. Content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutError {
    /// Unknown key or modifier name, or a typing key without a strong
    /// modifier (the same rule as Linux).
    InvalidCombination,
}

impl ShortcutError {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidCombination => "shortcut_invalid_combination",
        }
    }
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ShortcutError {}

/// A combination ready for `RegisterHotKey`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hotkey {
    modifiers: Vec<Modifier>,
    key: String,
    vk: u16,
}

impl Hotkey {
    /// # Errors
    /// [`ShortcutError`] (see its variants).
    pub fn new(modifiers: &[Modifier], key: &str) -> Result<Self, ShortcutError> {
        let mut modifiers = modifiers.to_vec();
        modifiers.sort();
        modifiers.dedup();
        if key.is_empty() {
            // One to four modifiers alone, as on Linux; watched by the hook.
            if modifiers.is_empty() {
                return Err(ShortcutError::InvalidCombination);
            }
            return Ok(Self {
                modifiers,
                key: String::new(),
                vk: 0,
            });
        }
        let (key, vk) = canonical_key(key).ok_or(ShortcutError::InvalidCombination)?;
        let strong = modifiers.iter().any(|m| *m != Modifier::Shift);
        if !strong && !is_standalone_key(&key) {
            return Err(ShortcutError::InvalidCombination);
        }
        Ok(Self { modifiers, key, vk })
    }

    /// Parse the `+`-joined form, such as `Super+Shift+d` or `Ctrl+space`.
    ///
    /// # Errors
    /// [`ShortcutError::InvalidCombination`] for an unknown name.
    pub fn parse(value: &str) -> Result<Self, ShortcutError> {
        let mut parts: Vec<&str> = value.split('+').map(str::trim).collect();
        let key = parts.pop().ok_or(ShortcutError::InvalidCombination)?;
        let mut modifiers = Vec::with_capacity(parts.len());
        for part in parts {
            modifiers.push(Modifier::parse(part).ok_or(ShortcutError::InvalidCombination)?);
        }
        // `Super+Ctrl` alone: the last part is a modifier, not a key.
        if let Some(last) = Modifier::parse(key) {
            modifiers.push(last);
            return Self::new(&modifiers, "");
        }
        Self::new(&modifiers, key)
    }

    #[must_use]
    pub fn modifiers(&self) -> &[Modifier] {
        &self.modifiers
    }

    /// The key name, empty for modifiers alone.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Modifiers alone: watched by the keyboard hook, not `RegisterHotKey`.
    #[must_use]
    pub fn is_modifiers_only(&self) -> bool {
        self.key.is_empty()
    }

    /// `RegisterHotKey` arguments: modifier flags (with `MOD_NOREPEAT`) and
    /// the virtual-key code.
    #[must_use]
    pub fn win32(&self) -> (u32, u32) {
        let flags = self
            .modifiers
            .iter()
            .fold(MOD_NOREPEAT, |flags, m| flags | m.win32_flag());
        (flags, u32::from(self.vk))
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self
            .modifiers
            .iter()
            .map(|modifier| modifier.as_str())
            .chain((!self.key.is_empty()).then_some(self.key.as_str()))
            .collect();
        formatter.write_str(&names.join("+"))
    }
}

/// The modifier a virtual key belongs to, left, right or either.
#[must_use]
pub fn modifier_of(vk: u32) -> Option<Modifier> {
    match vk {
        0x10 | 0xA0 | 0xA1 => Some(Modifier::Shift),
        0x11 | 0xA2 | 0xA3 => Some(Modifier::Ctrl),
        0x12 | 0xA4 | 0xA5 => Some(Modifier::Alt),
        0x5B | 0x5C => Some(Modifier::Super),
        _ => None,
    }
}

/// Watches one modifiers-only shortcut from raw key events. It fires once,
/// on the first release, when exactly its modifiers were held and no other
/// key (or extra modifier) went down since the first of them; nothing is
/// fired again until every key is up. Only key codes are seen, never kept
/// beyond the keys currently held.
#[derive(Debug, Clone, Default)]
pub struct ChordTracker {
    target: Vec<Modifier>,
    held: Vec<u32>,
    complete: bool,
    spoiled: bool,
}

/// The most keys counted as held at once; more spoils the chord.
const MAX_HELD: usize = 8;

impl ChordTracker {
    #[must_use]
    pub fn new(modifiers: &[Modifier]) -> Self {
        let mut target = modifiers.to_vec();
        target.sort();
        target.dedup();
        Self {
            target,
            ..Self::default()
        }
    }

    fn held_modifiers(&self) -> Vec<Modifier> {
        let mut held: Vec<Modifier> = self.held.iter().filter_map(|vk| modifier_of(*vk)).collect();
        held.sort();
        held.dedup();
        held
    }

    /// One key event; `true` when the shortcut fires.
    pub fn key_event(&mut self, vk: u32, up: bool) -> bool {
        if self.target.is_empty() {
            return false;
        }
        let is_modifier = modifier_of(vk).is_some();
        if !up {
            if !is_modifier {
                // Another key with the modifiers is some other shortcut.
                if !self.held.is_empty() {
                    self.spoiled = true;
                }
                return false;
            }
            if !self.held.contains(&vk) {
                if self.held.len() >= MAX_HELD {
                    self.spoiled = true;
                    return false;
                }
                self.held.push(vk);
            }
            let held = self.held_modifiers();
            if held == self.target {
                self.complete = true;
            } else if !held.iter().all(|modifier| self.target.contains(modifier)) {
                self.spoiled = true;
            }
            return false;
        }
        if !is_modifier {
            return false;
        }
        let fire = self.complete && !self.spoiled && self.held.contains(&vk);
        self.held.retain(|held| *held != vk);
        if fire {
            // Once per press of the chord.
            self.spoiled = true;
        }
        if self.held.is_empty() {
            self.complete = false;
            self.spoiled = false;
        }
        fire
    }
}

fn function_key_number(key: &str) -> Option<u8> {
    key.strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .filter(|n| (1..=24).contains(n))
}

fn is_standalone_key(key: &str) -> bool {
    function_key_number(key).is_some()
        || matches!(key, "Pause" | "Insert" | "Scroll_Lock" | "Print" | "Menu")
}

/// Key names a user may bind (the Linux spelling) and their virtual keys.
const NAMED_KEYS: &[(&str, u16)] = &[
    ("space", 0x20),
    ("Insert", 0x2D),
    ("Home", 0x24),
    ("End", 0x23),
    ("Prior", 0x21),
    ("Next", 0x22),
    ("Pause", 0x13),
    ("Scroll_Lock", 0x91),
    ("Print", 0x2C),
    ("Menu", 0x5D),
    ("comma", 0xBC),
    ("period", 0xBE),
    ("slash", 0xBF),
    ("semicolon", 0xBA),
    ("apostrophe", 0xDE),
    ("grave", 0xC0),
    ("minus", 0xBD),
    ("equal", 0xBB),
    ("bracketleft", 0xDB),
    ("bracketright", 0xDD),
    ("backslash", 0xDC),
];

/// Canonical name and virtual key. Punctuation uses the US-layout OEM keys,
/// which is also where the Linux names point.
fn canonical_key(key: &str) -> Option<(String, u16)> {
    if key.len() == 1 {
        let c = key.chars().next()?;
        if c.is_ascii_alphabetic() {
            // VK_A..VK_Z are the uppercase ASCII codes.
            return Some((
                c.to_ascii_lowercase().to_string(),
                u16::from(c.to_ascii_uppercase() as u8),
            ));
        }
        if c.is_ascii_digit() {
            return Some((c.to_string(), u16::from(c as u8)));
        }
        return None;
    }
    if let Some(n) = function_key_number(key) {
        // VK_F1 = 0x70 .. VK_F24 = 0x87.
        return Some((key.to_owned(), 0x6F + u16::from(n)));
    }
    NAMED_KEYS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(name, vk)| ((*name).to_owned(), *vk))
}

/// Keys offered for press and hold: the same ones as on Linux, by name, with
/// the virtual key a low-level hook reports for each.
pub const HOLD_KEYS: [(u16, &str); 8] = [
    (0xA3, "Right Ctrl"),
    (0xA5, "Right Alt"),
    (0x5C, "Right Super"),
    (0x5D, "Menu"),
    (0x91, "Scroll Lock"),
    (0x13, "Pause"),
    (0x7C, "F13"),
    (0x7D, "F14"),
];

/// The hold key with this display name.
#[must_use]
pub fn hold_key(name: &str) -> Option<u16> {
    HOLD_KEYS
        .iter()
        .find(|(_, label)| *label == name)
        .map(|(vk, _)| *vk)
}

/// Press and release of the hold key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldEvent {
    Pressed,
    Released,
}

/// Turns the hook's raw stream for one key into presses and releases:
/// auto-repeat presses while held are dropped, so each press is reported
/// once and always followed by one release.
#[derive(Debug, Default)]
pub struct HoldTracker {
    down: bool,
}

impl HoldTracker {
    pub fn key_event(&mut self, key_up: bool) -> Option<HoldEvent> {
        match (self.down, key_up) {
            (false, false) => {
                self.down = true;
                Some(HoldEvent::Pressed)
            }
            (true, true) => {
                self.down = false;
                Some(HoldEvent::Released)
            }
            _ => None,
        }
    }

    /// The watcher is stopping: a key still down counts as released, so a
    /// session never stays open on a release that will not come.
    pub fn reset(&mut self) -> Option<HoldEvent> {
        std::mem::take(&mut self.down).then_some(HoldEvent::Released)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hotkey(value: &str) -> Hotkey {
        match Hotkey::parse(value) {
            Ok(hotkey) => hotkey,
            Err(error) => unreachable!("{value}: {error}"),
        }
    }

    #[test]
    fn parses_and_maps_common_shortcuts() {
        assert_eq!(hotkey("Super+d").win32(), (MOD_NOREPEAT | MOD_WIN, 0x44));
        assert_eq!(
            hotkey("Ctrl+Alt+space").win32(),
            (MOD_NOREPEAT | MOD_CONTROL | MOD_ALT, 0x20)
        );
        assert_eq!(hotkey("F9").win32(), (MOD_NOREPEAT, 0x78));
        assert_eq!(
            hotkey("Shift+F24").win32(),
            (MOD_NOREPEAT | MOD_SHIFT, 0x87)
        );
        assert_eq!(hotkey("Ctrl+5").win32().1, 0x35);
        assert_eq!(hotkey("Alt+comma").win32().1, 0xBC);
        assert_eq!(hotkey("Pause").win32().1, 0x13);
    }

    #[test]
    fn names_are_canonical_and_round_trip() {
        let parsed = hotkey("Shift+Super+D");
        assert_eq!(parsed.to_string(), "Super+Shift+d");
        assert_eq!(hotkey(&parsed.to_string()), parsed);
        assert_eq!(hotkey("Ctrl+Ctrl+x").modifiers(), [Modifier::Ctrl]);
    }

    #[test]
    fn typing_keys_need_a_strong_modifier() {
        for refused in ["d", "Shift+d", "space", "Shift+comma", "7"] {
            assert_eq!(
                Hotkey::parse(refused).err(),
                Some(ShortcutError::InvalidCombination),
                "{refused}"
            );
        }
    }

    #[test]
    fn unknown_names_are_refused_and_modifiers_alone_are_taken() {
        assert_eq!(
            Hotkey::parse("Hyper+d").err(),
            Some(ShortcutError::InvalidCombination)
        );
        assert_eq!(
            Hotkey::parse("Ctrl+F25").err(),
            Some(ShortcutError::InvalidCombination)
        );
        assert_eq!(
            Hotkey::parse("Ctrl+Escape").err(),
            Some(ShortcutError::InvalidCombination)
        );
        let chord = Hotkey::parse("Super+Ctrl").ok();
        assert_eq!(
            chord.as_ref().map(ToString::to_string).as_deref(),
            Some("Super+Ctrl")
        );
        assert!(chord.is_some_and(|chord| chord.is_modifiers_only()));
        assert!(Hotkey::new(&[Modifier::Super], "").is_ok_and(|key| key.is_modifiers_only()));
        assert!(Hotkey::parse("Ctrl+d").is_ok_and(|key| !key.is_modifiers_only()));
        assert_eq!(
            Hotkey::new(&[], "").err(),
            Some(ShortcutError::InvalidCombination)
        );
    }

    #[test]
    fn hold_keys_match_the_linux_offer() {
        assert_eq!(hold_key("Right Ctrl"), Some(0xA3));
        assert_eq!(hold_key("F14"), Some(0x7D));
        assert_eq!(hold_key("Left Ctrl"), None);
    }

    #[test]
    fn hold_tracker_drops_repeats_and_releases_on_reset() {
        let mut tracker = HoldTracker::default();
        assert_eq!(tracker.key_event(true), None);
        assert_eq!(tracker.key_event(false), Some(HoldEvent::Pressed));
        assert_eq!(tracker.key_event(false), None);
        assert_eq!(tracker.key_event(true), Some(HoldEvent::Released));
        assert_eq!(tracker.key_event(false), Some(HoldEvent::Pressed));
        assert_eq!(tracker.reset(), Some(HoldEvent::Released));
        assert_eq!(tracker.reset(), None);
    }

    #[test]
    fn a_chord_fires_once_on_release_when_nothing_else_was_pressed() {
        const LCTRL: u32 = 0xA2;
        const LWIN: u32 = 0x5B;
        let mut chord = ChordTracker::new(&[Modifier::Ctrl, Modifier::Super]);
        assert!(!chord.key_event(LCTRL, false));
        assert!(!chord.key_event(LWIN, false));
        // Auto-repeat of a held key changes nothing.
        assert!(!chord.key_event(LWIN, false));
        assert!(chord.key_event(LWIN, true));
        assert!(!chord.key_event(LCTRL, true));

        // Ctrl+Super+D is another shortcut: no fire.
        chord.key_event(LCTRL, false);
        chord.key_event(LWIN, false);
        chord.key_event(0x44, false);
        chord.key_event(0x44, true);
        assert!(!chord.key_event(LWIN, true));
        assert!(!chord.key_event(LCTRL, true));

        // An extra modifier spoils it; Ctrl alone never completes it.
        chord.key_event(LCTRL, false);
        chord.key_event(0xA0, false);
        chord.key_event(LWIN, false);
        assert!(!chord.key_event(LWIN, true));
        chord.key_event(0xA0, true);
        assert!(!chord.key_event(LCTRL, true));
        chord.key_event(LCTRL, false);
        assert!(!chord.key_event(LCTRL, true));

        // After everything is up it works again, either hand.
        chord.key_event(0xA3, false);
        chord.key_event(0x5C, false);
        assert!(chord.key_event(0xA3, true));
    }
}
