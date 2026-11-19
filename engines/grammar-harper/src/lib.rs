//! Local English rule checker for the writing assistant (D26, SQ-WRITE).
//!
//! A thin layer over `harper-core`: it keeps one rule set per dialect, applies
//! the user's rule switches and personal dictionary, and reports findings with
//! UTF-16 offsets, the unit browsers use for text positions. It runs on the
//! CPU in a few milliseconds and never sees the network.

use std::collections::{BTreeMap, HashMap, HashSet};

use harper_core::linting::{Lint, LintGroup, Suggestion};
use harper_core::spell::FstDictionary;
use harper_core::{Dialect, Document};

/// Longest text one check accepts, in bytes. Longer fields are checked by the
/// caller in pieces (or not at all).
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// Most findings returned for one check; the rest are dropped, most
/// important first being kept.
pub const MAX_FINDINGS: usize = 500;

/// One problem in the checked text. `start`/`end` are UTF-16 code-unit
/// offsets into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub start: usize,
    pub end: usize,
    /// Harper's category, such as `Spelling`, `Grammar` or `Style`.
    pub kind: String,
    /// The rule that raised it, for "turn this rule off".
    pub rule: String,
    pub message: String,
    pub suggestions: Vec<Fix>,
    /// Lower is more important.
    pub priority: u8,
}

/// One way to resolve a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Replace the span with this text.
    Replace(String),
    /// Insert this text after the span.
    InsertAfter(String),
    /// Remove the span.
    Remove,
}

/// A rule the user can switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub name: String,
    pub description: String,
    pub enabled_by_default: bool,
}

/// What a check should apply on top of the defaults.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub dialect: Option<String>,
    /// Rule name to on/off, overriding the default.
    pub rules: BTreeMap<String, bool>,
    /// Personal dictionary: words never reported as misspelled.
    pub words: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    TooLong,
}

/// The checker. Building a rule set takes tens of milliseconds, so one is
/// kept per dialect and reused.
pub struct Checker {
    groups: HashMap<u8, Group>,
}

struct Group {
    lints: LintGroup,
    defaults: BTreeMap<String, bool>,
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

impl Checker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            groups: HashMap::new(),
        }
    }

    /// Every rule, sorted by name.
    pub fn rules(&mut self) -> Vec<Rule> {
        let group = self.group(Dialect::American);
        let descriptions = group.lints.all_descriptions();
        group
            .defaults
            .iter()
            .map(|(name, enabled)| Rule {
                name: name.clone(),
                description: descriptions
                    .get(name.as_str())
                    .map(|text| (*text).to_owned())
                    .unwrap_or_default(),
                enabled_by_default: *enabled,
            })
            .collect()
    }

    /// Check `text`.
    ///
    /// # Errors
    /// `TooLong` above [`MAX_TEXT_BYTES`].
    pub fn check(&mut self, text: &str, options: &Options) -> Result<Vec<Finding>, CheckError> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(CheckError::TooLong);
        }
        let dialect = parse_dialect(options.dialect.as_deref());
        let group = self.group(dialect);
        for (name, default) in &group.defaults {
            let enabled = options.rules.get(name).copied().unwrap_or(*default);
            group.lints.config.set_rule_enabled(name, enabled);
        }
        let chars: Vec<char> = text.chars().collect();
        let document = Document::new_plain_english_curated_chars(&chars);
        let organized = group.lints.organized_lints(&document);

        let words: HashSet<String> = options.words.iter().map(|w| w.to_lowercase()).collect();
        let utf16 = utf16_offsets(&chars);
        let mut findings: Vec<Finding> = organized
            .into_iter()
            .flat_map(|(rule, lints)| lints.into_iter().map(move |lint| (rule.clone(), lint)))
            .filter(|(_, lint)| !is_personal_word(lint, &chars, &words))
            .filter_map(|(rule, lint)| finding(rule, lint, &chars, &utf16))
            .collect();
        findings.sort_by_key(|f| (f.start, f.priority));
        findings.dedup_by(|b, a| a.start == b.start && a.end == b.end && a.kind == b.kind);
        if findings.len() > MAX_FINDINGS {
            findings.sort_by_key(|f| f.priority);
            findings.truncate(MAX_FINDINGS);
            findings.sort_by_key(|f| (f.start, f.priority));
        }
        Ok(findings)
    }

    fn group(&mut self, dialect: Dialect) -> &mut Group {
        self.groups.entry(dialect as u8).or_insert_with(|| {
            let lints = LintGroup::new_curated(FstDictionary::curated(), dialect);
            let defaults = lints
                .iter_keys()
                .map(|name| (name.to_owned(), lints.config.is_rule_enabled(name)))
                .collect();
            Group { lints, defaults }
        })
    }
}

fn parse_dialect(dialect: Option<&str>) -> Dialect {
    match dialect.map(str::to_ascii_lowercase).as_deref() {
        Some("british" | "en-gb" | "uk") => Dialect::British,
        Some("canadian" | "en-ca" | "ca") => Dialect::Canadian,
        Some("australian" | "en-au" | "au") => Dialect::Australian,
        Some("indian" | "en-in" | "in") => Dialect::Indian,
        _ => Dialect::American,
    }
}

fn is_personal_word(lint: &Lint, chars: &[char], words: &HashSet<String>) -> bool {
    if words.is_empty() || lint.lint_kind.to_string_key() != "Spelling" {
        return false;
    }
    let word: String = chars
        .get(lint.span.start..lint.span.end)
        .unwrap_or_default()
        .iter()
        .collect();
    words.contains(&word.to_lowercase())
}

/// `out[i]` is the UTF-16 offset of char `i`; `out[len]` is the total.
fn utf16_offsets(chars: &[char]) -> Vec<usize> {
    let mut out = Vec::with_capacity(chars.len() + 1);
    let mut at = 0;
    for c in chars {
        out.push(at);
        at += c.len_utf16();
    }
    out.push(at);
    out
}

fn finding(rule: String, lint: Lint, chars: &[char], utf16: &[usize]) -> Option<Finding> {
    if lint.span.start > lint.span.end || lint.span.end > chars.len() {
        return None;
    }
    let suggestions = lint
        .suggestions
        .into_iter()
        .take(6)
        .map(|suggestion| match suggestion {
            Suggestion::ReplaceWith(text) => Fix::Replace(text.into_iter().collect()),
            Suggestion::InsertAfter(text) => Fix::InsertAfter(text.into_iter().collect()),
            Suggestion::Remove => Fix::Remove,
        })
        .collect();
    Some(Finding {
        start: utf16[lint.span.start],
        end: utf16[lint.span.end],
        kind: lint.lint_kind.to_string_key(),
        rule,
        message: lint.message,
        suggestions,
        priority: lint.priority,
    })
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    fn check(text: &str, options: &Options) -> Vec<Finding> {
        Checker::new().check(text, options).unwrap_or_default()
    }

    #[test]
    fn a_misspelling_is_found_with_a_replacement() {
        let found = check("This is an exmaple.", &Options::default());
        let spelling = found.iter().find(|f| f.kind == "Spelling");
        let Some(spelling) = spelling else {
            panic!("no spelling finding in {found:?}");
        };
        assert_eq!((spelling.start, spelling.end), (11, 18));
        assert!(
            spelling
                .suggestions
                .contains(&Fix::Replace("example".into()))
        );
    }

    #[test]
    fn offsets_are_utf16_after_emoji_and_accents() {
        // 😀 is two UTF-16 units, é is one.
        let found = check("😀 café exmaple", &Options::default());
        let Some(spelling) = found.iter().find(|f| f.kind == "Spelling") else {
            panic!("no spelling finding in {found:?}");
        };
        assert_eq!((spelling.start, spelling.end), (8, 15));
    }

    #[test]
    fn personal_words_are_not_misspellings() {
        let options = Options {
            words: vec!["Narrivy".into()],
            ..Options::default()
        };
        let found = check("I use narrivy every day.", &options);
        assert!(found.iter().all(|f| f.kind != "Spelling"), "{found:?}");
    }

    #[test]
    fn a_rule_turned_off_is_silent() {
        let text = "This is an exmaple.";
        let found = check(text, &Options::default());
        let Some(rule) = found
            .iter()
            .find(|f| f.kind == "Spelling")
            .map(|f| f.rule.clone())
        else {
            panic!("no spelling finding");
        };
        let options = Options {
            rules: BTreeMap::from([(rule.clone(), false)]),
            ..Options::default()
        };
        assert!(check(text, &options).iter().all(|f| f.rule != rule));
    }

    #[test]
    fn a_grammar_slip_is_found() {
        let found = check("She go to the the store.", &Options::default());
        assert!(found.iter().any(|f| f.kind != "Spelling"), "{found:?}");
    }

    #[test]
    fn rules_are_listed_with_descriptions() {
        let rules = Checker::new().rules();
        assert!(rules.len() > 50);
        assert!(rules.iter().any(|r| r.enabled_by_default));
        assert!(rules.iter().all(|r| !r.name.is_empty()));
    }

    #[test]
    fn too_long_text_is_refused() {
        let text = "a ".repeat(MAX_TEXT_BYTES);
        assert_eq!(
            Checker::new().check(&text, &Options::default()),
            Err(CheckError::TooLong)
        );
    }
}
