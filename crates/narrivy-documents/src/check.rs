//! Check a file after writing it, before it is shown: open it again, count
//! its pages or slides, read its text back and make sure every heading,
//! paragraph, list item and table cell of the outline is there (nothing
//! cut or lost). Reads only what this crate writes.

use crate::{Block, Format, plain, without_slide_number};

/// What the check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// Pages (PDF), slides (pptx); 0 for Word and Markdown, which reflow.
    pub units: usize,
    /// Pieces of the outline not found in the file (at most a few).
    pub missing: Vec<String>,
}

/// The stored entries of a zip this crate wrote, by name.
fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 0;
    let u16_at = |b: &[u8], i: usize| b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |b: &[u8], i: usize| {
        b.get(i..i + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    while u32_at(bytes, at) == Some(0x0403_4b50) {
        let (Some(method), Some(size), Some(name_len), Some(extra_len)) = (
            u16_at(bytes, at + 8),
            u32_at(bytes, at + 18),
            u16_at(bytes, at + 26),
            u16_at(bytes, at + 28),
        ) else {
            break;
        };
        let name_start = at + 30;
        let data_start = name_start + usize::from(name_len) + usize::from(extra_len);
        let data_end = data_start + usize::try_from(size).unwrap_or(usize::MAX);
        let (Some(name), Some(data)) = (
            bytes.get(name_start..name_start + usize::from(name_len)),
            bytes.get(data_start..data_end),
        ) else {
            break;
        };
        if method == 0 {
            out.push((String::from_utf8_lossy(name).into_owned(), data.to_vec()));
        }
        at = data_end;
    }
    out
}

/// The text of XML, its tags dropped and entities read.
fn xml_text(xml: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in xml.chars() {
        match c {
            '<' => {
                inside = true;
                out.push(' ');
            }
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The text shown by a PDF this crate wrote: its string literals.
fn pdf_text(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            i += 1;
            let mut piece = Vec::new();
            while i < bytes.len() && bytes[i] != b')' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    let next = bytes[i + 1];
                    if next.is_ascii_digit()
                        && let Some(code) = bytes
                            .get(i + 1..i + 4)
                            .and_then(|o| std::str::from_utf8(o).ok())
                            .and_then(|o| u8::from_str_radix(o, 8).ok())
                    {
                        piece.push(code);
                        i += 4;
                        continue;
                    }
                    piece.push(next);
                    i += 2;
                    continue;
                }
                piece.push(bytes[i]);
                i += 1;
            }
            // WinAnsi's upper half is Latin-1 for letters.
            out.extend(piece.iter().map(|&b| char::from(b)));
            out.push(' ');
        }
        i += 1;
    }
    out
}

/// Letters and digits only, lower case: line breaks, spacing and
/// punctuation the layout changes do not count.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The pieces of the outline the file must contain.
fn expected(blocks: &[Block], format: Format) -> Vec<String> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            Block::Heading(_, spans) => {
                let spans = if format == Format::Pptx {
                    without_slide_number(spans)
                } else {
                    spans.clone()
                };
                out.push(plain(&spans));
            }
            Block::Paragraph(spans) => out.push(plain(spans)),
            Block::List { items, .. } => out.extend(items.iter().map(|i| plain(i))),
            Block::Table(rows) => {
                out.extend(rows.iter().flatten().map(|c| plain(c)));
            }
            Block::Code(code) => out.extend(code.lines().map(str::to_owned)),
            Block::Rule => {}
        }
    }
    out.retain(|t| !squash(t).is_empty());
    out
}

/// Check `bytes`, a `format` file made from `blocks`.
#[must_use]
pub fn check(format: Format, bytes: &[u8], blocks: &[Block]) -> Checked {
    let (units, text) = match format {
        Format::Docx => {
            let text = entries(bytes)
                .into_iter()
                .find(|(n, _)| n == "word/document.xml")
                .map(|(_, d)| xml_text(&String::from_utf8_lossy(&d)))
                .unwrap_or_default();
            (0, text)
        }
        Format::Pptx => {
            let slides: Vec<(String, Vec<u8>)> = entries(bytes)
                .into_iter()
                .filter(|(n, _)| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
                .collect();
            let text = slides
                .iter()
                .map(|(_, d)| xml_text(&String::from_utf8_lossy(d)))
                .collect::<Vec<_>>()
                .join(" ");
            (slides.len(), text)
        }
        Format::Pdf => {
            let raw = String::from_utf8_lossy(bytes);
            (raw.matches("/Type /Page ").count(), pdf_text(bytes))
        }
        Format::Markdown => (0, String::from_utf8_lossy(bytes).into_owned()),
    };
    let have = squash(&text);
    let missing = expected(blocks, format)
        .into_iter()
        // What a PDF cannot show is written as "?": not counted here (the
        // daemon refuses such a PDF before writing it).
        .filter(|piece| !have.contains(&squash(&piece.replace(|c: char| !c.is_ascii(), ""))))
        .take(5)
        .collect();
    Checked { units, missing }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse, render};

    const OUTLINE: &str = "# Plan\n\nIntro with **bold** text that is long enough to wrap across a line or two in the PDF writer, \
which must keep every word.\n\n## Steps\n\n- one\n- two\n\n| A | B |\n|---|---|\n| x | y |\n\n```\ncode line\n```";

    #[test]
    fn every_format_is_read_back_whole() -> Result<(), crate::DocumentError> {
        let blocks = parse(OUTLINE);
        for format in [Format::Docx, Format::Pdf, Format::Pptx, Format::Markdown] {
            let bytes = render(format, "Plan", OUTLINE)?;
            let checked = check(format, &bytes, &blocks);
            assert!(
                checked.missing.is_empty(),
                "{format:?}: {:?}",
                checked.missing
            );
            match format {
                Format::Pdf => assert_eq!(checked.units, 1),
                Format::Pptx => assert!(checked.units >= 2),
                _ => {}
            }
        }
        Ok(())
    }

    #[test]
    fn something_lost_is_reported() -> Result<(), crate::DocumentError> {
        let bytes = render(Format::Docx, "Plan", "# Plan\n\n- one")?;
        let checked = check(Format::Docx, &bytes, &parse("# Plan\n\n- one\n- two"));
        assert_eq!(checked.missing, vec!["two".to_owned()]);
        Ok(())
    }

    #[test]
    fn a_long_deck_and_document_count_their_slides_and_pages() -> Result<(), crate::DocumentError> {
        let long: String = (0..40)
            .map(|i| format!("## Point {i}\n\nText for point {i}.\n\n"))
            .collect();
        let slides = check(
            Format::Pptx,
            &render(Format::Pptx, "Deck", &long)?,
            &parse(&long),
        );
        assert_eq!(slides.units, 41);
        assert!(slides.missing.is_empty());
        let words = "word ".repeat(3000);
        let pages = check(
            Format::Pdf,
            &render(Format::Pdf, "Long", &words)?,
            &parse(&words),
        );
        assert!(pages.units >= 3);
        Ok(())
    }
}
