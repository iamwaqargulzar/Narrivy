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
        .map(sanitized)
        .unwrap_or_default()
}

/// The saved languages as the setters would have stored them: the file is
/// only ours to write, but an edited or older file must not put a bad tag
/// or more than [`MAX_LANGUAGES`] on the user's list.
fn sanitized(mut options: Options) -> Options {
    let mut languages = Vec::new();
    for tag in options.languages.iter().filter_map(|tag| language_tag(tag)) {
        if languages.len() == MAX_LANGUAGES {
            break;
        }
        if !languages.contains(&tag) {
            languages.push(tag);
        }
    }
    options.languages = languages;
    options.language = options.language.as_deref().and_then(language_tag);
    options
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

/// Whether `tag` asks for automatic: `auto` in any case, or nothing.
fn is_automatic(tag: &str) -> bool {
    let tag = tag.trim();
    tag.is_empty() || tag.eq_ignore_ascii_case("auto")
}

/// A recognition tag in its stored form, or `None` when it is not one.
/// `auto` is not a language: it never goes on the list.
fn language_tag(tag: &str) -> Option<String> {
    if is_automatic(tag) {
        return None;
    }
    narrivy_ipc::RecognitionTagDto::parse(tag.trim().to_owned())
        .ok()
        .map(|tag| tag.as_str().to_ascii_lowercase().replace('_', "-"))
}

/// The user's list from `tags`: each one in its stored form, without
/// repeats, at most [`MAX_LANGUAGES`]. Stops at the first bad tag or the
/// first one past the limit, so a long request costs no more than that.
fn languages_from(tags: &[String]) -> Result<Vec<String>, &'static str> {
    let mut languages: Vec<String> = Vec::new();
    for tag in tags {
        let tag = language_tag(tag).ok_or("dictation_language_invalid")?;
        if !languages.contains(&tag) {
            if languages.len() == MAX_LANGUAGES {
                return Err("dictation_languages_full");
            }
            languages.push(tag);
        }
    }
    Ok(languages)
}

/// Choose the dictation language: `auto` (or empty) for automatic, else a
/// language tag, which joins the user's list when it is not on it.
///
/// # Errors
/// `dictation_language_invalid`, `dictation_languages_full`, or
/// `text_options_unavailable`.
pub fn set_dictation_language(tag: &str) -> Result<(), &'static str> {
    let mut options = read();
    if is_automatic(tag) {
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
    let languages = languages_from(tags)?;
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
    use super::{
        DeliveryMethod, MAX_LANGUAGES, Options, is_automatic, language_tag, languages_from,
        paste_for, sanitized,
    };
    use proptest::prelude::*;

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

    /// A stored tag is short plain ASCII: lowercase letters, digits and
    /// single hyphens between non-empty subtags.
    fn is_stored_form(tag: &str) -> bool {
        !tag.is_empty()
            && tag.len() <= 32
            && tag != "auto"
            && tag.split('-').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
    }

    #[test]
    fn odd_tags_are_refused_or_normalised() {
        assert_eq!(language_tag(" EN_us ").as_deref(), Some("en-us"));
        assert_eq!(language_tag("\u{2003}fr\t").as_deref(), Some("fr"));
        for bad in [
            "auto",
            "AUTO",
            " Auto ",
            "",
            "en--us",
            "-en",
            "en-",
            "en\u{0}",
            "en\u{1b}[31m",
            "e\nn",
            "ｅｎ",
            "en\u{200b}",
            "\u{130}",
            "en us",
            "en/../us",
            &"a".repeat(33),
            &"ab-".repeat(200),
        ] {
            assert_eq!(language_tag(bad), None, "{bad:?}");
        }
        assert!(is_automatic("AUTO") && is_automatic(" ") && !is_automatic("en"));
    }

    #[test]
    fn the_list_is_bounded() {
        let many: Vec<String> = (0..10_000).map(|n| format!("x{n}")).collect();
        assert_eq!(languages_from(&many), Err("dictation_languages_full"));
        let exactly: Vec<String> = many[..MAX_LANGUAGES].to_vec();
        assert_eq!(languages_from(&exactly).map(|l| l.len()), Ok(MAX_LANGUAGES));
        // Repeats (in any spelling) count once.
        let repeats: Vec<String> = ["en", "EN", "en_US", "en-us", " en "]
            .iter()
            .cycle()
            .take(1_000)
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(
            languages_from(&repeats),
            Ok(vec!["en".into(), "en-us".into()])
        );

        // A hand-edited file is cut back to the limit and cleaned.
        let mut languages = many.clone();
        languages.insert(0, "bad tag".into());
        let options = sanitized(Options {
            languages,
            language: Some("\u{1b}".into()),
            ..Options::default()
        });
        assert_eq!(options.languages.len(), MAX_LANGUAGES);
        assert_eq!(options.language, None);
    }

    /// `cases` per property, unless `PROPTEST_CASES` asks for more (see
    /// `scripts/fuzz-untrusted-input.sh`). No regression files are written.
    fn fuzz_config(cases: u32) -> ProptestConfig {
        let mut config = ProptestConfig {
            failure_persistence: None,
            ..ProptestConfig::default()
        };
        if std::env::var_os("PROPTEST_CASES").is_none() {
            config.cases = cases;
        }
        config
    }

    proptest! {
        #![proptest_config(fuzz_config(512))]

        /// Any string at all: refused, or stored in the plain form.
        #[test]
        fn any_tag_is_refused_or_stored_plain(tag in any::<String>()) {
            if let Some(stored) = language_tag(&tag) {
                prop_assert!(is_stored_form(&stored), "{stored:?}");
                // Storing is idempotent.
                prop_assert_eq!(language_tag(&stored), Some(stored));
            }
        }

        /// Tag-shaped strings with odd Unicode and control characters mixed in.
        #[test]
        fn tag_shaped_strings_are_refused_or_stored_plain(
            tag in r"[a-zA-Z0-9_\- \t\x00-\x1f\x7f\u{80}-\u{10ffff}]{0,40}",
        ) {
            if let Some(stored) = language_tag(&tag) {
                prop_assert!(is_stored_form(&stored), "{stored:?}");
            }
        }

        /// Any list: refused, or at most 16 distinct stored tags.
        #[test]
        fn any_list_is_refused_or_bounded(tags in prop::collection::vec("[a-zA-Z_ -]{0,6}|.{0,6}", 0..64)) {
            if let Ok(languages) = languages_from(&tags) {
                prop_assert!(languages.len() <= MAX_LANGUAGES);
                for (i, tag) in languages.iter().enumerate() {
                    prop_assert!(is_stored_form(tag));
                    prop_assert!(!languages[..i].contains(tag));
                }
            }
        }

        /// Whatever a file holds, what is read is bounded and plain.
        #[test]
        fn a_saved_file_is_read_back_bounded(
            languages in prop::collection::vec(".{0,8}|[a-z]{2}", 0..64),
            language in proptest::option::of(".{0,8}|[a-z]{2}"),
        ) {
            let options = sanitized(Options {
                languages,
                language,
                ..Options::default()
            });
            prop_assert!(options.languages.len() <= MAX_LANGUAGES);
            prop_assert!(options.languages.iter().all(|tag| is_stored_form(tag)));
            prop_assert!(options.language.iter().all(|tag| is_stored_form(tag)));
        }
    }
}
