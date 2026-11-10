//! English text to Kokoro's phonemes, without eSpeak NG.
//!
//! Words are looked up in the voice's own word list (`lexicon-us-en.txt`,
//! about 178 000 words). The list has few inflected forms ("degree" but
//! not "degrees"), so a word it lacks is tried as a known word plus an
//! ending (-s, -es, -'s, -ed, -ing, -er, -est, -ly), then as the parts of a
//! hyphenated word. A short all-capitals word is spelled out; anything
//! else, such as a name, is read with simple English spelling rules, so it
//! is said, not skipped.

use std::collections::HashMap;

/// Kokoro's stress mark.
const STRESS: char = 'ˈ';

/// The voice's word list.
#[derive(Debug, Default)]
pub struct Lexicon {
    words: HashMap<String, String>,
}

impl Lexicon {
    /// Parse `word p h o n e s` lines; malformed lines are skipped.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut words = HashMap::new();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let Some(word) = parts.next() else {
                continue;
            };
            let phonemes: String = parts.collect();
            if !phonemes.is_empty() {
                words.entry(word.to_lowercase()).or_insert(phonemes);
            }
        }
        Self { words }
    }

    /// How many words it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.words.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    fn get(&self, word: &str) -> Option<String> {
        self.words.get(word).cloned()
    }

    /// The phonemes of one word (letters and apostrophes).
    #[must_use]
    pub fn word(&self, word: &str) -> String {
        let lower = word.to_lowercase();
        if let Some(found) = self.get(&lower) {
            // A short all-capitals word not used as a word ("NPU") is spelled.
            if !(is_acronym(word) && !self.is_common(&lower)) {
                return found;
            }
        }
        if is_acronym(word) {
            return self.spelled(&lower);
        }
        if let Some(found) = self.inflected(&lower) {
            return found;
        }
        if lower.contains('-') {
            let parts: Vec<String> = lower
                .split('-')
                .filter(|part| !part.is_empty())
                .map(|part| self.word(part))
                .collect();
            return parts.join(" ");
        }
        by_rules(&lower)
    }

    /// Whether an all-capitals word is also an ordinary word ("IT", "US"
    /// are read as words when the list knows them and they are long).
    fn is_common(&self, lower: &str) -> bool {
        lower.chars().count() > 3 && self.words.contains_key(lower)
    }

    fn spelled(&self, lower: &str) -> String {
        lower
            .chars()
            .filter(char::is_ascii_alphabetic)
            .map(|letter| {
                self.get(&letter.to_string())
                    .unwrap_or_else(|| by_rules(&letter.to_string()))
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn inflected(&self, word: &str) -> Option<String> {
        if let Some(base) = word.strip_suffix("'s").or_else(|| word.strip_suffix("s'")) {
            return self.known(base).map(|found| plural(&found));
        }
        if let Some(stem) = word.strip_suffix("ies")
            && let Some(found) = self.known(&format!("{stem}y"))
        {
            return Some(format!("{found}z"));
        }
        if let Some(stem) = word.strip_suffix("es")
            && ["s", "x", "z", "ch", "sh"]
                .iter()
                .any(|end| stem.ends_with(end))
            && let Some(found) = self.known(stem)
        {
            return Some(format!("{found}ᵻz"));
        }
        if let Some(stem) = word.strip_suffix('s')
            && !stem.ends_with('s')
            && let Some(found) = self.known(stem)
        {
            return Some(plural(&found));
        }
        if let Some(stem) = word.strip_suffix("ied")
            && let Some(found) = self.known(&format!("{stem}y"))
        {
            return Some(format!("{found}d"));
        }
        if let Some(stem) = word.strip_suffix("ed")
            && let Some(found) = self.stem_of(stem)
        {
            return Some(past(&found));
        }
        if let Some(stem) = word.strip_suffix("ing")
            && let Some(found) = self.stem_of(stem)
        {
            return Some(format!("{found}ɪŋ"));
        }
        if let Some(stem) = word.strip_suffix("ly")
            && let Some(found) = self.known(stem)
        {
            return Some(format!("{found}li"));
        }
        if let Some(stem) = word.strip_suffix("est")
            && let Some(found) = self.stem_of(stem)
        {
            return Some(format!("{found}ᵻst"));
        }
        if let Some(stem) = word.strip_suffix("er")
            && let Some(found) = self.stem_of(stem)
        {
            return Some(format!("{found}ɚ"));
        }
        None
    }

    fn known(&self, word: &str) -> Option<String> {
        (!word.is_empty()).then(|| self.get(word)).flatten()
    }

    /// The stem an ending was added to: as written, with its silent `e`
    /// back ("making"), or with a doubled consonant undone ("running").
    fn stem_of(&self, stem: &str) -> Option<String> {
        self.known(stem)
            .or_else(|| self.known(&format!("{stem}e")))
            .or_else(|| {
                let mut chars = stem.chars().rev();
                match (chars.next(), chars.next()) {
                    (Some(a), Some(b)) if a == b && !"aeiou".contains(a) => {
                        self.known(&stem[..stem.len() - a.len_utf8()])
                    }
                    _ => None,
                }
            })
            .or_else(|| {
                stem.strip_suffix('i')
                    .and_then(|s| self.known(&format!("{s}y")))
            })
    }

    /// A whole sentence: words, spaces and the punctuation Kokoro knows.
    #[must_use]
    pub fn sentence(&self, text: &str) -> String {
        let mut out = String::new();
        let mut word = String::new();
        let flush = |word: &mut String, out: &mut String| {
            if !word.is_empty() {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(&self.word(word));
                word.clear();
            }
        };
        for c in text.chars() {
            if c.is_alphanumeric() || (matches!(c, '\'' | '’' | '-') && !word.is_empty()) {
                word.push(if c == '’' { '\'' } else { c });
                continue;
            }
            flush(&mut word, &mut out);
            match c {
                ',' | '.' | '!' | '?' | ';' | ':' | '—' | '…' => {
                    out.push(c);
                }
                c if c.is_whitespace() && !out.is_empty() && !out.ends_with(' ') => {
                    out.push(' ');
                }
                _ => {}
            }
        }
        flush(&mut word, &mut out);
        // Words are trimmed of hyphens or apostrophes they end with.
        out.trim().to_owned()
    }
}

fn is_acronym(word: &str) -> bool {
    let letters = word.chars().filter(char::is_ascii_alphabetic).count();
    (2..=5).contains(&letters)
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn last_sound(phonemes: &str) -> Option<char> {
    phonemes
        .chars()
        .rev()
        .find(|c| *c != STRESS && *c != 'ˌ' && *c != 'ː')
}

/// The plural or possessive ending after `found`.
fn plural(found: &str) -> String {
    match last_sound(found) {
        Some('s' | 'z' | 'ʃ' | 'ʒ' | 'ʧ' | 'ʤ') => format!("{found}ᵻz"),
        Some('p' | 't' | 'k' | 'f' | 'θ') => format!("{found}s"),
        _ => format!("{found}z"),
    }
}

/// The past ending after `found`.
fn past(found: &str) -> String {
    match last_sound(found) {
        Some('t' | 'd') => format!("{found}ᵻd"),
        Some('p' | 'k' | 'f' | 'θ' | 's' | 'ʃ' | 'ʧ') => format!("{found}t"),
        _ => format!("{found}d"),
    }
}

/// Simple English spelling rules, longest match first, with the stress on
/// the first vowel.
#[must_use]
pub fn by_rules(word: &str) -> String {
    const RULES: &[(&str, &str)] = &[
        ("tion", "ʃən"),
        ("sion", "ʒən"),
        ("ough", "O"),
        ("eigh", "A"),
        ("igh", "I"),
        ("tch", "ʧ"),
        ("sch", "sk"),
        ("ch", "ʧ"),
        ("sh", "ʃ"),
        ("th", "θ"),
        ("ph", "f"),
        ("wh", "w"),
        ("ck", "k"),
        ("ng", "ŋ"),
        ("qu", "kw"),
        ("kh", "k"),
        ("gh", "ɡ"),
        ("ee", "i"),
        ("ea", "i"),
        ("ie", "i"),
        ("oo", "u"),
        ("ou", "W"),
        ("ow", "O"),
        ("oa", "O"),
        ("oi", "Y"),
        ("oy", "Y"),
        ("ai", "A"),
        ("ay", "A"),
        ("au", "ɔ"),
        ("aw", "ɔ"),
        ("ey", "A"),
        ("ar", "ɑɹ"),
        ("er", "ɚ"),
        ("ir", "ɚ"),
        ("ur", "ɚ"),
        ("or", "ɔɹ"),
        ("a", "æ"),
        ("e", "ɛ"),
        ("i", "ɪ"),
        ("o", "ɑ"),
        ("u", "ʌ"),
        ("y", "i"),
        ("b", "b"),
        ("c", "k"),
        ("d", "d"),
        ("f", "f"),
        ("g", "ɡ"),
        ("h", "h"),
        ("j", "ʤ"),
        ("k", "k"),
        ("l", "l"),
        ("m", "m"),
        ("n", "n"),
        ("p", "p"),
        ("q", "k"),
        ("r", "ɹ"),
        ("s", "s"),
        ("t", "t"),
        ("v", "v"),
        ("w", "w"),
        ("x", "ks"),
        ("z", "z"),
    ];
    const VOWELS: &str = "æɛɪɑʌiuAIOWYɚɔə";
    let word: String = word.chars().filter(char::is_ascii_alphabetic).collect();
    // A final silent `e` after a consonant ("Zubaire").
    let word = match word.strip_suffix('e') {
        Some(stem) if stem.len() > 2 && !stem.ends_with(['a', 'e', 'i', 'o', 'u']) => {
            stem.to_owned()
        }
        _ => word,
    };
    let mut out = String::new();
    let mut rest = word.as_str();
    let mut stressed = false;
    while !rest.is_empty() {
        let Some(&(letters, sounds)) = RULES.iter().find(|(letters, _)| rest.starts_with(letters))
        else {
            rest = &rest[rest.chars().next().map_or(1, char::len_utf8)..];
            continue;
        };
        if !stressed && sounds.chars().next().is_some_and(|c| VOWELS.contains(c)) {
            out.push(STRESS);
            stressed = true;
        }
        out.push_str(sounds);
        rest = &rest[letters.len()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexicon() -> Lexicon {
        Lexicon::parse(
            "degree d ə ɡ ɹ ˈ i\ncat k ˈ æ t\ndog d ˈ ɔ ɡ\nwant w ˈ ɑ n t\nmake m ˈ A k\n\
             run ɹ ˈ ʌ n\nbus b ˈ ʌ s\ncity s ˈ ɪ t i\nquick k w ˈ ɪ k\nn ˈ ɛ n\np p ˈ i\n\
             u j ˈ u\nhello h ə l ˈ O\nworld w ˈ ɜ ɹ l d\n",
        )
    }

    #[test]
    fn known_words_come_from_the_list() {
        let words = lexicon();
        assert_eq!(words.word("Hello"), "həlˈO");
        assert_eq!(words.len(), 14);
    }

    #[test]
    fn endings_are_added_to_known_words() {
        let words = lexicon();
        assert_eq!(words.word("degrees"), "dəɡɹˈiz");
        assert_eq!(words.word("cats"), "kˈæts");
        assert_eq!(words.word("buses"), "bˈʌsᵻz");
        assert_eq!(words.word("dog's"), "dˈɔɡz");
        assert_eq!(words.word("wanted"), "wˈɑntᵻd");
        assert_eq!(words.word("making"), "mˈAkɪŋ");
        assert_eq!(words.word("running"), "ɹˈʌnɪŋ");
        assert_eq!(words.word("quickly"), "kwˈɪkli");
        assert_eq!(words.word("cities"), "sˈɪtiz");
    }

    #[test]
    fn acronyms_are_spelled_and_names_are_said() {
        let words = lexicon();
        assert_eq!(words.word("NPU"), "ˈɛn pˈi jˈu");
        let name = words.word("Waqar");
        assert!(name.starts_with("wˈæ") && name.ends_with("ɑɹ"), "{name}");
        assert!(!words.word("Zubair").is_empty());
        assert_eq!(words.word("dog-cat"), "dˈɔɡ kˈæt");
    }

    #[test]
    fn a_sentence_keeps_its_spaces_and_punctuation() {
        let words = lexicon();
        assert_eq!(words.sentence("Hello, world!"), "həlˈO, wˈɜɹld!");
        assert_eq!(words.sentence("  (Hello)  world. "), "həlˈO wˈɜɹld.");
    }
}
