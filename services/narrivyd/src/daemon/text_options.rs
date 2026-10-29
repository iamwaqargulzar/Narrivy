//! Text options: how the Clean pipeline writes its final text.
//!
//! - **Numbers as digits** ("twenty five" as 25, see `narrivyd::numbers`): on
//!   unless turned off. The Raw pipeline is never changed.
//! - **Delivery method**: `paste` (the clipboard and Shift+Insert, then the
//!   user's clipboard put back), `paste-keyboard` (the same, with Shift+Insert
//!   pressed on Narrivy's own `/dev/uinput` keyboard, for compositors that
//!   drop the modifiers `wtype` sends; opt-in only, never a default) or
//!   `type` (the virtual keyboard). The user's choice is the clipboard
//!   authorization (D37): choosing a paste method authorizes the clipboard,
//!   choosing `type` withdraws it. Until the user chooses, the daemon's
//!   composition decides (`NARRIVY_CLIPBOARD_AUTHORIZED`; the package leaves it
//!   unset, so it types). Either way, a dictation that cannot be delivered is
//!   kept in the Inbox.
//!
//! - **Dictation language**: the language a start without one of its own
//!   (the plain toggle, the tray's Start, press and hold) is dictated in;
//!   `None` is automatic. The tray and the Pipelines page choose it from the
//!   user's short list of languages. A language shortcut carries its own.
//!
//! Kept in `text.json` in the state directory.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Options {
    #[serde(default = "yes")]
    numbers_as_digits: bool,
    /// `paste`, `paste-keyboard` or `type`; `None` until the user chooses.
    #[serde(default)]
    delivery: Option<DeliveryMethod>,
    /// Unload the clean-up model after this many idle minutes; 0 never.
    #[serde(default)]
    cleanup_idle_minutes: u32,
    /// "new line" / "new paragraph" become line breaks (Clean only).
    #[serde(default)]
    spoken_breaks: bool,
    /// The current dictation language tag; `None` is automatic.
    #[serde(default)]
    language: Option<String>,
    /// The user's languages to choose from, in their order.
    #[serde(default)]
    languages: Vec<String>,
}

/// The user's list of languages holds at most this many.
pub const MAX_LANGUAGES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeliveryMethod {
    Paste,
    #[serde(rename = "paste-keyboard")]
    PasteUinput,
    Type,
}

impl DeliveryMethod {
    const fn id(self) -> &'static str {
        match self {
            Self::Paste => "paste",
            Self::PasteUinput => "paste-keyboard",
            Self::Type => "type",
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            numbers_as_digits: true,
            delivery: None,
            cleanup_idle_minutes: 0,
            spoken_breaks: false,
            language: None,
            languages: Vec::new(),
        }
    }
}

fn read() -> Options {
    path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Options>(&bytes).ok())
        .unwrap_or_default()
}

fn write(options: &Options) -> Result<(), &'static str> {
    let path = path().ok_or("text_options_unavailable")?;
    let bytes = serde_json::to_vec(options).map_err(|_| "text_options_unavailable")?;
    std::fs::write(path, bytes).map_err(|_| "text_options_unavailable")
}

/// Whether to paste: the user's choice, else `composed`.
#[must_use]
pub fn paste(composed: bool) -> bool {
    paste_for(read().delivery, composed)
}

fn paste_for(chosen: Option<DeliveryMethod>, composed: bool) -> bool {
    chosen.map_or(composed, |method| method != DeliveryMethod::Type)
}

/// Whether the user chose to send the paste chord through uinput.
#[must_use]
pub fn paste_through_uinput() -> bool {
    read().delivery == Some(DeliveryMethod::PasteUinput)
}

/// Minutes of no use after which the clean-up model is unloaded; 0 never.
#[must_use]
pub fn cleanup_idle_minutes() -> u32 {
    read().cleanup_idle_minutes
}

/// # Errors
/// `cleanup_idle_invalid` past a day, or `text_options_unavailable`.
pub fn set_cleanup_idle_minutes(minutes: u32) -> Result<(), &'static str> {
    if minutes > 24 * 60 {
        return Err("cleanup_idle_invalid");
    }
    let mut options = read();
    options.cleanup_idle_minutes = minutes;
    write(&options)
}

/// # Errors
/// `text_options_unavailable` when it cannot be saved.
pub fn set_spoken_breaks(on: bool) -> Result<(), &'static str> {
    let mut options = read();
    options.spoken_breaks = on;
    write(&options)
}

/// The current dictation language; `None` is automatic.
#[must_use]
pub fn dictation_language() -> Option<String> {
    read().language
}

/// A recognition tag in its stored form, or `None` when it is not one.
fn language_tag(tag: &str) -> Option<String> {
    narrivy_ipc::RecognitionTagDto::parse(tag.trim().to_owned())
        .ok()
        .map(|tag| tag.as_str().to_ascii_lowercase().replace('_', "-"))
}

/// Choose the dictation language: `auto` (or empty) for automatic, else a
/// language tag, which joins the user's list when it is not on it.
///
/// # Errors
/// `dictation_language_invalid`, `dictation_languages_full`, or
/// `text_options_unavailable`.
pub fn set_dictation_language(tag: &str) -> Result<(), &'static str> {
    let mut options = read();
    if tag.trim().is_empty() || tag.trim() == "auto" {
        options.language = None;
    } else {
        let tag = language_tag(tag).ok_or("dictation_language_invalid")?;
        if !options.languages.contains(&tag) {
            if options.languages.len() >= MAX_LANGUAGES {
                return Err("dictation_languages_full");
            }
            options.languages.push(tag.clone());
        }
        options.language = Some(tag);
    }
    write(&options)
}

/// Replace the user's list of languages. A current language taken off the
/// list goes back to automatic.
///
/// # Errors
/// `dictation_language_invalid`, `dictation_languages_full`, or
/// `text_options_unavailable`.
pub fn set_dictation_languages(tags: &[String]) -> Result<(), &'static str> {
    let mut languages: Vec<String> = Vec::new();
    for tag in tags {
        let tag = language_tag(tag).ok_or("dictation_language_invalid")?;
        if !languages.contains(&tag) {
            languages.push(tag);
        }
    }
    if languages.len() > MAX_LANGUAGES {
        return Err("dictation_languages_full");
    }
    let mut options = read();
    if options
        .language
        .as_ref()
        .is_some_and(|current| !languages.contains(current))
    {
        options.language = None;
    }
    options.languages = languages;
    write(&options)
}

/// Choose `paste`, `paste-keyboard` or `type`.
///
/// # Errors
/// `delivery_method_unknown`, or `text_options_unavailable`.
pub fn set_delivery(method: &str) -> Result<(), &'static str> {
    let method = match method {
        "paste" => DeliveryMethod::Paste,
        "paste-keyboard" => DeliveryMethod::PasteUinput,
        "type" => DeliveryMethod::Type,
        _ => return Err("delivery_method_unknown"),
    };
    let mut options = read();
    options.delivery = Some(method);
    write(&options)
}

const fn yes() -> bool {
    true
}

fn path() -> Option<std::path::PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("text.json"))
}

#[must_use]
pub fn numbers_as_digits() -> bool {
    read().numbers_as_digits
}

/// # Errors
/// `text_options_unavailable` when the choice cannot be saved.
pub fn set_numbers_as_digits(on: bool) -> Result<(), &'static str> {
    let mut options = read();
    options.numbers_as_digits = on;
    write(&options)
}

/// `composed`: the daemon's own clipboard authorization, for the method in
/// use until the user chooses.
#[must_use]
pub fn dto(composed: bool) -> narrivy_ipc::TextOptionsDto {
    narrivy_ipc::TextOptionsDto {
        numbers_as_digits: numbers_as_digits(),
        delivery_method: read()
            .delivery
            .unwrap_or(if composed {
                DeliveryMethod::Paste
            } else {
                DeliveryMethod::Type
            })
            .id()
            .to_owned(),
        cleanup_idle_minutes: cleanup_idle_minutes(),
        spoken_breaks: read().spoken_breaks,
        dictation_language: read().language,
        dictation_languages: read().languages,
    }
}

/// The Clean pipeline's final step.
#[must_use]
pub fn finish_clean(text: String) -> String {
    let options = read();
    let text = if options.numbers_as_digits {
        narrivyd::numbers::digits(&text)
    } else {
        text
    };
    if options.spoken_breaks {
        narrivyd::breaks::line_breaks(&text)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::{DeliveryMethod, paste_for};

    /// The package composes without clipboard authorization: it types until
    /// the user picks a paste method, and `type` withdraws a composed one.
    #[test]
    fn the_users_method_is_the_clipboard_authorization() {
        assert!(!paste_for(None, false));
        assert!(paste_for(None, true));
        for composed in [false, true] {
            assert!(paste_for(Some(DeliveryMethod::Paste), composed));
            assert!(paste_for(Some(DeliveryMethod::PasteUinput), composed));
            assert!(!paste_for(Some(DeliveryMethod::Type), composed));
        }
    }
}
