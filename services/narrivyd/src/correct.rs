//! Deterministic text correction — the guaranteed local baseline stage.
//!
//! Rules (v1, ordered):
//! 1. collapse repeated whitespace / fix space-before-punctuation;
//! 2. ensure single space after `,.;:!?` where missing (never glue words);
//! 3. capitalize first letter of the text and after sentence terminators;
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

fn fix_space_after_punct(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        out.push(c);
        if matches!(c, ',' | '.' | ';' | ':' | '!' | '?') {
            let next = chars.get(i + 1).copied();
            match next {
                Some(n) if n.is_alphanumeric() => out.push(' '),
                _ => {}
            }
        }
        // remove space BEFORE punctuation that we may have inherited
    }
    // second pass: drop spaces before punctuators
    let mut fin = String::with_capacity(out.len());
    let oc: Vec<char> = out.chars().collect();
    for (i, &c) in oc.iter().enumerate() {
        if c == ' ' {
            if let Some(&n) = oc.get(i + 1) {
                if matches!(n, ',' | '.' | ';' | ':' | '!' | '?') {
                    continue;
                }
            }
        }
        fin.push(c);
    }
    fin
}

fn capitalize_sentences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut capitalize_next = true;
    for c in text.chars() {
        if capitalize_next && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            out.push(c);
            if matches!(c, '.' | '!' | '?') {
                capitalize_next = true;
            }
        }
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
}
