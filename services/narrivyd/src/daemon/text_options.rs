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
//! - **Rules**: the deterministic corrector's rules a Clean pipeline applies
//!   (`narrivyd::correct::Rules`); all on unless turned off.
//! - **Space after**: a space after each dictation's text, so the next one
//!   does not run on; off unless turned on. Every pipeline.
//! - **Clean-up level**: how far the clean-up model may change the text:
//!   `off` (no model), `faithful` (the default), `polish` or `rewrite`.
//! - **Clean-up device**: where the clean-up model runs: `auto`, `npu`, `gpu`
//!   or `cpu`; until chosen, `NARRIVY_REFINE_COMPUTE` or `auto`.
//!
//! Every pipeline has its own values (D50): the fields above are the
//! starting values, and `pipelines` holds what each pipeline changed. A
//! dictation reads its own pipeline's values ([`super::user_pipelines::active`]);
//! a change names its pipeline, or applies to the active one. Only the list
//! of languages to choose from is one for all.
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
    /// Corrector rules turned off: `spacing`, `punctuation`, `capitals`,
    /// `ending`.
    #[serde(default)]
    rules_off: Vec<String>,
    #[serde(default)]
    space_after: bool,
    /// `None` is `faithful`.
    #[serde(default)]
    cleanup_level: Option<CleanupLevel>,
    /// `None` follows `NARRIVY_REFINE_COMPUTE`, else `auto`.
    #[serde(default)]
    cleanup_compute: Option<String>,
    /// Each pipeline's own values, where it changed them (D50).
    #[serde(default)]
    pipelines: std::collections::BTreeMap<String, PipelineText>,
}

/// One pipeline's own text options; `None` keeps the starting value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PipelineText {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    numbers_as_digits: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery: Option<DeliveryMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup_idle_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spoken_breaks: Option<bool>,
    /// The language tag, or [`AUTOMATIC`] for automatic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rules_off: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    space_after: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup_level: Option<CleanupLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup_compute: Option<String>,
}

/// A pipeline's language choice meaning automatic.
const AUTOMATIC: &str = "auto";

impl Options {
    /// These options as `pipeline` sees them: its own values over the
    /// starting ones.
    fn for_pipeline(mut self, pipeline: &str) -> Self {
        let Some(own) = self.pipelines.get(pipeline).cloned() else {
            return self;
        };
        if let Some(v) = own.numbers_as_digits {
            self.numbers_as_digits = v;
        }
        if own.delivery.is_some() {
            self.delivery = own.delivery;
        }
        if let Some(v) = own.cleanup_idle_minutes {
            self.cleanup_idle_minutes = v;
        }
        if let Some(v) = own.spoken_breaks {
            self.spoken_breaks = v;
        }
        if let Some(v) = own.language {
            self.language = (v != AUTOMATIC)
                .then_some(v)
                .as_deref()
                .and_then(language_tag);
        }
        if let Some(v) = own.rules_off {
            self.rules_off = v;
        }
        if let Some(v) = own.space_after {
            self.space_after = v;
        }
        if own.cleanup_level.is_some() {
            self.cleanup_level = own.cleanup_level;
        }
        if own.cleanup_compute.is_some() {
            self.cleanup_compute = own.cleanup_compute;
        }
        self
    }
}

/// The corrector rules, as clients name them.
pub const RULES: [&str; 4] = ["spacing", "punctuation", "capitals", "ending"];

/// How far the clean-up model may change a dictation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupLevel {
    /// The model is not used; the rules alone.
    Off,
    /// Fixes, keeps the user's words.
    Faithful,
    /// Smoother sentences.
    Polish,
    /// May change tone or format.
    Rewrite,
}

impl CleanupLevel {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Faithful => "faithful",
            Self::Polish => "polish",
            Self::Rewrite => "rewrite",
        }
    }

    fn parse(id: &str) -> Option<Self> {
        match id {
            "off" => Some(Self::Off),
            "faithful" => Some(Self::Faithful),
            "polish" => Some(Self::Polish),
            "rewrite" => Some(Self::Rewrite),
            _ => None,
        }
    }
}

/// Where the clean-up model may run, as clients name it.
pub const CLEANUP_DEVICES: [&str; 4] = ["auto", "npu", "gpu", "cpu"];

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
            rules_off: Vec::new(),
            space_after: false,
            cleanup_level: None,
            cleanup_compute: None,
            pipelines: std::collections::BTreeMap::new(),
        }
    }
}

/// The options the active pipeline sees.
fn read() -> Options {
    read_for(None)
}

/// The options `pipeline` sees (`None`: the active pipeline).
fn read_for(pipeline: Option<&str>) -> Options {
    let id = pipeline.map_or_else(super::user_pipelines::active, str::to_owned);
    stored().for_pipeline(&id)
}

/// Whether `id` can name a pipeline: short plain ASCII.
fn valid_pipeline(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// Change `pipeline`'s own values (`None`: the active pipeline's).
fn update(
    pipeline: Option<&str>,
    change: impl FnOnce(&mut PipelineText),
) -> Result<(), &'static str> {
    let id = pipeline.map_or_else(super::user_pipelines::active, str::to_owned);
    if !valid_pipeline(&id) {
        return Err("text_pipeline_invalid");
    }
    let mut options = stored();
    change(options.pipelines.entry(id).or_default());
    write(&options)
}

/// A deleted pipeline's own values go with it.
pub fn forget_pipeline(pipeline: &str) {
    let mut options = stored();
    if options.pipelines.remove(pipeline).is_some() {
        let _ = write(&options);
    }
}

/// The file as saved: the starting values and each pipeline's own.
fn stored() -> Options {
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
pub fn set_cleanup_idle_minutes(pipeline: Option<&str>, minutes: u32) -> Result<(), &'static str> {
    if minutes > 24 * 60 {
        return Err("cleanup_idle_invalid");
    }
    update(pipeline, |own| own.cleanup_idle_minutes = Some(minutes))
}

/// # Errors
/// `text_options_unavailable` when it cannot be saved.
pub fn set_spoken_breaks(pipeline: Option<&str>, on: bool) -> Result<(), &'static str> {
    update(pipeline, |own| own.spoken_breaks = Some(on))
}

/// The corrector rules that are on.
#[must_use]
pub fn rules() -> narrivyd::correct::Rules {
    rules_from(&read().rules_off)
}

fn rules_from(off: &[String]) -> narrivyd::correct::Rules {
    let on = |rule: &str| !off.iter().any(|o| o == rule);
    narrivyd::correct::Rules {
        spacing: on("spacing"),
        punctuation: on("punctuation"),
        capitals: on("capitals"),
        ending: on("ending"),
    }
}

/// Turn one corrector rule on or off.
///
/// # Errors
/// `refine_rule_unknown`, or `text_options_unavailable`.
pub fn set_rule(pipeline: Option<&str>, rule: &str, on: bool) -> Result<(), &'static str> {
    if !RULES.contains(&rule) {
        return Err("refine_rule_unknown");
    }
    let mut off = read_for(pipeline).rules_off;
    off.retain(|o| o != rule);
    if !on {
        off.push(rule.to_owned());
    }
    update(pipeline, |own| own.rules_off = Some(off))
}

/// # Errors
/// `text_options_unavailable` when it cannot be saved.
pub fn set_space_after(pipeline: Option<&str>, on: bool) -> Result<(), &'static str> {
    update(pipeline, |own| own.space_after = Some(on))
}

/// The text a dictation delivers: `text`, with a space after it when the
/// user chose that and it does not already end in one.
#[must_use]
pub fn delivered(text: String) -> String {
    if !text.trim().is_empty() && !text.ends_with(char::is_whitespace) && read().space_after {
        text + " "
    } else {
        text
    }
}

#[must_use]
pub fn cleanup_level() -> CleanupLevel {
    read().cleanup_level.unwrap_or(CleanupLevel::Faithful)
}

/// # Errors
/// `cleanup_level_unknown`, or `text_options_unavailable`.
pub fn set_cleanup_level(pipeline: Option<&str>, level: &str) -> Result<(), &'static str> {
    let level = CleanupLevel::parse(level).ok_or("cleanup_level_unknown")?;
    update(pipeline, |own| own.cleanup_level = Some(level))
}

/// Where the clean-up model runs: the user's choice, else
/// `NARRIVY_REFINE_COMPUTE` when it names a device, else `auto`.
#[must_use]
pub fn cleanup_compute() -> String {
    read()
        .cleanup_compute
        .or_else(|| std::env::var(super::refine::COMPUTE_ENV).ok())
        .filter(|device| CLEANUP_DEVICES.contains(&device.as_str()))
        .unwrap_or_else(|| "auto".to_owned())
}

/// # Errors
/// `cleanup_device_unknown`, or `text_options_unavailable`.
pub fn set_cleanup_compute(pipeline: Option<&str>, device: &str) -> Result<(), &'static str> {
    if !CLEANUP_DEVICES.contains(&device) {
        return Err("cleanup_device_unknown");
    }
    update(pipeline, |own| {
        own.cleanup_compute = Some(device.to_owned())
    })
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
pub fn set_dictation_language(pipeline: Option<&str>, tag: &str) -> Result<(), &'static str> {
    let chosen = if is_automatic(tag) {
        AUTOMATIC.to_owned()
    } else {
        let tag = language_tag(tag).ok_or("dictation_language_invalid")?;
        let mut options = stored();
        if !options.languages.contains(&tag) {
            if options.languages.len() >= MAX_LANGUAGES {
                return Err("dictation_languages_full");
            }
            options.languages.push(tag.clone());
            write(&options)?;
        }
        tag
    };
    update(pipeline, |own| own.language = Some(chosen))
}

/// Replace the user's list of languages. A current language taken off the
/// list goes back to automatic.
///
/// # Errors
/// `dictation_language_invalid`, `dictation_languages_full`, or
/// `text_options_unavailable`.
pub fn set_dictation_languages(tags: &[String]) -> Result<(), &'static str> {
    let languages = languages_from(tags)?;
    let mut options = stored();
    let off_list = |current: &Option<String>| {
        current
            .as_ref()
            .is_some_and(|tag| tag != AUTOMATIC && !languages.contains(tag))
    };
    if off_list(&options.language) {
        options.language = None;
    }
    for own in options.pipelines.values_mut() {
        if off_list(&own.language) {
            own.language = Some(AUTOMATIC.to_owned());
        }
    }
    options.languages = languages;
    write(&options)
}

/// Choose `paste`, `paste-keyboard` or `type`.
///
/// # Errors
/// `delivery_method_unknown`, or `text_options_unavailable`.
pub fn set_delivery(pipeline: Option<&str>, method: &str) -> Result<(), &'static str> {
    let method = match method {
        "paste" => DeliveryMethod::Paste,
        "paste-keyboard" => DeliveryMethod::PasteUinput,
        "type" => DeliveryMethod::Type,
        _ => return Err("delivery_method_unknown"),
    };
    update(pipeline, |own| own.delivery = Some(method))
}

const fn yes() -> bool {
    true
}

#[cfg(not(test))]
fn path() -> Option<std::path::PathBuf> {
    super::host::state_dir()
        .ok()
        .map(|dir| dir.join("text.json"))
}

/// Tests never read or write the user's own choices.
#[cfg(test)]
fn path() -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("narrivy-text-options-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("text.json"))
}

#[must_use]
pub fn numbers_as_digits() -> bool {
    read().numbers_as_digits
}

/// # Errors
/// `text_options_unavailable` when the choice cannot be saved.
pub fn set_numbers_as_digits(pipeline: Option<&str>, on: bool) -> Result<(), &'static str> {
    update(pipeline, |own| own.numbers_as_digits = Some(on))
}

/// `composed`: the daemon's own clipboard authorization, for the method in
/// use until the user chooses.
#[must_use]
pub fn dto(composed: bool, pipeline: Option<&str>) -> narrivy_ipc::TextOptionsDto {
    let options = read_for(pipeline);
    let rules = rules_from(&options.rules_off);
    let compute = options
        .cleanup_compute
        .clone()
        .or_else(|| std::env::var(super::refine::COMPUTE_ENV).ok())
        .filter(|device| CLEANUP_DEVICES.contains(&device.as_str()))
        .unwrap_or_else(|| "auto".to_owned());
    narrivy_ipc::TextOptionsDto {
        numbers_as_digits: options.numbers_as_digits,
        delivery_method: options
            .delivery
            .unwrap_or(if composed {
                DeliveryMethod::Paste
            } else {
                DeliveryMethod::Type
            })
            .id()
            .to_owned(),
        cleanup_idle_minutes: options.cleanup_idle_minutes,
        spoken_breaks: options.spoken_breaks,
        dictation_language: options.language,
        dictation_languages: options.languages,
        refine_rules: Some(narrivy_ipc::RefineRulesDto {
            spacing: rules.spacing,
            punctuation: rules.punctuation,
            capitals: rules.capitals,
            ending: rules.ending,
        }),
        space_after: Some(options.space_after),
        cleanup_level: Some(
            options
                .cleanup_level
                .unwrap_or(CleanupLevel::Faithful)
                .id()
                .to_owned(),
        ),
        cleanup_compute: Some(compute),
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
        paste_for, read_for, sanitized,
    };

    /// D50: what one pipeline changes, another does not see; a pipeline that
    /// changed nothing keeps the starting values.
    #[test]
    #[allow(clippy::expect_used)]
    fn each_pipeline_keeps_its_own_options() {
        let a = Some("d50-test-a");
        let b = Some("d50-test-b");
        let untouched = Some("d50-test-c");
        super::set_numbers_as_digits(a, false).expect("saved");
        super::set_numbers_as_digits(b, true).expect("saved");
        super::set_rule(a, "capitals", false).expect("saved");
        super::set_delivery(b, "type").expect("saved");
        super::set_dictation_language(a, "ur").expect("saved");
        super::set_space_after(b, true).expect("saved");
        assert!(!read_for(a).numbers_as_digits);
        assert!(read_for(b).numbers_as_digits);
        assert!(read_for(a).rules_off.contains(&"capitals".to_owned()));
        assert!(read_for(b).rules_off.is_empty());
        assert_eq!(read_for(b).delivery, Some(DeliveryMethod::Type));
        assert_eq!(read_for(a).language.as_deref(), Some("ur"));
        assert_eq!(read_for(b).language, None);
        assert!(read_for(b).space_after);
        assert!(!read_for(a).space_after);
        // Automatic is a choice of its own, not "keep the starting value".
        super::set_dictation_language(a, "auto").expect("saved");
        assert_eq!(read_for(a).language, None);
        let start = super::stored();
        let c = read_for(untouched);
        assert_eq!(c.numbers_as_digits, start.numbers_as_digits);
        assert_eq!(c.delivery, start.delivery);
        super::forget_pipeline("d50-test-a");
        assert!(!super::stored().pipelines.contains_key("d50-test-a"));
        assert!(super::set_space_after(Some("bad id/"), true).is_err());
    }
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
