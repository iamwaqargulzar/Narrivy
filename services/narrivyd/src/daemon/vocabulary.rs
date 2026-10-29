//! The user's vocabulary: words to recognise, and replacements.
//!
//! - **Words** (names, product names, terms) reach the speech model as its
//!   initial prompt when the model takes one, so it hears them more readily.
//!   After recognition, a near miss of a word ("Narivi" for "Narrivy") is
//!   corrected to it, whether or not the model took the prompt.
//! - **Replacements** turn something said into something written ("my
//!   email" into an address, "Narrivy app" into "Narrivy"). They apply last.
//!
//! Kept in `vocabulary.json` in the state directory. Applies to every
//! pipeline: it is part of transcription.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use narrivy_asr_api::{ContextBias, MAX_CONTEXT_HINT_BYTES, MAX_CONTEXT_HINTS};
use serde::{Deserialize, Serialize};

/// Bounds on what the user keeps.
pub const MAX_WORDS: usize = 200;
pub const MAX_REPLACEMENTS: usize = 100;
pub const MAX_ENTRY_CHARS: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replacement {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vocabulary {
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub replacements: Vec<Replacement>,
}

fn path() -> Option<PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("vocabulary.json"))
}

/// The saved vocabulary (empty when none).
#[must_use]
pub fn load() -> Vocabulary {
    path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn clean(entry: &str) -> Option<String> {
    let entry = entry.split_whitespace().collect::<Vec<_>>().join(" ");
    (!entry.is_empty() && entry.chars().count() <= MAX_ENTRY_CHARS).then_some(entry)
}

/// Save a vocabulary, tidied: blanks and duplicates dropped, bounded.
///
/// # Errors
/// `vocabulary_too_large` past the bounds, `vocabulary_unavailable` when it
/// cannot be saved.
pub fn save(vocabulary: Vocabulary) -> Result<Vocabulary, &'static str> {
    let mut words: Vec<String> = Vec::new();
    for word in vocabulary.words.iter().filter_map(|w| clean(w)) {
        if !words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
            words.push(word);
        }
    }
    let mut replacements: Vec<Replacement> = Vec::new();
    for r in vocabulary.replacements {
        if let (Some(from), Some(to)) = (clean(&r.from), clean(&r.to))
            && !replacements
                .iter()
                .any(|x| x.from.eq_ignore_ascii_case(&from))
        {
            replacements.push(Replacement { from, to });
        }
    }
    if words.len() > MAX_WORDS || replacements.len() > MAX_REPLACEMENTS {
        return Err("vocabulary_too_large");
    }
    let tidy = Vocabulary {
        words,
        replacements,
    };
    let target = path().ok_or("vocabulary_unavailable")?;
    let part = target.with_extension("json.part");
    let bytes = serde_json::to_vec_pretty(&tidy).map_err(|_| "vocabulary_unavailable")?;
    std::fs::write(&part, bytes).map_err(|_| "vocabulary_unavailable")?;
    std::fs::rename(&part, &target).map_err(|_| "vocabulary_unavailable")?;
    Ok(tidy)
}

#[must_use]
pub fn to_dto(vocabulary: &Vocabulary) -> narrivy_ipc::VocabularyDto {
    narrivy_ipc::VocabularyDto {
        words: vocabulary.words.clone(),
        replacements: vocabulary
            .replacements
            .iter()
            .map(|r| narrivy_ipc::ReplacementDto {
                from: r.from.clone(),
                to: r.to.clone(),
            })
            .collect(),
    }
}

#[must_use]
pub fn from_dto(dto: narrivy_ipc::VocabularyDto) -> Vocabulary {
    Vocabulary {
        words: dto.words,
        replacements: dto
            .replacements
            .into_iter()
            .map(|r| Replacement {
                from: r.from,
                to: r.to,
            })
            .collect(),
    }
}

/// The words as context hints for the speech model: packed, comma
/// separated, into the request's bounds (the model reads a limited prompt;
/// words past the bounds still get the correction after recognition).
#[must_use]
pub fn context_bias(vocabulary: &Vocabulary) -> Option<ContextBias> {
    context_bias_with(None, vocabulary)
}

/// `given` hints (a client's own) first, then the user's words, within the
/// same bounds.
#[must_use]
pub fn context_bias_with(
    given: Option<&ContextBias>,
    vocabulary: &Vocabulary,
) -> Option<ContextBias> {
    let mut hints: Vec<String> = given.map(|bias| bias.hints().to_vec()).unwrap_or_default();
    if hints.len() >= MAX_CONTEXT_HINTS {
        return given.cloned();
    }
    let mut current = String::new();
    for word in &vocabulary.words {
        let extra = if current.is_empty() {
            word.len()
        } else {
            word.len() + 2
        };
        if current.len() + extra > MAX_CONTEXT_HINT_BYTES {
            if hints.len() + 1 >= MAX_CONTEXT_HINTS {
                break;
            }
            hints.push(std::mem::take(&mut current));
        }
        if word.len() > MAX_CONTEXT_HINT_BYTES {
            continue;
        }
        if !current.is_empty() {
            current.push_str(", ");
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        hints.push(current);
    }
    if hints.is_empty() {
        None
    } else {
        ContextBias::try_new(hints).ok()
    }
}

/// Letters and digits, lower case.
fn key(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous + usize::from(ca != cb);
            previous = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(previous + 1);
        }
    }
    row[b.len()]
}

/// A rough sound key: first letter, then consonant groups (vowels and
/// h/w/y dropped, repeats collapsed). "Narivi" and "Narrivy" share one.
fn sound(word: &str) -> String {
    let mut out = String::new();
    let mut last = None;
    for (i, c) in word.chars().enumerate() {
        let group = match c {
            'b' | 'f' | 'p' | 'v' => Some('1'),
            'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => Some('2'),
            'd' | 't' => Some('3'),
            'l' => Some('4'),
            'm' | 'n' => Some('5'),
            'r' => Some('6'),
            _ => None,
        };
        if i == 0 {
            out.push(c);
            last = group;
            continue;
        }
        if group.is_some() && group != last {
            out.extend(group);
        }
        if !matches!(c, 'h' | 'w') {
            last = group;
        }
    }
    out
}

/// Whether `heard` is close enough to `word` to be a mishearing of it.
fn near(heard: &str, word: &str) -> bool {
    let (h, w): (Vec<char>, Vec<char>) = (heard.chars().collect(), word.chars().collect());
    if w.len() < 4 || h.is_empty() || h == w {
        return false;
    }
    let distance = edit_distance(&h, &w);
    let longest = h.len().max(w.len());
    // About one edit in three letters, and it must sound alike.
    distance * 3 <= longest && sound(heard) == sound(word)
}

/// Correct near misses of the user's words, and fix their spelling and
/// capitals when heard exactly.
#[must_use]
pub fn correct(text: &str, vocabulary: &Vocabulary) -> String {
    if vocabulary.words.is_empty() {
        return text.to_owned();
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    'tokens: while i < tokens.len() {
        // Longer spans first: "open vino" heard as two words is one term.
        for size in (1..=3).rev() {
            for word in &vocabulary.words {
                if i + size > tokens.len() || size > word.split_whitespace().count() + 1 {
                    continue;
                }
                let span = &tokens[i..i + size];
                let heard: String = span.iter().map(|t| key(t)).collect();
                let wanted = key(word);
                if heard.is_empty() || wanted.is_empty() {
                    continue;
                }
                if heard == wanted || near(&heard, &wanted) {
                    let last = span[size - 1];
                    let trailing: String = last
                        .chars()
                        .rev()
                        .take_while(|c| !c.is_alphanumeric())
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    let leading: String = span[0]
                        .chars()
                        .take_while(|c| !c.is_alphanumeric())
                        .collect();
                    out.push(format!("{leading}{word}{trailing}"));
                    i += size;
                    continue 'tokens;
                }
            }
        }
        out.push(tokens[i].to_owned());
        i += 1;
    }
    out.join(" ")
}

/// Apply replacements: whole words or phrases, any capitals.
#[must_use]
pub fn replace(text: &str, vocabulary: &Vocabulary) -> String {
    let mut result = text.to_owned();
    for r in &vocabulary.replacements {
        let from = r.from.to_lowercase();
        let mut out = String::with_capacity(result.len());
        let lower = result.to_lowercase();
        // Lower-casing can change byte lengths outside ASCII: only replace
        // when the text keeps its layout.
        if lower.len() != result.len() {
            continue;
        }
        let mut start = 0;
        while let Some(found) = lower[start..].find(&from) {
            let at = start + found;
            let end = at + from.len();
            let before_ok = lower[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
            let after_ok = lower[end..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric());
            out.push_str(&result[start..at]);
            if before_ok && after_ok {
                out.push_str(&r.to);
            } else {
                out.push_str(&result[at..end]);
            }
            start = end;
        }
        out.push_str(&result[start..]);
        result = out;
    }
    result
}

/// Everything after recognition: corrections, then replacements.
#[must_use]
pub fn apply(text: &str, vocabulary: &Vocabulary) -> String {
    replace(&correct(text, vocabulary), vocabulary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(words: &[&str], replacements: &[(&str, &str)]) -> Vocabulary {
        Vocabulary {
            words: words.iter().map(|w| (*w).to_owned()).collect(),
            replacements: replacements
                .iter()
                .map(|(from, to)| Replacement {
                    from: (*from).to_owned(),
                    to: (*to).to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn near_misses_become_the_users_word() {
        let v = vocab(&["Narrivy", "OpenVINO", "Waqar"], &[]);
        assert_eq!(
            correct("I built narivi on open vino.", &v),
            "I built Narrivy on OpenVINO."
        );
        assert_eq!(correct("Ask waqar, please", &v), "Ask Waqar, please");
        // Unrelated words stay.
        assert_eq!(correct("Open the window now", &v), "Open the window now");
        assert_eq!(correct("I'm nervous", &v), "I'm nervous");
    }

    #[test]
    fn replacements_apply_to_whole_words_only() {
        let v = vocab(
            &[],
            &[("my email", "me@example.com"), ("btw", "by the way")],
        );
        assert_eq!(
            replace("Send it to My Email, btw.", &v),
            "Send it to me@example.com, by the way."
        );
        assert_eq!(replace("subtwo", &vocab(&[], &[("btw", "x")])), "subtwo");
    }

    #[test]
    fn words_pack_into_bounded_hints() -> Result<(), Box<dyn std::error::Error>> {
        let many: Vec<String> = (0..300).map(|i| format!("Term{i}")).collect();
        let v = Vocabulary {
            words: many,
            replacements: Vec::new(),
        };
        let bias = context_bias(&v).ok_or("no hints")?;
        assert!(bias.hint_count() <= MAX_CONTEXT_HINTS);
        assert!(
            bias.hints()
                .iter()
                .all(|h| h.len() <= MAX_CONTEXT_HINT_BYTES)
        );
        assert!(context_bias(&Vocabulary::default()).is_none());
        let given = ContextBias::try_new(["Lahore"]).map_err(|_| "hint")?;
        let merged =
            context_bias_with(Some(&given), &vocab(&["Narrivy"], &[])).ok_or("no hints")?;
        assert_eq!(merged.hints(), ["Lahore", "Narrivy"]);
        Ok(())
    }
}
