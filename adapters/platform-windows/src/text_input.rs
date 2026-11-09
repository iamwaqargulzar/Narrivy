//! The keyboard events that type a text, as `SendInput` will send them.
//!
//! Every character becomes a `KEYEVENTF_UNICODE` press and release of its
//! UTF-16 code unit, so typing does not depend on the keyboard layout. A
//! character outside the Basic Multilingual Plane (most emoji, for example)
//! is two code units, a surrogate pair, sent high then low; the target's
//! message loop receives two `WM_CHAR`s and joins them.
//!
//! Line breaks and tabs are sent as the Enter and Tab keys, because many
//! applications ignore a Unicode carriage return. `\r\n` is one Enter. Any
//! other control character is refused before anything is sent: typing it
//! could trigger a shortcut in the target, and dropping it would change the
//! text. The clipboard route carries such text unchanged.
//!
//! Pure computation; the events never leave this module except through the
//! caller's `SendInput`. Nothing here logs.

#![forbid(unsafe_code)]

/// Virtual-key codes used here (winuser.h).
pub const VK_TAB: u16 = 0x09;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_V: u16 = 0x56;

/// Largest text sent through direct input, as on Linux. Longer text goes to
/// the clipboard route or the recovery Inbox.
pub const MAX_DIRECT_TEXT_BYTES: usize = 64 * 1024;

/// One keyboard event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStroke {
    /// `KEYEVENTF_UNICODE` with this UTF-16 code unit as the scan code.
    Unicode { unit: u16, up: bool },
    /// A virtual key.
    Virtual { vk: u16, up: bool },
}

/// Why a text cannot be typed. Content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputError {
    Empty,
    TooLong,
    UnsupportedControl,
}

impl TextInputError {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Empty => "delivery_text_empty",
            Self::TooLong => "delivery_text_too_long",
            Self::UnsupportedControl => "delivery_text_control_character",
        }
    }
}

fn press(vk: u16, out: &mut Vec<KeyStroke>) {
    out.push(KeyStroke::Virtual { vk, up: false });
    out.push(KeyStroke::Virtual { vk, up: true });
}

/// The events that type `text`, in order.
///
/// # Errors
/// [`TextInputError`] for an empty or oversized text, or one holding a
/// control character other than line breaks and tabs.
pub fn key_strokes(text: &str) -> Result<Vec<KeyStroke>, TextInputError> {
    if text.is_empty() {
        return Err(TextInputError::Empty);
    }
    if text.len() > MAX_DIRECT_TEXT_BYTES {
        return Err(TextInputError::TooLong);
    }
    let mut out = Vec::with_capacity(text.len() * 2);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                press(VK_RETURN, &mut out);
            }
            '\n' => press(VK_RETURN, &mut out),
            '\t' => press(VK_TAB, &mut out),
            c if c.is_control() => return Err(TextInputError::UnsupportedControl),
            c => {
                let mut units = [0_u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push(KeyStroke::Unicode {
                        unit: *unit,
                        up: false,
                    });
                    out.push(KeyStroke::Unicode {
                        unit: *unit,
                        up: true,
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Ctrl+V.
#[must_use]
pub fn paste_chord() -> [KeyStroke; 4] {
    [
        KeyStroke::Virtual {
            vk: VK_CONTROL,
            up: false,
        },
        KeyStroke::Virtual {
            vk: VK_V,
            up: false,
        },
        KeyStroke::Virtual { vk: VK_V, up: true },
        KeyStroke::Virtual {
            vk: VK_CONTROL,
            up: true,
        },
    ]
}

/// Modifier keys that must be up before typing: with one held (the user still
/// releasing the dictation shortcut), a typed letter would be a shortcut.
pub const MODIFIER_KEYS: [u16; 5] = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN];

/// Events per `SendInput` call. Small batches let the target's input queue
/// keep up and let cancellation be observed between batches.
pub const SEND_BATCH: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;

    fn units(strokes: &[KeyStroke]) -> Vec<u16> {
        strokes
            .iter()
            .filter_map(|stroke| match stroke {
                KeyStroke::Unicode { unit, up: false } => Some(*unit),
                _ => None,
            })
            .collect()
    }

    fn ok(text: &str) -> Vec<KeyStroke> {
        match key_strokes(text) {
            Ok(strokes) => strokes,
            Err(error) => unreachable!("{error:?}"),
        }
    }

    #[test]
    fn each_unit_is_pressed_then_released() {
        assert_eq!(
            ok("hé"),
            [
                KeyStroke::Unicode {
                    unit: 0x68,
                    up: false
                },
                KeyStroke::Unicode {
                    unit: 0x68,
                    up: true
                },
                KeyStroke::Unicode {
                    unit: 0xE9,
                    up: false
                },
                KeyStroke::Unicode {
                    unit: 0xE9,
                    up: true
                },
            ]
        );
    }

    #[test]
    fn non_bmp_characters_become_surrogate_pairs() {
        let strokes = ok("a😀");
        assert_eq!(units(&strokes), [0x61, 0xD83D, 0xDE00]);
        assert_eq!(strokes.len(), 6);
    }

    #[test]
    fn typed_units_round_trip_to_the_text() {
        let text = "Grüße, 世界! 𝄞 — ok";
        assert_eq!(
            String::from_utf16(&units(&ok(text))).ok().as_deref(),
            Some(text)
        );
    }

    #[test]
    fn line_breaks_and_tabs_are_keys() {
        let enter = [
            KeyStroke::Virtual {
                vk: VK_RETURN,
                up: false,
            },
            KeyStroke::Virtual {
                vk: VK_RETURN,
                up: true,
            },
        ];
        assert_eq!(ok("\n"), enter);
        assert_eq!(ok("\r\n"), enter);
        assert_eq!(ok("\r"), enter);
        assert_eq!(ok("\n\n").len(), 4);
        assert_eq!(
            ok("\t"),
            [
                KeyStroke::Virtual {
                    vk: VK_TAB,
                    up: false
                },
                KeyStroke::Virtual {
                    vk: VK_TAB,
                    up: true
                },
            ]
        );
    }

    #[test]
    fn other_controls_empty_and_oversized_text_are_refused() {
        assert_eq!(key_strokes(""), Err(TextInputError::Empty));
        assert_eq!(
            key_strokes("a\u{1b}b"),
            Err(TextInputError::UnsupportedControl)
        );
        assert_eq!(
            key_strokes("a\u{7f}"),
            Err(TextInputError::UnsupportedControl)
        );
        let long = "a".repeat(MAX_DIRECT_TEXT_BYTES + 1);
        assert_eq!(key_strokes(&long), Err(TextInputError::TooLong));
    }

    #[test]
    fn paste_chord_releases_what_it_presses() {
        let chord = paste_chord();
        assert_eq!(
            chord[0],
            KeyStroke::Virtual {
                vk: VK_CONTROL,
                up: false
            }
        );
        assert_eq!(
            chord[3],
            KeyStroke::Virtual {
                vk: VK_CONTROL,
                up: true
            }
        );
    }
}
