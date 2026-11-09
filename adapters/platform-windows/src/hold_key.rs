//! Press and hold to dictate on Windows: the same calls as the Linux
//! adapter's `hold_key` (which reads evdev there), on a low-level keyboard
//! hook ([`crate::os::ShortcutListener`] with no toggle shortcuts).
//!
//! Keys are named by their Linux key codes, so the user's choice
//! (`hold.json`) and the list offered are the same on both systems; each
//! is mapped to its Windows virtual key here. The hook sees every key but
//! keeps nothing except the chosen key's press and release, ignores keys
//! Narrivy types itself, and lets every key through to the applications.

#![forbid(unsafe_code)]

pub use crate::shortcuts::HoldEvent;

/// Keys offered for press and hold: ones that do nothing much alone. The
/// Linux adapter's list, by the same Linux key codes.
pub const KEYS: [(u16, &str); 8] = [
    (97, "Right Ctrl"),
    (100, "Right Alt"),
    (126, "Right Super"),
    (127, "Menu"),
    (70, "Scroll Lock"),
    (119, "Pause"),
    (183, "F13"),
    (184, "F14"),
];

/// Each offered key's Windows virtual key. A low-level hook reports the
/// right-hand modifiers apart from the left-hand ones.
const VIRTUAL_KEYS: [(u16, u16); 8] = [
    (97, 0xA3),  // VK_RCONTROL
    (100, 0xA5), // VK_RMENU
    (126, 0x5C), // VK_RWIN
    (127, 0x5D), // VK_APPS
    (70, 0x91),  // VK_SCROLL
    (119, 0x13), // VK_PAUSE
    (183, 0x7C), // VK_F13
    (184, 0x7D), // VK_F14
];

/// The Windows virtual key for the offered key `code`.
#[must_use]
pub fn virtual_key(code: u16) -> Option<u16> {
    VIRTUAL_KEYS
        .iter()
        .find(|(key, _)| *key == code)
        .map(|(_, vk)| *vk)
}

/// A running reader of the chosen key. Dropping it removes the hook; a key
/// still held is then reported released.
#[cfg(windows)]
pub struct HoldKey {
    _listener: crate::os::ShortcutListener,
}

#[cfg(windows)]
impl HoldKey {
    /// Watch the offered key `code`; `on_event` gets each press and release,
    /// on a thread of its own.
    ///
    /// # Errors
    /// `hold_key_unknown` for a key not in [`KEYS`]; `hold_key_unavailable`
    /// when the hook cannot be installed.
    pub fn start(
        code: u16,
        on_event: impl Fn(HoldEvent) + Send + Sync + 'static,
    ) -> Result<Self, &'static str> {
        let vk = virtual_key(code).ok_or("hold_key_unknown")?;
        let (listener, _) = crate::os::ShortcutListener::start(
            Vec::new(),
            Some(vk),
            Box::new(move |event| {
                if let crate::os::ShortcutEvent::Hold(event) = event {
                    on_event(event);
                }
            }),
        )
        .map_err(|_| "hold_key_unavailable")?;
        Ok(Self {
            _listener: listener,
        })
    }

    /// Whether the key is being read: always, once started (the hook sees
    /// every keyboard).
    #[must_use]
    pub fn is_reading(&self) -> bool {
        true
    }

    /// Whether a keyboard could not be read for lack of permission: never
    /// on Windows (a hook needs no permission).
    #[must_use]
    pub fn is_denied(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_key_has_one_virtual_key() {
        for (code, _) in KEYS {
            assert!(virtual_key(code).is_some(), "{code}");
        }
        let mut keys: Vec<u16> = VIRTUAL_KEYS.iter().map(|(_, vk)| *vk).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), KEYS.len());
        assert_eq!(virtual_key(30), None);
    }
}
