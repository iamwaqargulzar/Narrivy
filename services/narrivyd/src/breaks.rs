//! Spoken line breaks: "new line" and "new paragraph" said while dictating
//! become a line break and a blank line; the next word starts with a
//! capital. Punctuation the recogniser put around the command goes with it
//! ("first point, new line. Second" becomes "first point\nSecond").

#![forbid(unsafe_code)]

fn plain(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Turn spoken line-break commands into line breaks.
#[must_use]
pub fn line_breaks(text: &str) -> String {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    let mut out = String::with_capacity(text.len());
    let mut capital = false;
    let mut i = 0;
    while i < words.len() {
        let next = words.get(i + 1).map(|w| plain(w));
        let command = match (plain(words[i]).as_str(), next.as_deref()) {
            ("new", Some("line")) => Some("\n"),
            ("new", Some("paragraph")) => Some("\n\n"),
            _ => None,
        };
        if let Some(brk) = command {
            // The command replaces the punctuation and space before it.
            while out.ends_with([' ', ',', ';', ':']) {
                out.pop();
            }
            if !out.is_empty() {
                // Keep a full stop the speaker ended the sentence with.
                out.push_str(brk);
            }
            capital = true;
            i += 2;
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        let word = words[i];
        if capital {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
            capital = false;
        } else {
            out.push_str(word);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::line_breaks;

    #[test]
    fn spoken_commands_become_breaks() {
        assert_eq!(line_breaks("first point, new line. second point"), "first point\nSecond point");
        assert_eq!(line_breaks("Dear Ali. New paragraph thanks for the call."), "Dear Ali.\n\nThanks for the call.");
        assert_eq!(line_breaks("a list: new line milk new line eggs"), "a list\nMilk\nEggs");
    }

    #[test]
    fn ordinary_words_stay() {
        assert_eq!(line_breaks("the new lineup is out"), "the new lineup is out");
        assert_eq!(line_breaks("a new plan"), "a new plan");
        assert_eq!(line_breaks(""), "");
    }
}
