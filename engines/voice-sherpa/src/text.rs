//! What the voice reads: the reply cut into sentences as it arrives,
//! without markdown, thinking blocks or symbols, and with numbers written
//! as words (the voice's word lists have no digits).

use std::sync::LazyLock;

use regex::Regex;

static CODE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?s)```.*?```").ok());
static LINK: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").ok());
static LINE_MARK: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(#+|[-*•]|\d+[.)])\s+").ok());
static REPEAT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\W*(please\s+)?(say (that|it) again|repeat( that| it)?|come again|pardon( me)?|what did you say|دوبارہ\s*(کہیں|کہو|بولیں|بولو)|پھر سے\s*(کہیں|کہو|بولیں|بولو))\W*$",
    )
    .ok()
});
static NUMBER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(\d{1,2}):(\d{2})\b|(\d+(?:,\d{3})*)(?:\.(\d+))?(%?)").ok());

/// Words a speech model invents for a short noise.
const INVENTED: &[&str] = &[
    "thank you",
    "thanks",
    "thank you for watching",
    "you",
    "bye",
    "thanks for watching",
];

fn replace(pattern: &LazyLock<Option<Regex>>, text: &str, with: &str) -> String {
    match pattern.as_ref() {
        Some(regex) => regex.replace_all(text, with).into_owned(),
        None => text.to_owned(),
    }
}

/// Whether `text` is written in Urdu (Arabic) letters.
#[must_use]
pub fn has_urdu(text: &str) -> bool {
    text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c))
}

/// "Say that again", in English or Urdu.
#[must_use]
pub fn is_repeat(text: &str) -> bool {
    REPEAT
        .as_ref()
        .is_some_and(|regex| regex.is_match(text.trim()))
}

/// A short turn the speech model probably invented from a noise.
#[must_use]
pub fn is_invented(text: &str, samples: usize) -> bool {
    let plain: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    samples < (crate::RATE as usize * 3) / 2 && INVENTED.contains(&plain.as_str())
}

/// Markdown and symbols the voice would read out, removed; numbers as
/// words.
#[must_use]
pub fn speakable(text: &str) -> String {
    let text = replace(&CODE, text, " ");
    let text = replace(&LINK, &text, "$1");
    let text = replace(&LINE_MARK, &text, "");
    let text = text.replace("**", "").replace("__", "").replace('`', "");
    let text = lone_emphasis_removed(&text);
    let text = numbers_as_words(&text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `*` and `_` that open or close emphasis (not inside a word).
fn lone_emphasis_removed(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '*' || c == '_' {
            let before = i.checked_sub(1).map(|j| chars[j]);
            let after = chars.get(i + 1).copied();
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
            let opens = !word(before) && !space(after);
            let closes = !space(before) && !word(after);
            if opens || closes {
                continue;
            }
        }
        out.push(c);
    }
    out
}

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn below_thousand(n: u64, out: &mut Vec<String>) {
    let hundreds = n / 100;
    let rest = n % 100;
    if hundreds > 0 {
        out.push(ONES[hundreds as usize].to_owned());
        out.push("hundred".to_owned());
    }
    if rest >= 20 {
        let tens = TENS[(rest / 10) as usize];
        if rest % 10 == 0 {
            out.push(tens.to_owned());
        } else {
            out.push(format!("{tens}-{}", ONES[(rest % 10) as usize]));
        }
    } else if rest > 0 || hundreds == 0 {
        out.push(ONES[rest as usize].to_owned());
    }
}

/// `n` in words, up to the trillions; larger numbers digit by digit.
#[must_use]
pub fn number_words(n: u64) -> String {
    if n >= 1_000_000_000_000_000 {
        return digits_words(&n.to_string());
    }
    if n == 0 {
        return "zero".to_owned();
    }
    let mut out = Vec::new();
    let scales = [
        (1_000_000_000_000, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1_000, "thousand"),
    ];
    let mut rest = n;
    for (size, name) in scales {
        if rest >= size {
            below_thousand(rest / size, &mut out);
            out.push(name.to_owned());
            rest %= size;
        }
    }
    if rest > 0 {
        below_thousand(rest, &mut out);
    }
    out.join(" ")
}

fn digits_words(digits: &str) -> String {
    digits
        .chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| ONES[d as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

/// Digits written as words: times (`3:45`), thousands (`1,200`), decimals
/// and percentages.
#[must_use]
pub fn numbers_as_words(text: &str) -> String {
    let Some(regex) = NUMBER.as_ref() else {
        return text.to_owned();
    };
    regex
        .replace_all(text, |caps: &regex::Captures<'_>| {
            if let (Some(hour), Some(minute)) = (caps.get(1), caps.get(2)) {
                let hour: u64 = hour.as_str().parse().unwrap_or(0);
                let minute: u64 = minute.as_str().parse().unwrap_or(0);
                return match minute {
                    0 => format!("{} o'clock", number_words(hour)),
                    1..=9 => format!("{} oh {}", number_words(hour), number_words(minute)),
                    _ => format!("{} {}", number_words(hour), number_words(minute)),
                };
            }
            let whole = caps.get(3).map_or("", |m| m.as_str()).replace(',', "");
            let mut words = match whole.parse::<u64>() {
                Ok(n) if whole.len() <= 15 => number_words(n),
                _ => digits_words(&whole),
            };
            if let Some(fraction) = caps.get(4) {
                words.push_str(" point ");
                words.push_str(&digits_words(fraction.as_str()));
            }
            if caps.get(5).is_some_and(|m| !m.as_str().is_empty()) {
                words.push_str(" percent");
            }
            format!(" {words} ")
        })
        .into_owned()
}

/// Collects reply pieces and hands out whole sentences.
#[derive(Debug, Default)]
pub struct Sentences {
    buffer: String,
    /// Inside a thinking block: what has arrived of it so far.
    thought: Option<String>,
}

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";
/// A run this long with no full stop is spoken at a comma.
const LONG_RUN: usize = 220;

/// Byte offset just past the first sentence end in `text`: `.`, `!`, `?`
/// (or the Urdu marks) with closing quotes or brackets and then white
/// space, or a run of line breaks.
fn sentence_end(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let offset = |index: usize| chars.get(index).map_or(text.len(), |&(at, _)| at);
    for (i, &(_, c)) in chars.iter().enumerate() {
        if c == '\n' {
            let mut end = i;
            while end < chars.len() && chars[end].1 == '\n' {
                end += 1;
            }
            return Some(offset(end));
        }
        if matches!(c, '.' | '!' | '?' | '\u{061F}' | '\u{06D4}') {
            let mut end = i + 1;
            while end < chars.len() && matches!(chars[end].1, '"' | '\'' | ')' | ']') {
                end += 1;
            }
            let closed = end;
            while end < chars.len() && chars[end].1.is_whitespace() {
                end += 1;
            }
            if end > closed {
                return Some(offset(end));
            }
        }
    }
    None
}

impl Sentences {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a piece of the reply; returns the sentences it completed. A
    /// thinking block (if the model writes one) is not spoken.
    pub fn add(&mut self, piece: &str) -> Vec<String> {
        if let Some(thought) = self.thought.as_mut() {
            thought.push_str(piece);
            let Some(end) = thought.find(THINK_CLOSE) else {
                return Vec::new();
            };
            let after = thought[end + THINK_CLOSE.len()..].to_owned();
            self.thought = None;
            return self.add(&after);
        }
        self.buffer.push_str(piece);
        let Some(start) = self.buffer.find(THINK_OPEN) else {
            return self.complete();
        };
        let block = self.buffer.split_off(start);
        let mut out = self.complete();
        self.thought = Some(String::new());
        out.extend(self.add(&block[THINK_OPEN.len()..]));
        out
    }

    fn complete(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(end) = sentence_end(&self.buffer) {
            let rest = self.buffer.split_off(end);
            let sentence = std::mem::replace(&mut self.buffer, rest);
            push_spoken(&mut out, &sentence);
        }
        if self.buffer.len() > LONG_RUN {
            let cut = [", ", "\u{060C} "]
                .iter()
                .filter_map(|comma| self.buffer.rfind(comma).map(|at| (at, comma.len())))
                .max();
            if let Some((cut, length)) = cut.filter(|&(cut, _)| cut > 60) {
                let rest = self.buffer.split_off(cut + length);
                let sentence = std::mem::replace(&mut self.buffer, rest);
                push_spoken(&mut out, &sentence);
            }
        }
        out
    }

    /// The reply ended: whatever is left, unless it is inside a thinking
    /// block.
    pub fn rest(&mut self) -> Vec<String> {
        let rest = std::mem::take(&mut self.buffer);
        let mut out = Vec::new();
        if self.thought.take().is_none() {
            push_spoken(&mut out, &rest);
        }
        out
    }
}

fn push_spoken(out: &mut Vec<String>, sentence: &str) {
    let spoken = speakable(sentence);
    if !spoken.is_empty() {
        out.push(spoken);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_come_out_as_they_complete() {
        let mut sentences = Sentences::new();
        assert!(sentences.add("Hello there").is_empty());
        assert_eq!(sentences.add(". How are"), vec!["Hello there."]);
        assert_eq!(sentences.add(" you?\n\nFine"), vec!["How are you?"]);
        assert_eq!(sentences.rest(), vec!["Fine"]);
        assert!(sentences.rest().is_empty());
    }

    #[test]
    fn thinking_is_not_spoken_even_across_pieces() {
        let mut sentences = Sentences::new();
        assert_eq!(sentences.add("Sure. <think>let me th"), vec!["Sure."]);
        assert!(sentences.add("ink. ok</thi").is_empty());
        assert_eq!(sentences.add("nk>It is ten. "), vec!["It is ten."]);
        let mut open = Sentences::new();
        let _ = open.add("<think>never closed. ");
        assert!(open.rest().is_empty());
    }

    #[test]
    fn a_long_run_is_spoken_at_a_comma() {
        let mut sentences = Sentences::new();
        let long = format!(
            "{}, and then more words follow",
            "word ".repeat(50).trim_end()
        );
        let out = sentences.add(&long);
        assert_eq!(out.len(), 1);
        assert!(out[0].ends_with("word,"));
        assert_eq!(sentences.rest(), vec!["and then more words follow"]);
    }

    #[test]
    fn markdown_and_symbols_are_not_read_out() {
        assert_eq!(speakable("## Title"), "Title");
        assert_eq!(
            speakable("- **bold** and _soft_ `code`"),
            "bold and soft code"
        );
        assert_eq!(
            speakable("see [the docs](https://x.y) now"),
            "see the docs now"
        );
        assert_eq!(speakable("a ```let x = 1;``` b"), "a b");
        assert_eq!(speakable("snake_case stays"), "snake_case stays");
    }

    #[test]
    fn numbers_are_read_as_words() {
        assert_eq!(speakable("at 3:45 today"), "at three forty-five today");
        assert_eq!(speakable("at 9:05"), "at nine oh five");
        assert_eq!(speakable("at 10:00"), "at ten o'clock");
        assert_eq!(speakable("1,200 people"), "one thousand two hundred people");
        assert_eq!(speakable("2.5 kg"), "two point five kg");
        assert_eq!(speakable("50% off"), "fifty percent off");
        assert_eq!(speakable("in 2026"), "in two thousand twenty-six");
        assert_eq!(number_words(0), "zero");
        assert_eq!(number_words(1_000_001), "one million one");
        assert_eq!(speakable("v3"), "v three");
    }

    #[test]
    fn repeat_requests_and_invented_words_are_recognised() {
        assert!(is_repeat("Say that again?"));
        assert!(is_repeat("please repeat it"));
        assert!(is_repeat("دوبارہ کہیں"));
        assert!(!is_repeat("say that again about the weather"));
        assert!(is_invented("Thank you.", 16_000));
        assert!(!is_invented("Thank you.", 32_000));
        assert!(!is_invented("Thank you for the help", 8_000));
        assert!(has_urdu("یہ اردو ہے"));
        assert!(!has_urdu("English"));
    }
}
