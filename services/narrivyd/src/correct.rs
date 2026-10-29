//! Deterministic text correction — the guaranteed local baseline stage.
//!
//! Rules (v1, ordered):
//! 1. collapse repeated whitespace / fix space-before-punctuation;
//! 2. ensure single space after `,.;:!?` between letters (never inside
//!    numbers, times, links, file names or abbreviations);
//! 3. capitalize the first word and the word after a sentence terminator;
//! 4. guarantee terminal punctuation (`.` when no `.!?:;` at end);
//!
//! Numbers are written as digits separately (`narrivyd::numbers`), on the Clean
//! pipeline's final text, when the user keeps that on.
//!
//! Deterministic: same input -> same output, no network, no learning.

pub fn correct(text: &str) -> String {
    let mut out = collapse_whitespace(text);
    out = fix_space_after_punct(&out);
    out = capitalize_sentences(&out);
    ensure_terminal_punctuation(&out)
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

const PUNCT: [char; 6] = [',', '.', ';', ':', '!', '?'];

/// Fix spacing around `,.;:!?` without touching numbers, times, links or
/// abbreviations. A space is added after punctuation only between two letters
/// (`hello,world` -> `hello, world`); after `.` only between a lowercase and an
/// uppercase letter (`end.Next`), so `example.com`, `file.txt`, `e.g.` and
/// `U.S.` stay as they are. Words that look like a URL, email or path are left
/// alone.
fn fix_space_after_punct(text: &str) -> String {
    drop_space_before_punct(text)
        .split(' ')
        .map(|word| {
            if is_link_like(word) {
                word.to_string()
            } else {
                space_inside_word(word)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Drop a space before punctuation (`fine ,thanks` -> `fine,thanks`), but keep
/// it before a leading dot (`.txt`, `.5`).
fn drop_space_before_punct(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' {
            if let Some(&n) = chars.get(i + 1) {
                let leading_dot = n == '.' && chars.get(i + 2).is_some_and(|a| a.is_alphanumeric());
                if PUNCT.contains(&n) && !leading_dot {
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

fn space_inside_word(word: &str) -> String {
    let chars: Vec<char> = word.chars().collect();
    let mut out = String::with_capacity(word.len() + 2);
    for (i, &c) in chars.iter().enumerate() {
        out.push(c);
        if !PUNCT.contains(&c) || i == 0 {
            continue;
        }
        let (Some(&prev), Some(&next)) = (chars.get(i - 1), chars.get(i + 1)) else {
            continue;
        };
        let add = if c == '.' {
            prev.is_lowercase() && next.is_uppercase()
        } else {
            prev.is_alphabetic() && next.is_alphabetic()
        };
        if add {
            out.push(' ');
        }
    }
    out
}

fn is_link_like(word: &str) -> bool {
    word.contains('/') || word.contains('@') || word.contains('\\')
}

/// A dotted abbreviation such as `e.g.`, `i.e.`, `U.S.` or `Ph.D.`: every
/// dot-separated part is one or two letters.
fn is_dotted_abbreviation(word: &str) -> bool {
    let Some(body) = word.strip_suffix('.') else {
        return false;
    };
    body.contains('.')
        && body.split('.').all(|part| {
            (1..=2).contains(&part.chars().count()) && part.chars().all(char::is_alphabetic)
        })
}

/// Capitalize the first word of the text and the word after `.!?`. Works on
/// whole words, so a dot inside a word (`example.com`, `3.14`) never starts a
/// sentence, and a dotted abbreviation (`e.g.`) does not either.
fn capitalize_sentences(text: &str) -> String {
    let mut capitalize_next = true;
    text.split(' ')
        .map(|word| {
            let mut out = word.to_string();
            if capitalize_next && !word.is_empty() {
                if !is_link_like(word) {
                    out = capitalize_word(word);
                }
                capitalize_next = false;
            }
            let end = word.trim_end_matches(['"', '\'', ')', ']', '\u{201D}', '\u{2019}']);
            if end.ends_with(['.', '!', '?']) && !is_dotted_abbreviation(end) {
                capitalize_next = true;
            }
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Uppercase the first letter of a word, after any leading quotes or brackets.
/// A word that starts with a digit is left alone.
fn capitalize_word(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut done = false;
    for c in word.chars() {
        if !done && c.is_alphanumeric() {
            done = true;
            if c.is_alphabetic() {
                out.extend(c.to_uppercase());
                continue;
            }
        }
        out.push(c);
    }
    out
}

fn ensure_terminal_punctuation(text: &str) -> String {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return String::new();
    }
    match trimmed.chars().last() {
        // already terminated — never stack punctuation
        Some(c) if ".!?;:".contains(c) => trimmed.to_string(),
        _ => format!("{trimmed}."),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_correction_basics() {
        assert_eq!(correct("hello   world"), "Hello world.");
        assert_eq!(correct("one two three"), "One two three.");
        assert_eq!(correct("it's  fine ,thanks"), "It's fine, thanks.");
        assert_eq!(correct(""), "");
        // deterministic baseline cannot infer interrogatives; it appends "."
        assert_eq!(correct("wait , what"), "Wait, what.");
        assert_eq!(correct("stop!"), "Stop!");
    }

    #[test]
    fn correction_never_glues_words() {
        let out = correct("comma,then word");
        assert_eq!(out, "Comma, then word.");
        assert!(!out.contains(",then"));
    }

    #[test]
    fn correction_is_deterministic() {
        let a = correct("some   spoken TEXT one two");
        let b = correct("some   spoken TEXT one two");
        assert_eq!(a, b);
    }

    #[test]
    fn sentences_capitalize_after_terminators() {
        assert_eq!(
            correct("good morning. how are you"),
            "Good morning. How are you."
        );
    }

    #[test]
    fn punctuation_inside_tokens_is_kept() {
        let cases = [
            ("pi is 3.14 today", "Pi is 3.14 today."),
            ("we sold 1,000 units", "We sold 1,000 units."),
            ("meet at 10:30 please", "Meet at 10:30 please."),
            ("visit example.com now", "Visit example.com now."),
            ("open https://a.b/c?d=1 now", "Open https://a.b/c?d=1 now."),
            ("mail me@x.org today", "Mail me@x.org today."),
            ("save file.txt now", "Save file.txt now."),
            ("use one e.g. this", "Use one e.g. this."),
            ("the U.S. army", "The U.S. army."),
            ("see .txt files", "See .txt files."),
            ("hello,world", "Hello, world."),
            ("end.Next", "End. Next."),
            ("note:buy milk", "Note: buy milk."),
            ("what?no way", "What? No way."),
            ("3 apples", "3 apples."),
        ];
        for (input, want) in cases {
            assert_eq!(correct(input), want, "input: {input:?}");
        }
    }
}
