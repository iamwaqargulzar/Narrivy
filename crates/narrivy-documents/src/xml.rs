//! XML text escaping for the Office writers.

/// `text` escaped for XML element content and attribute values, with
/// characters XML 1.0 forbids dropped.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn markup_and_control_characters_are_made_safe() {
        assert_eq!(
            super::escape("a<b>&\"c'\u{1}"),
            "a&lt;b&gt;&amp;&quot;c&apos;"
        );
    }
}
