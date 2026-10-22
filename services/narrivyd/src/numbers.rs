//! Numbers as digits: "twenty five" becomes 25, "three hundred and two"
//! 302, "five percent" 5%, "one point five" 1.5.
//!
//! The usual writing style, and conservative: zero to nine stay words
//! ("one of them") unless a unit follows ("five percent"). Anything
//! ambiguous stays as said: two number phrases side by side ("nineteen
//! ninety nine"), ordinals ("twenty first").

#![forbid(unsafe_code)]

const UNITS: [&str; 10] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];
const TEENS: [&str; 10] = [
    "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen",
    "eighteen", "nineteen",
];
const TENS: [&str; 8] = [
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const ORDINALS: [&str; 12] = [
    "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
    "tenth", "hundredth", "thousandth",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Unit,
    Teen,
    Tens,
    Hundred,
    Scale,
}

fn word_value(word: &str) -> Option<(Kind, u64)> {
    if let Some(i) = UNITS.iter().position(|w| *w == word) {
        return Some((Kind::Unit, i as u64));
    }
    if let Some(i) = TEENS.iter().position(|w| *w == word) {
        return Some((Kind::Teen, 10 + i as u64));
    }
    if let Some(i) = TENS.iter().position(|w| *w == word) {
        return Some((Kind::Tens, 20 + 10 * i as u64));
    }
    match word {
        "hundred" => Some((Kind::Hundred, 100)),
        "thousand" => Some((Kind::Scale, 1_000)),
        "million" => Some((Kind::Scale, 1_000_000)),
        "billion" => Some((Kind::Scale, 1_000_000_000)),
        _ => None,
    }
}

/// A token split into leading punctuation, the word, trailing punctuation.
struct Token<'a> {
    lead: &'a str,
    word: String,
    trail: &'a str,
    raw: &'a str,
}

fn token(raw: &str) -> Token<'_> {
    let start = raw.find(|c: char| c.is_alphanumeric()).unwrap_or(raw.len());
    let end = raw
        .rfind(|c: char| c.is_alphanumeric())
        .map_or(start, |i| i + raw[i..].chars().next().map_or(1, char::len_utf8));
    Token {
        lead: &raw[..start],
        word: raw[start..end.max(start)].to_lowercase(),
        trail: &raw[end.max(start)..],
        raw,
    }
}

/// One number phrase starting at `tokens[i]`: its value, how many tokens
/// it used, and whether it used more than a single small word.
fn phrase(tokens: &[Token<'_>], i: usize) -> Option<(u64, usize, bool)> {
    let mut total = 0u64;
    let mut current = 0u64;
    let mut used = 0;
    let mut last: Option<Kind> = None;
    let mut compound = false;
    while let Some(t) = tokens.get(i + used) {
        // "and" only inside a number: "three hundred and two".
        if t.word == "and" && last == Some(Kind::Hundred) {
            if let Some(next) = tokens.get(i + used + 1)
                && matches!(word_value(&next.word), Some((Kind::Unit | Kind::Teen | Kind::Tens, _)))
                && t.lead.is_empty()
                && t.trail.is_empty()
            {
                used += 1;
                continue;
            }
            break;
        }
        // Hyphenated: "twenty-five".
        let parts: Vec<&str> = t.word.split('-').collect();
        let mut values = Vec::new();
        for part in &parts {
            match word_value(part) {
                Some(v) => values.push(v),
                None => break,
            }
        }
        if values.len() != parts.len() || values.is_empty() {
            break;
        }
        // A new phrase would start here: stop.
        let fits = values.iter().all(|(kind, _)| {
            matches!(
                (last, kind),
                (None, _)
                    | (Some(Kind::Tens), Kind::Unit)
                    | (Some(Kind::Unit | Kind::Teen | Kind::Tens), Kind::Hundred | Kind::Scale)
                    | (Some(Kind::Hundred), Kind::Unit | Kind::Teen | Kind::Tens | Kind::Scale)
                    | (Some(Kind::Scale), Kind::Unit | Kind::Teen | Kind::Tens)
            )
        });
        if !fits {
            break;
        }
        for (kind, value) in values {
            match kind {
                Kind::Unit | Kind::Teen | Kind::Tens => current += value,
                Kind::Hundred => current = current.max(1) * 100,
                Kind::Scale => {
                    total += current.max(1) * value;
                    current = 0;
                }
            }
            if last.is_some() || kind != Kind::Unit {
                compound |= last.is_some() || matches!(kind, Kind::Hundred | Kind::Scale);
            }
            last = Some(kind);
        }
        used += 1;
        // Punctuation after a word ends the phrase.
        if !t.trail.is_empty() {
            break;
        }
    }
    // "a million", "hundreds": a scale word alone is not a count.
    let starts_with_scale = matches!(
        word_value(tokens[i].word.split('-').next().unwrap_or_default()),
        Some((Kind::Hundred | Kind::Scale, _))
    );
    (used > 0 && !starts_with_scale).then_some((total + current, used, compound))
}

/// How a value is written: "25,000", "2 million", "2026".
fn written(value: u64, last_word: &str) -> String {
    for (scale, name) in [(1_000_000_000, "billion"), (1_000_000, "million")] {
        if last_word == name && value % scale == 0 {
            return format!("{} {name}", value / scale);
        }
    }
    let plain = value.to_string();
    if value < 10_000 {
        return plain;
    }
    let mut out = String::new();
    for (i, c) in plain.chars().enumerate() {
        if i > 0 && (plain.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Write number phrases as digits.
#[must_use]
pub fn digits(text: &str) -> String {
    let tokens: Vec<Token<'_>> = text.split(' ').map(token).collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let Some((value, used, compound)) = phrase(&tokens, i) else {
            out.push(tokens[i].raw.to_owned());
            i += 1;
            continue;
        };
        let first = &tokens[i];
        let last = &tokens[i + used - 1];
        let next = tokens.get(i + used);
        let next_word = next.map(|t| t.word.as_str());
        let previous_number = i > 0
            && tokens[i - 1].trail.is_empty()
            && word_value(&tokens[i - 1].word).is_some();
        let adjacent_number = previous_number
            || (last.trail.is_empty() && next.is_some_and(|t| word_value(&t.word).is_some()));
        let ordinal = last.trail.is_empty()
            && next_word.is_some_and(|w| ORDINALS.contains(&w) || (w.ends_with("th") && w.len() > 4));
        if adjacent_number || ordinal {
            // Ambiguous: leave the words as said.
            for t in &tokens[i..i + used] {
                out.push(t.raw.to_owned());
            }
            i += used;
            continue;
        }
        let mut written = written(value, last.word.as_str());
        let mut consumed = used;
        let mut trail = last.trail.to_owned();
        // "one point five"
        if last.trail.is_empty() && next_word == Some("point") {
            let mut decimals = String::new();
            let mut j = i + used + 1;
            while let Some(t) = tokens.get(j) {
                match word_value(&t.word) {
                    Some((Kind::Unit, d)) => {
                        decimals.push_str(&d.to_string());
                        j += 1;
                        if !t.trail.is_empty() {
                            trail = t.trail.to_owned();
                            break;
                        }
                    }
                    _ => break,
                }
            }
            if !decimals.is_empty() {
                written = format!("{written}.{decimals}");
                consumed = j - i;
            }
        }
        let after = tokens.get(i + consumed);
        let percent = trail.is_empty() && after.is_some_and(|t| t.word == "percent");
        let is_small = value < 10 && !compound && !written.contains('.');
        if percent {
            let p = &tokens[i + consumed];
            out.push(format!("{}{written}%{}", first.lead, p.trail));
            i += consumed + 1;
        } else if is_small {
            out.push(first.raw.to_owned());
            i += 1;
        } else {
            out.push(format!("{}{written}{trail}", first.lead));
            i += consumed;
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::digits;

    #[test]
    fn number_phrases_become_digits() {
        assert_eq!(digits("we need twenty five chairs"), "we need 25 chairs");
        assert_eq!(digits("three hundred and two people"), "302 people");
        assert_eq!(digits("about two thousand twenty six words"), "about 2026 words");
        assert_eq!(digits("it grew five percent."), "it grew 5%.");
        assert_eq!(digits("version one point five"), "version 1.5");
        assert_eq!(digits("twenty-five, then thirty"), "25, then 30");
        assert_eq!(digits("twenty five thousand people"), "25,000 people");
        assert_eq!(digits("two million users"), "2 million users");
    }

    #[test]
    fn small_and_ambiguous_numbers_stay_words() {
        assert_eq!(digits("one of them"), "one of them");
        assert_eq!(digits("one two three"), "one two three");
        assert_eq!(digits("in nineteen ninety nine"), "in nineteen ninety nine");
        assert_eq!(digits("the twenty first of May"), "the twenty first of May");
        assert_eq!(digits("someone and nothing"), "someone and nothing");
        assert_eq!(digits("a million users"), "a million users");
        assert_eq!(digits(""), "");
    }
}
