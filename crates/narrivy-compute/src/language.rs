//! Language, translation, and biasing capability truth (FUN-028/FUN-029).
//!
//! Capabilities are declared per model entry and are the single source of
//! truth for what the UI may offer. Unsupported combinations fail with a
//! typed error instead of silently degrading.

use crate::planner::ComputeError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Capability surface for one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageCapabilities {
    pub model_id: String,
    /// BCP-47-ish language codes genuinely supported (multilingual models only).
    pub languages: Vec<String>,
    pub translation: bool,
    pub hotword_biasing: bool,
    pub code_switch: bool,
}

impl LanguageCapabilities {
    /// English-only entry (e.g. whisper `tiny.en`): exactly `["en"]`, no
    /// translation, no code-switching. Truthful by construction.
    pub fn english_only(model_id: &str, hotword_biasing: bool) -> Self {
        Self {
            model_id: model_id.to_string(),
            languages: vec!["en".into()],
            translation: false,
            hotword_biasing,
            code_switch: false,
        }
    }

    /// Multilingual entry builder for models with real multi-language support.
    pub fn multilingual(
        model_id: &str,
        languages: &[&str],
        translation: bool,
        hotword_biasing: bool,
        code_switch: bool,
    ) -> Self {
        Self {
            model_id: model_id.to_string(),
            languages: languages.iter().map(|s| s.to_string()).collect(),
            translation,
            hotword_biasing,
            code_switch,
        }
    }
}

/// Registry of per-model capabilities.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LanguageRegistry {
    entries: BTreeMap<String, LanguageCapabilities>,
}

impl LanguageRegistry {
    pub fn new() -> Self {
        let mut entries = BTreeMap::new();
        // Known truth for the currently shipped artifact: tiny.en is
        // English-only; initial-prompt biasing IS supported by transcribe.cpp.
        entries.insert(
            "whisper-tiny.en".to_string(),
            LanguageCapabilities::english_only("whisper-tiny.en", true),
        );
        Self { entries }
    }

    pub fn insert(&mut self, caps: LanguageCapabilities) {
        self.entries.insert(caps.model_id.clone(), caps);
    }

    pub fn get(&self, model_id: &str) -> Option<&LanguageCapabilities> {
        self.entries.get(model_id)
    }

    /// Validate one requested decoding combination against model truth.
    ///
    /// Errors are typed [`ComputeError::UnsupportedCombo`] with an explicit
    /// reason suitable for direct UI display.
    pub fn check_combo(
        &self,
        model_id: &str,
        language: Option<&str>,
        translate: bool,
        want_hotwords: bool,
        want_code_switch: bool,
    ) -> Result<(), ComputeError> {
        let caps = self
            .get(model_id)
            .ok_or_else(|| ComputeError::UnsupportedCombo(format!("unknown model '{model_id}'")))?;
        if translate && !caps.translation {
            return Err(ComputeError::UnsupportedCombo(format!(
                "model '{model_id}' does not support translation"
            )));
        }
        if want_hotwords && !caps.hotword_biasing {
            return Err(ComputeError::UnsupportedCombo(format!(
                "model '{model_id}' does not support hotword/context biasing"
            )));
        }
        if want_code_switch && !caps.code_switch {
            return Err(ComputeError::UnsupportedCombo(format!(
                "model '{model_id}' does not support in-utterance code switching"
            )));
        }
        if let Some(lang) = language {
            let base = lang.split(['-', '_']).next().unwrap_or(lang);
            if !caps
                .languages
                .iter()
                .any(|l| l.split('-').next() == Some(base))
            {
                return Err(ComputeError::UnsupportedCombo(format!(
                    "language '{lang}' unsupported by '{model_id}' (supports: {})",
                    caps.languages.join(", ")
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // test scaffolding only
mod tests {
    use super::*;

    #[test]
    fn tiny_en_is_truthfully_english_only() {
        let reg = LanguageRegistry::new();
        let caps = reg.get("whisper-tiny.en").unwrap();
        assert_eq!(caps.languages, vec!["en"]);
        assert!(!caps.translation);
        assert!(!caps.code_switch);
        assert!(caps.hotword_biasing); // transcribe.cpp --initial-prompt works
    }

    #[test]
    fn english_combo_passes_on_tiny_en() {
        let reg = LanguageRegistry::new();
        reg.check_combo("whisper-tiny.en", Some("en"), false, false, false)
            .unwrap();
        // Hotword biasing supported -> passes too.
        reg.check_combo("whisper-tiny.en", Some("en"), false, true, false)
            .unwrap();
    }

    #[test]
    fn translation_on_english_only_model_is_unsupported_combo() {
        let reg = LanguageRegistry::new();
        let err = reg
            .check_combo("whisper-tiny.en", Some("en"), true, false, false)
            .unwrap_err();
        match err {
            ComputeError::UnsupportedCombo(msg) => {
                assert!(msg.contains("translation"), "msg={msg}");
            }
            other => assert!(
                matches!(other, ComputeError::UnsupportedCombo(_)),
                "wrong error: {other:?}"
            ),
        }
    }

    #[test]
    fn non_english_language_is_unsupported_combo_on_tiny_en() {
        let reg = LanguageRegistry::new();
        let err = reg
            .check_combo("whisper-tiny.en", Some("de"), false, false, false)
            .unwrap_err();
        assert!(matches!(err, ComputeError::UnsupportedCombo(_)));
        // Regional variants of supported language still pass ("en-US").
        reg.check_combo("whisper-tiny.en", Some("en-US"), false, false, false)
            .unwrap();
    }

    #[test]
    fn multilingual_registry_rejects_only_genuinely_unsupported() {
        let mut reg = LanguageRegistry::new();
        reg.insert(LanguageCapabilities::multilingual(
            "whisper-base",
            &["en", "de", "es"],
            true,
            true,
            false,
        ));
        reg.check_combo("whisper-base", Some("de"), true, true, false)
            .unwrap();
        let err = reg
            .check_combo("whisper-base", Some("ja"), false, false, false)
            .unwrap_err();
        assert!(matches!(err, ComputeError::UnsupportedCombo(_)));
        let err = reg
            .check_combo("whisper-base", None, false, false, true)
            .unwrap_err();
        assert!(matches!(err, ComputeError::UnsupportedCombo(_)));
    }
}
