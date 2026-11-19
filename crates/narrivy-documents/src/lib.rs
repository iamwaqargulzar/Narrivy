//! Narrivy's own document writers (D55): the assistant's Markdown turned
//! into Word (`.docx`), PDF and slides (`.pptx`) with no other program.
//!
//! [`parse`] reads a Markdown subset into [`Block`]s; [`render`] writes the
//! chosen format. Everything is in memory and bounded by [`MAX_MARKDOWN`].

#![forbid(unsafe_code)]

mod check;
mod docx;
mod pdf;
mod pptx;
mod xml;
mod zip;

use std::fmt;

pub use check::{Checked, check};

/// The longest Markdown accepted (bytes).
pub const MAX_MARKDOWN: usize = 400 * 1024;

/// A file format the assistant can make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Docx,
    Pdf,
    Pptx,
    Markdown,
}

impl Format {
    /// From a name the model or the app uses: `docx`/`word`, `pdf`,
    /// `pptx`/`slides`/`presentation`, `md`/`markdown`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "docx" | "word" | "doc" | "document" => Some(Self::Docx),
            "pdf" => Some(Self::Pdf),
            "pptx" | "ppt" | "slides" | "presentation" | "powerpoint" | "deck" => Some(Self::Pptx),
            "md" | "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }

    /// The file name extension, without the dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Pdf => "pdf",
            Self::Pptx => "pptx",
            Self::Markdown => "md",
        }
    }

    /// The media type.
    #[must_use]
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Pdf => "application/pdf",
            Self::Pptx => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            }
            Self::Markdown => "text/markdown",
        }
    }
}

/// Why a document could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentError {
    /// Nothing to write.
    Empty,
    /// More than [`MAX_MARKDOWN`].
    TooLarge,
}

impl DocumentError {
    /// A short code for clients.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Empty => "document_empty",
            Self::TooLarge => "document_too_large",
        }
    }
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for DocumentError {}

/// A piece of text with its emphasis.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
}

/// A part of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Level 1–3.
    Heading(u8, Vec<Span>),
    Paragraph(Vec<Span>),
    List {
        ordered: bool,
        items: Vec<Vec<Span>>,
    },
    Code(String),
    /// Rows of cells; the first row is the header.
    Table(Vec<Vec<Vec<Span>>>),
    Rule,
}

/// Make a document of `format` from `markdown`, titled `title`.
///
/// # Errors
/// [`DocumentError::Empty`] when there is nothing to write,
/// [`DocumentError::TooLarge`] above [`MAX_MARKDOWN`].
pub fn render(format: Format, title: &str, markdown: &str) -> Result<Vec<u8>, DocumentError> {
    if markdown.len() > MAX_MARKDOWN {
        return Err(DocumentError::TooLarge);
    }
    let blocks = parse(markdown);
    if blocks.is_empty() && title.trim().is_empty() {
        return Err(DocumentError::Empty);
    }
    let title = title.trim();
    Ok(match format {
        Format::Docx => docx::write(title, &blocks),
        Format::Pdf => pdf::write(title, &blocks),
        Format::Pptx => pptx::write(title, &blocks),
        Format::Markdown => {
            let mut out = String::new();
            if !title.is_empty() && !markdown.trim_start().starts_with("# ") {
                out.push_str("# ");
                out.push_str(title);
                out.push_str("\n\n");
            }
            out.push_str(markdown.trim());
            out.push('\n');
            out.into_bytes()
        }
    })
}

/// The letters in `markdown` a PDF from [`render`] cannot show (its fonts
/// cover Latin text): empty when it can show all of them. Such text is
/// kept in `.docx` and `.pptx`.
#[must_use]
pub fn pdf_missing(markdown: &str) -> Vec<char> {
    let mut missing: Vec<char> = Vec::new();
    for c in markdown.chars() {
        if !pdf::shows(c) && !missing.contains(&c) {
            missing.push(c);
            if missing.len() >= 8 {
                break;
            }
        }
    }
    missing
}

/// Whether a heading says the same as the title, whatever the capitals,
/// spaces or end punctuation.
#[must_use]
pub(crate) fn same_title(heading: &str, title: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches(['.', ':', '!'])
            .to_lowercase()
    };
    !title.trim().is_empty() && norm(heading) == norm(title)
}

/// A slide title without a "Slide 3:" the model put in front of it.
pub(crate) fn without_slide_number(spans: &[Span]) -> Vec<Span> {
    let mut spans = spans.to_vec();
    if let Some(first) = spans.first_mut() {
        let text = first.text.trim_start();
        let lower = text.to_lowercase();
        if let Some(rest) = lower.strip_prefix("slide ") {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let after = &rest[digits..];
            if digits > 0
                && (after.starts_with(':')
                    || after.starts_with(" -")
                    || after.starts_with(" –")
                    || after.starts_with('.'))
            {
                let skip = "slide ".len() + digits + 1;
                let cut = text.char_indices().nth(skip).map_or(text.len(), |(i, _)| i);
                first.text = text[cut..].trim_start_matches(['-', '–', ' ']).to_owned();
            }
        }
    }
    spans.retain(|s| !s.text.is_empty());
    spans
}

/// The plain text of some spans.
#[must_use]
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// Whether text is mostly in a right-to-left script (Urdu, Arabic, Hebrew).
#[must_use]
pub fn right_to_left(text: &str) -> bool {
    let mut rtl = 0usize;
    let mut ltr = 0usize;
    for c in text.chars() {
        if matches!(c, '\u{0590}'..='\u{08FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
        {
            rtl += 1;
        } else if c.is_alphabetic() {
            ltr += 1;
        }
    }
    rtl > ltr
}

/// Read a Markdown subset into blocks.
#[must_use]
pub fn parse(markdown: &str) -> Vec<Block> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(inline(&paragraph.join(" "))));
            paragraph.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            i += 1;
            continue;
        }
        if let Some(fence) = fence(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(fence) {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            blocks.push(Block::Code(code.join("\n")));
            continue;
        }
        if let Some((level, text)) = heading(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(level, inline(text)));
            i += 1;
            continue;
        }
        if is_rule(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if trimmed.starts_with('|') {
            flush(&mut paragraph, &mut blocks);
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('|') {
                let row = lines[i].trim();
                if !is_table_divider(row) {
                    rows.push(cells(row).into_iter().map(|c| inline(&c)).collect());
                }
                i += 1;
            }
            blocks.push(Block::Table(rows));
            continue;
        }
        if let Some((ordered, _)) = list_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let mut items: Vec<String> = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                match list_item(t) {
                    Some((o, text)) if o == ordered => items.push(text.to_owned()),
                    // A wrapped item's next line.
                    None if !t.is_empty()
                        && lines[i].starts_with([' ', '\t'])
                        && !items.is_empty() =>
                    {
                        if let Some(last) = items.last_mut() {
                            last.push(' ');
                            last.push_str(t);
                        }
                    }
                    _ => break,
                }
                i += 1;
            }
            blocks.push(Block::List {
                ordered,
                items: items.iter().map(|t| inline(t)).collect(),
            });
            continue;
        }
        paragraph.push(trimmed.trim_start_matches("> ").to_owned());
        i += 1;
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

fn fence(line: &str) -> Option<&'static str> {
    if line.starts_with("```") {
        Some("```")
    } else if line.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        let level = u8::try_from(hashes.min(3)).unwrap_or(3);
        Some((level, line[hashes..].trim().trim_end_matches('#').trim()))
    } else {
        None
    }
}

fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && (compact.chars().all(|c| c == '-')
            || compact.chars().all(|c| c == '*')
            || compact.chars().all(|c| c == '_'))
}

fn list_item(line: &str) -> Option<(bool, &str)> {
    for marker in ["- ", "* ", "+ ", "• "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some((false, rest.trim()));
        }
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && digits < 4 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((true, rest.trim()));
        }
    }
    None
}

fn is_table_divider(row: &str) -> bool {
    row.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn cells(row: &str) -> Vec<String> {
    let inner = row.trim().trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(|c| c.trim().to_owned()).collect()
}

/// Inline emphasis: `**bold**`, `*italic*`/`_italic_`, `` `code` ``,
/// `[text](link)` (the text is kept).
fn inline(text: &str) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut current = Span::default();
    let (mut bold, mut italic) = (false, false);
    let push = |spans: &mut Vec<Span>, current: &mut Span| {
        if !current.text.is_empty() {
            spans.push(std::mem::take(current));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            current.text.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '`'
            && let Some(end) = chars[i + 1..].iter().position(|&x| x == '`')
        {
            {
                push(&mut spans, &mut current);
                spans.push(Span {
                    text: chars[i + 1..i + 1 + end].iter().collect(),
                    bold,
                    italic,
                    code: true,
                });
                current = Span {
                    bold,
                    italic,
                    ..Span::default()
                };
                i += end + 2;
                continue;
            }
        }
        if (c == '*' || c == '_') && i + 1 < chars.len() && chars[i + 1] == c {
            push(&mut spans, &mut current);
            bold = !bold;
            current = Span {
                bold,
                italic,
                ..Span::default()
            };
            i += 2;
            continue;
        }
        let word_inside = c == '_'
            && i > 0
            && chars[i - 1].is_alphanumeric()
            && chars.get(i + 1).is_some_and(|n| n.is_alphanumeric());
        if (c == '*' || c == '_') && !word_inside {
            let opens = chars.get(i + 1).is_some_and(|n| !n.is_whitespace());
            if italic || opens {
                push(&mut spans, &mut current);
                italic = !italic;
                current = Span {
                    bold,
                    italic,
                    ..Span::default()
                };
                i += 1;
                continue;
            }
        }
        if c == '[' {
            let rest: String = chars[i..].iter().collect();
            if let Some(close) = rest.find("](")
                && let Some(end) = rest[close..].find(')')
            {
                current.text.push_str(&rest[1..close]);
                i += rest[..close + end + 1].chars().count();
                continue;
            }
        }
        current.text.push(c);
        i += 1;
    }
    push(&mut spans, &mut current);
    spans
}

/// The blocks grouped into slides: a heading of level 1 or 2, or a rule,
/// starts a new slide. A first level-1 heading followed directly by another
/// heading (or nothing) is the title slide.
#[must_use]
pub(crate) fn slides(blocks: &[Block]) -> Vec<(Vec<Span>, Vec<Block>)> {
    let mut slides: Vec<(Vec<Span>, Vec<Block>)> = Vec::new();
    for block in blocks {
        match block {
            Block::Heading(level, text) if *level <= 2 => {
                slides.push((without_slide_number(text), Vec::new()));
            }
            Block::Rule => slides.push((Vec::new(), Vec::new())),
            other => {
                if slides.is_empty() {
                    slides.push((Vec::new(), Vec::new()));
                }
                if let Some(last) = slides.last_mut() {
                    last.1.push(other.clone());
                }
            }
        }
    }
    slides.retain(|(title, body)| !title.is_empty() || !body.is_empty());
    slides
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_read_into_blocks() {
        let blocks = parse(
            "# Plan\n\nIntro **bold** and *it* with `x`.\nSame paragraph.\n\n- one\n- two\n  wrapped\n\n1. first\n2. second\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```\ncode\n```\n---\n### Small",
        );
        assert_eq!(blocks.len(), 8);
        assert!(matches!(&blocks[0], Block::Heading(1, t) if plain(t) == "Plan"));
        let spans = match &blocks[1] {
            Block::Paragraph(spans) => spans.clone(),
            _ => Vec::new(),
        };
        let spans = &spans;
        assert_eq!(plain(spans), "Intro bold and it with x. Same paragraph.");
        assert!(spans.iter().any(|s| s.bold && s.text == "bold"));
        assert!(spans.iter().any(|s| s.italic && s.text == "it"));
        assert!(spans.iter().any(|s| s.code && s.text == "x"));
        assert!(
            matches!(&blocks[2], Block::List { ordered: false, items } if plain(&items[1]) == "two wrapped")
        );
        assert!(matches!(&blocks[3], Block::List { ordered: true, items } if items.len() == 2));
        assert!(
            matches!(&blocks[4], Block::Table(rows) if rows.len() == 2 && plain(&rows[1][1]) == "2")
        );
        assert_eq!(blocks[5], Block::Code("code".to_owned()));
        assert_eq!(blocks[6], Block::Rule);
        assert!(matches!(&blocks[7], Block::Heading(3, _)));
    }

    #[test]
    fn links_keep_their_text_and_snake_case_is_not_italic() {
        let spans = inline("See [the docs](https://x.y) about snake_case_names.");
        assert_eq!(plain(&spans), "See the docs about snake_case_names.");
        assert!(spans.iter().all(|s| !s.italic));
    }

    #[test]
    fn formats_are_named_as_people_say_them() {
        assert_eq!(Format::from_name("Word"), Some(Format::Docx));
        assert_eq!(Format::from_name(".PDF"), Some(Format::Pdf));
        assert_eq!(Format::from_name("slides"), Some(Format::Pptx));
        assert_eq!(Format::from_name("exe"), None);
    }

    #[test]
    fn nothing_or_too_much_is_refused() {
        assert_eq!(render(Format::Pdf, " ", ""), Err(DocumentError::Empty));
        let huge = "a".repeat(MAX_MARKDOWN + 1);
        assert_eq!(
            render(Format::Docx, "t", &huge),
            Err(DocumentError::TooLarge)
        );
    }

    #[test]
    fn slides_start_at_headings_and_rules() {
        let blocks = parse("# Deck\n## One\n- a\n## Two\ntext\n---\nmore");
        let slides = slides(&blocks);
        assert_eq!(slides.len(), 4);
        assert_eq!(plain(&slides[0].0), "Deck");
        assert!(slides[0].1.is_empty());
        assert_eq!(plain(&slides[2].0), "Two");
        assert!(slides[3].0.is_empty());
    }

    #[test]
    fn a_pdf_says_which_letters_it_cannot_show() {
        assert!(pdf_missing("Café – “quoted” • 10 €").is_empty());
        assert_eq!(pdf_missing("یہ"), vec!['ی', 'ہ']);
        // Emoji and other symbols are skipped quietly, not refused.
        assert!(pdf_missing("Done ✅").is_empty());
    }

    #[test]
    fn titles_match_whatever_the_capitals_and_slide_numbers_go() {
        assert!(same_title("Team Offsite Plan", "Team offsite plan"));
        assert!(!same_title("Agenda", "Team offsite plan"));
        let slides = slides(&parse(
            "# Slide 1: What is Narrivy?\ntext\n# Slide 2 - Privacy\nmore",
        ));
        assert_eq!(plain(&slides[0].0), "What is Narrivy?");
        assert_eq!(plain(&slides[1].0), "Privacy");
    }

    #[test]
    fn urdu_is_right_to_left() {
        assert!(right_to_left("یہ اردو ہے"));
        assert!(!right_to_left("This is English"));
    }
}
