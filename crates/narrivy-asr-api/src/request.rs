//! Capability-driven recognition request (FUN-028 / FUN-029).
//!
//! Spoken language, written/output language and script, translation target,
//! locale/formatting, and context/bias hints are distinct typed fields.
//! Engines must validate a request against [`AsrCapabilities`] before audio is
//! handed to a backend and must not guess or silently ignore a requested
//! setting. Default requests stay valid for simple callers.

use std::fmt;

use crate::AsrError;

/// Maximum UTF-8 bytes in one language, script, or locale tag.
pub const MAX_TAG_BYTES: usize = 32;
/// Maximum attested language / translation-target / script / locale tags.
/// Multilingual models attest about a hundred languages; a smaller bound
/// would refuse to load them.
pub const MAX_ATTESTED_TAGS: usize = 128;
/// Maximum context/bias hints on one request.
pub const MAX_CONTEXT_HINTS: usize = 8;
/// Maximum UTF-8 bytes in one context/bias hint.
pub const MAX_CONTEXT_HINT_BYTES: usize = 128;

/// Typed, content-free rejection codes. Messages never include transcript
/// text, audio, or context/bias content.
pub const UNSUPPORTED_LANGUAGE_DETECT: &str = "unsupported_language_detect";
pub const UNSUPPORTED_TRANSLATION: &str = "unsupported_translation";
pub const UNSUPPORTED_SPOKEN_LANGUAGE: &str = "unsupported_spoken_language";
pub const UNSUPPORTED_OUTPUT_LANGUAGE: &str = "unsupported_output_language";
pub const UNSUPPORTED_SCRIPT: &str = "unsupported_script";
pub const UNSUPPORTED_LOCALE: &str = "unsupported_locale";
pub const UNSUPPORTED_CONTEXT_BIAS: &str = "unsupported_context_bias";
pub const CONFLICTING_LANGUAGE_SETTINGS: &str = "conflicting_language_settings";
pub const INVALID_LANGUAGE_TAG: &str = "invalid_language_tag";
pub const INVALID_CONTEXT_BIAS: &str = "invalid_context_bias";
pub const INVALID_CAPABILITIES: &str = "invalid_capabilities";
pub const REQUEST_FIELD_TOO_LONG: &str = "request_field_too_long";
pub const TOO_MANY_CONTEXT_HINTS: &str = "too_many_context_hints";
pub const UNSUPPORTED_REQUEST_APPLY: &str = "unsupported_request_apply";

/// Bounded BCP-47-ish language, script, or locale tag.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AsrTag(String);

impl AsrTag {
    /// Parse a caller-supplied tag. Rejects empty, over-long, or malformed
    /// values without echoing the input into the error.
    pub fn parse(raw: &str) -> Result<Self, AsrError> {
        if raw.len() > MAX_TAG_BYTES {
            return Err(AsrError::new(
                REQUEST_FIELD_TOO_LONG,
                "language, script, or locale tag exceeds its bound",
            ));
        }
        if !is_valid_tag(raw) {
            return Err(AsrError::new(
                INVALID_LANGUAGE_TAG,
                "language, script, or locale tag is not a bounded BCP-47-ish value",
            ));
        }
        Ok(Self(raw.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn primary(&self) -> &str {
        self.0.split(['-', '_']).next().unwrap_or(&self.0)
    }

    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.0 == other.0 || self.primary() == other.primary()
    }
}

impl fmt::Debug for AsrTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Display for AsrTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Spoken-language selection. `Unspecified` is the engine default and is not
/// a request for automatic language detection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SpokenLanguage {
    #[default]
    Unspecified,
    AutoDetect,
    Explicit(AsrTag),
}

/// Written/output language and script preference, independent of spoken
/// language and of translation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WrittenOutput {
    pub language: Option<AsrTag>,
    pub script: Option<AsrTag>,
}

/// Bounded context/bias hints. Debug omits hint text so ordinary diagnostics
/// cannot leak prompt content.
#[derive(Clone, PartialEq, Eq)]
pub struct ContextBias {
    hints: Vec<String>,
}

impl ContextBias {
    pub fn try_new(hints: impl IntoIterator<Item = impl Into<String>>) -> Result<Self, AsrError> {
        let mut bounded = Vec::new();
        for hint in hints {
            let hint = hint.into();
            if hint.is_empty() {
                return Err(AsrError::new(
                    INVALID_CONTEXT_BIAS,
                    "context bias hint is empty",
                ));
            }
            if hint.len() > MAX_CONTEXT_HINT_BYTES {
                return Err(AsrError::new(
                    REQUEST_FIELD_TOO_LONG,
                    "context bias hint exceeds its bound",
                ));
            }
            if hint.as_bytes().contains(&0) {
                return Err(AsrError::new(
                    INVALID_CONTEXT_BIAS,
                    "context bias hint contains a NUL byte",
                ));
            }
            bounded.push(hint);
            if bounded.len() > MAX_CONTEXT_HINTS {
                return Err(AsrError::new(
                    TOO_MANY_CONTEXT_HINTS,
                    "too many context bias hints",
                ));
            }
        }
        if bounded.is_empty() {
            return Err(AsrError::new(
                INVALID_CONTEXT_BIAS,
                "context bias request has no hints",
            ));
        }
        Ok(Self { hints: bounded })
    }

    #[must_use]
    pub fn hints(&self) -> &[String] {
        &self.hints
    }

    #[must_use]
    pub fn hint_count(&self) -> usize {
        self.hints.len()
    }
}

impl fmt::Debug for ContextBias {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextBias")
            .field("hint_count", &self.hints.len())
            .finish()
    }
}

/// Recognition/session request. [`AsrRequest::default`] is the simple-caller
/// path and does not request auto-detect, translation, script, locale, or
/// context bias.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AsrRequest {
    pub spoken_language: SpokenLanguage,
    pub output: WrittenOutput,
    pub translation_target: Option<AsrTag>,
    pub locale: Option<AsrTag>,
    pub context_bias: Option<ContextBias>,
}

impl AsrRequest {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// True when this request can be satisfied by the engine-default decode
    /// path: either the request is empty, or it only names the sole attested
    /// spoken language.
    #[must_use]
    pub fn applies_as_engine_default(&self, capabilities: &AsrCapabilities) -> bool {
        if self.is_default() {
            return true;
        }
        if self.translation_target.is_some()
            || self.output.language.is_some()
            || self.output.script.is_some()
            || self.locale.is_some()
            || self.context_bias.is_some()
            || matches!(self.spoken_language, SpokenLanguage::AutoDetect)
        {
            return false;
        }
        let SpokenLanguage::Explicit(tag) = &self.spoken_language else {
            return false;
        };
        capabilities.languages.len() == 1 && capabilities.supports_spoken(tag)
    }
}

/// Engine-attested recognition capabilities. Empty language lists are an
/// information gap, not a claim that every language is supported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AsrCapabilities {
    pub streaming: bool,
    pub language_detection: bool,
    pub translation: bool,
    pub confidence: bool,
    pub hotword_biasing: bool,
    /// Optional per-token/word timestamps.
    pub timestamps: bool,
    pub languages: Vec<AsrTag>,
    pub translate_target_languages: Vec<AsrTag>,
    pub scripts: Vec<AsrTag>,
    pub locales: Vec<AsrTag>,
}

impl AsrCapabilities {
    #[must_use]
    pub fn supports_spoken(&self, tag: &AsrTag) -> bool {
        self.languages
            .iter()
            .any(|supported| supported.matches(tag))
    }

    #[must_use]
    pub fn supports_translate_target(&self, tag: &AsrTag) -> bool {
        self.translate_target_languages
            .iter()
            .any(|supported| supported.matches(tag))
    }

    #[must_use]
    pub fn supports_script(&self, tag: &AsrTag) -> bool {
        self.scripts.iter().any(|supported| supported == tag)
    }

    #[must_use]
    pub fn supports_locale(&self, tag: &AsrTag) -> bool {
        self.locales.iter().any(|supported| supported == tag)
    }

    /// Reject unsupported combinations before audio is handed to an engine.
    /// Errors are typed and content-free.
    pub fn validate(&self, request: &AsrRequest) -> Result<(), AsrError> {
        if [
            self.languages.len(),
            self.translate_target_languages.len(),
            self.scripts.len(),
            self.locales.len(),
        ]
        .into_iter()
        .any(|count| count > MAX_ATTESTED_TAGS)
        {
            return Err(AsrError::new(
                INVALID_CAPABILITIES,
                "engine capability attestations exceed their bound",
            ));
        }
        match &request.spoken_language {
            SpokenLanguage::Unspecified => {}
            SpokenLanguage::AutoDetect => {
                if !self.language_detection {
                    return Err(AsrError::new(
                        UNSUPPORTED_LANGUAGE_DETECT,
                        "automatic spoken-language detection is not supported by the loaded model",
                    ));
                }
            }
            SpokenLanguage::Explicit(tag) => {
                if !self.supports_spoken(tag) {
                    return Err(AsrError::new(
                        UNSUPPORTED_SPOKEN_LANGUAGE,
                        "requested spoken language is not supported by the loaded model",
                    ));
                }
            }
        }

        if let Some(target) = &request.translation_target {
            if !self.translation || !self.supports_translate_target(target) {
                return Err(AsrError::new(
                    UNSUPPORTED_TRANSLATION,
                    "requested translation is not supported by the loaded model",
                ));
            }
        }

        if let Some(output) = &request.output.language {
            if !output_language_supported(self, request, output) {
                return Err(AsrError::new(
                    UNSUPPORTED_OUTPUT_LANGUAGE,
                    "requested written language is not supported by the loaded model",
                ));
            }
            if let Some(target) = &request.translation_target
                && !output.matches(target)
            {
                return Err(AsrError::new(
                    CONFLICTING_LANGUAGE_SETTINGS,
                    "written language does not match the translation target",
                ));
            }
            if request.translation_target.is_none()
                && let SpokenLanguage::Explicit(spoken) = &request.spoken_language
                && !output.matches(spoken)
            {
                return Err(AsrError::new(
                    CONFLICTING_LANGUAGE_SETTINGS,
                    "written language differs from spoken language without a translation target",
                ));
            }
        }

        if let Some(script) = &request.output.script
            && !self.supports_script(script)
        {
            return Err(AsrError::new(
                UNSUPPORTED_SCRIPT,
                "requested output script is not supported by the loaded model",
            ));
        }

        if let Some(locale) = &request.locale
            && !self.supports_locale(locale)
        {
            return Err(AsrError::new(
                UNSUPPORTED_LOCALE,
                "requested locale or formatting convention is not supported by the loaded model",
            ));
        }

        if request.context_bias.is_some() && !self.hotword_biasing {
            return Err(AsrError::new(
                UNSUPPORTED_CONTEXT_BIAS,
                "context bias is not supported by the loaded model",
            ));
        }

        Ok(())
    }
}

fn output_language_supported(
    capabilities: &AsrCapabilities,
    request: &AsrRequest,
    output: &AsrTag,
) -> bool {
    if request.translation_target.is_some() {
        capabilities.supports_translate_target(output)
    } else {
        capabilities.supports_spoken(output)
    }
}

fn is_valid_tag(raw: &str) -> bool {
    if raw.is_empty() || raw.len() > MAX_TAG_BYTES {
        return false;
    }
    let mut parts = raw.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    if !(2..=8).contains(&primary.len()) || !primary.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return false;
    }
    for part in parts {
        if !(1..=8).contains(&part.len()) || !part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn tag(raw: &str) -> AsrTag {
        AsrTag::parse(raw).expect("test tag")
    }

    fn tiny_en() -> AsrCapabilities {
        AsrCapabilities {
            languages: vec![tag("en")],
            ..AsrCapabilities::default()
        }
    }

    fn multilingual() -> AsrCapabilities {
        AsrCapabilities {
            language_detection: true,
            translation: true,
            hotword_biasing: true,
            languages: vec![tag("en"), tag("de"), tag("es")],
            translate_target_languages: vec![tag("en"), tag("de")],
            scripts: vec![tag("Latn")],
            locales: vec![tag("en-US"), tag("de-DE")],
            ..AsrCapabilities::default()
        }
    }

    #[test]
    fn default_request_is_valid_on_empty_capabilities() {
        AsrCapabilities::default()
            .validate(&AsrRequest::default())
            .unwrap();
        assert!(AsrRequest::default().is_default());
        assert!(AsrRequest::default().applies_as_engine_default(&AsrCapabilities::default()));
    }

    #[test]
    fn spoken_output_translation_locale_and_bias_are_distinct_fields() {
        let request = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(tag("ur")),
            output: WrittenOutput {
                language: Some(tag("ur")),
                script: Some(tag("Arab")),
            },
            translation_target: Some(tag("en")),
            locale: Some(tag("ur-PK")),
            context_bias: Some(ContextBias::try_new(["narrivy"]).unwrap()),
        };
        assert_ne!(
            match &request.spoken_language {
                SpokenLanguage::Explicit(spoken) => spoken.as_str(),
                _ => "",
            },
            request.translation_target.as_ref().unwrap().as_str()
        );
        assert_eq!(request.output.script.as_ref().unwrap().as_str(), "arab");
        assert_eq!(request.locale.as_ref().unwrap().as_str(), "ur-pk");
        assert_eq!(request.context_bias.as_ref().unwrap().hint_count(), 1);
    }

    #[test]
    fn tiny_en_allows_explicit_english_and_rejects_every_unsupported_setting() {
        let caps = tiny_en();
        let english = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(tag("en")),
            ..AsrRequest::default()
        };
        caps.validate(&english).unwrap();
        assert!(english.applies_as_engine_default(&caps));

        let regional = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(tag("en-US")),
            ..AsrRequest::default()
        };
        caps.validate(&regional).unwrap();

        let auto = AsrRequest {
            spoken_language: SpokenLanguage::AutoDetect,
            ..AsrRequest::default()
        };
        assert_eq!(
            caps.validate(&auto).unwrap_err().code,
            UNSUPPORTED_LANGUAGE_DETECT
        );

        let german = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(tag("de")),
            ..AsrRequest::default()
        };
        assert_eq!(
            caps.validate(&german).unwrap_err().code,
            UNSUPPORTED_SPOKEN_LANGUAGE
        );

        let translate = AsrRequest {
            translation_target: Some(tag("en")),
            ..AsrRequest::default()
        };
        assert_eq!(
            caps.validate(&translate).unwrap_err().code,
            UNSUPPORTED_TRANSLATION
        );

        let script = AsrRequest {
            output: WrittenOutput {
                script: Some(tag("Latn")),
                ..WrittenOutput::default()
            },
            ..AsrRequest::default()
        };
        assert_eq!(caps.validate(&script).unwrap_err().code, UNSUPPORTED_SCRIPT);

        let locale = AsrRequest {
            locale: Some(tag("en-US")),
            ..AsrRequest::default()
        };
        assert_eq!(caps.validate(&locale).unwrap_err().code, UNSUPPORTED_LOCALE);

        let bias = AsrRequest {
            context_bias: Some(ContextBias::try_new(["secret-hotword"]).unwrap()),
            ..AsrRequest::default()
        };
        let error = caps.validate(&bias).unwrap_err();
        assert_eq!(error.code, UNSUPPORTED_CONTEXT_BIAS);
        assert!(
            !error.message.contains("secret-hotword"),
            "context text leaked into error message"
        );
        assert!(
            !format!("{error:?}").contains("secret-hotword"),
            "context text leaked into error debug"
        );
        assert!(
            !format!("{bias:?}").contains("secret-hotword"),
            "context text leaked into request debug"
        );
    }

    #[test]
    fn empty_language_list_rejects_explicit_language_instead_of_guessing() {
        let error = AsrCapabilities::default()
            .validate(&AsrRequest {
                spoken_language: SpokenLanguage::Explicit(tag("en")),
                ..AsrRequest::default()
            })
            .unwrap_err();
        assert_eq!(error.code, UNSUPPORTED_SPOKEN_LANGUAGE);
    }

    #[test]
    fn translation_requires_attested_target_and_does_not_guess() {
        let mut caps = multilingual();
        let request = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(tag("de")),
            translation_target: Some(tag("en")),
            ..AsrRequest::default()
        };
        caps.validate(&request).unwrap();

        let unknown_target = AsrRequest {
            translation_target: Some(tag("ja")),
            ..AsrRequest::default()
        };
        assert_eq!(
            caps.validate(&unknown_target).unwrap_err().code,
            UNSUPPORTED_TRANSLATION
        );

        caps.translate_target_languages.clear();
        assert_eq!(
            caps.validate(&request).unwrap_err().code,
            UNSUPPORTED_TRANSLATION
        );
    }

    #[test]
    fn written_language_conflict_and_script_locale_bias_use_separate_codes() {
        let caps = multilingual();
        let conflict = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(tag("de")),
            output: WrittenOutput {
                language: Some(tag("en")),
                script: None,
            },
            ..AsrRequest::default()
        };
        assert_eq!(
            caps.validate(&conflict).unwrap_err().code,
            CONFLICTING_LANGUAGE_SETTINGS
        );

        let matching_output = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(tag("de")),
            output: WrittenOutput {
                language: Some(tag("de")),
                script: Some(tag("Latn")),
            },
            locale: Some(tag("de-DE")),
            context_bias: Some(ContextBias::try_new(["narrivy"]).unwrap()),
            ..AsrRequest::default()
        };
        caps.validate(&matching_output).unwrap();

        let unsupported_script = AsrRequest {
            output: WrittenOutput {
                script: Some(tag("Arab")),
                ..WrittenOutput::default()
            },
            ..AsrRequest::default()
        };
        assert_eq!(
            caps.validate(&unsupported_script).unwrap_err().code,
            UNSUPPORTED_SCRIPT
        );
    }

    #[test]
    fn tag_and_hint_bounds_fail_closed_without_echoing_content() {
        let long_tag = "a".repeat(MAX_TAG_BYTES + 1);
        let error = AsrTag::parse(&long_tag).unwrap_err();
        assert_eq!(error.code, REQUEST_FIELD_TOO_LONG);
        assert!(!error.message.contains(&long_tag));

        assert_eq!(AsrTag::parse("e").unwrap_err().code, INVALID_LANGUAGE_TAG);
        assert_eq!(
            AsrTag::parse("en_US!").unwrap_err().code,
            INVALID_LANGUAGE_TAG
        );

        let long_hint = "h".repeat(MAX_CONTEXT_HINT_BYTES + 1);
        let error = ContextBias::try_new([long_hint.as_str()]).unwrap_err();
        assert_eq!(error.code, REQUEST_FIELD_TOO_LONG);
        assert!(!error.message.contains(&long_hint));

        let too_many = (0..=MAX_CONTEXT_HINTS).map(|index| format!("h{index}"));
        assert_eq!(
            ContextBias::try_new(too_many).unwrap_err().code,
            TOO_MANY_CONTEXT_HINTS
        );
    }

    #[test]
    fn oversized_capability_attestation_fails_closed() {
        let capabilities = AsrCapabilities {
            languages: vec![tag("en"); MAX_ATTESTED_TAGS + 1],
            ..AsrCapabilities::default()
        };
        assert_eq!(
            capabilities
                .validate(&AsrRequest::default())
                .unwrap_err()
                .code,
            INVALID_CAPABILITIES
        );
    }

    #[test]
    fn auto_detect_is_not_the_default_request() {
        assert_ne!(SpokenLanguage::default(), SpokenLanguage::AutoDetect);
        let caps = multilingual();
        caps.validate(&AsrRequest {
            spoken_language: SpokenLanguage::AutoDetect,
            ..AsrRequest::default()
        })
        .unwrap();
        assert!(
            !AsrRequest {
                spoken_language: SpokenLanguage::AutoDetect,
                ..AsrRequest::default()
            }
            .applies_as_engine_default(&caps)
        );
    }
}
