//! A guard against Whisper's repetition loops (HANDOFF 000c).
//!
//! On noise, music or a cut-off word, Whisper can fall into a loop and
//! write the same few words until its token limit ("and more and more" two
//! hundred times in the owner's live transcript). People do repeat words,
//! but not four times running: a phrase of up to [`MAX_PHRASE_WORDS`] words
//! said [`LOOP_REPEATS`] or more times in a row is a loop. It is cut back to
//! two times, which keeps a real "and more and more".

/// The longest repeated phrase looked for, in words.
pub const MAX_PHRASE_WORDS: usize = 8;
/// This many back-to-back copies of one phrase make a loop.
pub const LOOP_REPEATS: usize = 4;
/// Copies of the phrase kept when a loop is cut.
const KEPT_REPEATS: usize = 2;

/// Words compared without case or the punctuation stuck to them.
fn key(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// `text` with every loop cut back to two copies, and whether any was cut.
#[must_use]
pub fn cut_loops(text: &str) -> (String, bool) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let keys: Vec<String> = words.iter().map(|w| key(w)).collect();
    let mut kept: Vec<&str> = Vec::with_capacity(words.len());
    let mut cut = false;
    let mut at = 0;
    while at < words.len() {
        let mut best: Option<(usize, usize)> = None;
        for size in 1..=MAX_PHRASE_WORDS {
            if at + size * LOOP_REPEATS > words.len() {
                break;
            }
            let phrase = &keys[at..at + size];
            if phrase.iter().all(String::is_empty) {
                continue;
            }
            let mut copies = 1;
            while at + (copies + 1) * size <= words.len()
                && keys[at + copies * size..at + (copies + 1) * size] == *phrase
            {
                copies += 1;
            }
            // The longest stretch wins: "a b a b a b a b" is one phrase of
            // two words, not two loops of one.
            if copies >= LOOP_REPEATS && best.is_none_or(|(s, c)| size * copies > s * c) {
                best = Some((size, copies));
            }
        }
        match best {
            Some((size, copies)) => {
                kept.extend_from_slice(&words[at..at + size * KEPT_REPEATS]);
                at += size * copies;
                cut = true;
            }
            None => {
                kept.push(words[at]);
                at += 1;
            }
        }
    }
    if cut {
        (kept.join(" "), true)
    } else {
        (text.to_owned(), false)
    }
}

/// How many words the loops in `text` hold beyond their kept copies.
#[must_use]
pub fn looped_words(text: &str) -> usize {
    let (kept, cut) = cut_loops(text);
    if cut {
        text.split_whitespace()
            .count()
            .saturating_sub(kept.split_whitespace().count())
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_loop_is_cut_to_two_copies() {
        let looped = format!("We need to plan {}and more.", "and more ".repeat(200));
        let (text, cut) = cut_loops(&looped);
        assert!(cut);
        assert_eq!(text, "We need to plan and more and more");
        assert!(looped_words(&looped) > 390);
    }

    #[test]
    fn real_speech_is_left_alone() {
        for said in [
            "and more and more people came",
            "no no no, that is not it",
            "very very very good",
            "Thank you. Thank you. Thank you.",
            "",
        ] {
            assert_eq!(cut_loops(said), (said.to_owned(), false), "{said}");
        }
    }

    #[test]
    fn a_looping_sentence_keeps_what_came_before_and_after() {
        let looped = format!(
            "First point. {} Last point.",
            "I think so. ".repeat(6).trim_end()
        );
        let (text, cut) = cut_loops(&looped);
        assert!(cut);
        assert_eq!(text, "First point. I think so. I think so. Last point.");
    }

    #[test]
    fn case_and_punctuation_do_not_hide_a_loop() {
        let (text, cut) = cut_loops("Go, go. GO go! go go");
        assert!(cut);
        assert_eq!(text, "Go, go.");
    }

    #[test]
    fn urdu_loops_are_cut_too() {
        let (text, cut) = cut_loops(&"اور زیادہ ".repeat(10));
        assert!(cut);
        assert_eq!(text, "اور زیادہ اور زیادہ");
    }
}
