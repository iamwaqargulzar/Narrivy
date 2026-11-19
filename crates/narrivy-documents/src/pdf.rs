//! PDF documents with the standard fonts every PDF reader has (Helvetica
//! and Courier, WinAnsi encoding): nothing embedded, nothing to install.
//! Characters outside that encoding are written as `?`.

use crate::{Block, Span, plain};

const PAGE_W: f32 = 595.0;
const PAGE_H: f32 = 842.0;
const MARGIN: f32 = 56.0;
const TEXT_W: f32 = PAGE_W - 2.0 * MARGIN;
const BOTTOM: f32 = MARGIN + 14.0;

/// Helvetica's advance widths for ASCII 32..=126 (thousandths of an em).
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
/// Helvetica-Bold's.
const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Font {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
}

impl Font {
    fn of(span: &Span) -> Self {
        match (span.code, span.bold, span.italic) {
            (true, _, _) => Self::Mono,
            (_, true, true) => Self::BoldItalic,
            (_, true, false) => Self::Bold,
            (_, false, true) => Self::Italic,
            _ => Self::Regular,
        }
    }
    const fn name(self) -> &'static str {
        match self {
            Self::Regular => "F1",
            Self::Bold => "F2",
            Self::Italic => "F3",
            Self::BoldItalic => "F4",
            Self::Mono => "F5",
        }
    }
    fn bold(self) -> Self {
        match self {
            Self::Regular => Self::Bold,
            Self::Italic => Self::BoldItalic,
            other => other,
        }
    }
}

/// The WinAnsi byte for a character, or `?`.
fn byte(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\u{A0}'..='\u{FF}' => u8::try_from(c as u32).unwrap_or(b'?'),
        '€' => 0x80,
        '…' => 0x85,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '™' => 0x99,
        '\t' => b' ',
        _ => b'?',
    }
}

/// Whether a character is shown as itself; only letters and digits the
/// fonts lack count as missing (a symbol such as an emoji becomes `?`).
pub fn shows(c: char) -> bool {
    byte(c) != b'?' || c == '?' || c.is_whitespace() || !c.is_alphanumeric()
}

fn width(text: &str, font: Font, size: f32) -> f32 {
    let units: u32 = text
        .chars()
        .map(|c| {
            let b = byte(c);
            if font == Font::Mono {
                600
            } else if (32..=126).contains(&b) {
                let table = if matches!(font, Font::Bold | Font::BoldItalic) {
                    &HELVETICA_BOLD
                } else {
                    &HELVETICA
                };
                u32::from(table[usize::from(b - 32)])
            } else {
                556
            }
        })
        .sum();
    units as f32 * size / 1000.0
}

/// A PDF string literal of `text`.
fn literal(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        match byte(c) {
            b'(' => out.push_str("\\("),
            b')' => out.push_str("\\)"),
            b'\\' => out.push_str("\\\\"),
            b @ 32..=126 => out.push(char::from(b)),
            b => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push(')');
    out
}

/// One placed piece of a line.
struct Piece {
    text: String,
    font: Font,
}

struct Layout {
    pages: Vec<String>,
    y: f32,
}

impl Layout {
    fn new() -> Self {
        Self {
            pages: vec![String::new()],
            y: PAGE_H - MARGIN,
        }
    }

    fn page(&mut self) -> &mut String {
        if self.pages.is_empty() {
            self.pages.push(String::new());
        }
        let last = self.pages.len() - 1;
        &mut self.pages[last]
    }

    fn new_page(&mut self) {
        self.pages.push(String::new());
        self.y = PAGE_H - MARGIN;
    }

    /// Room for `height` more, or a new page.
    fn need(&mut self, height: f32) {
        if self.y - height < BOTTOM && self.y < PAGE_H - MARGIN - 1.0 {
            self.new_page();
        }
    }

    fn gap(&mut self, space: f32) {
        self.y -= space;
    }

    fn line_out(&mut self, x: f32, pieces: &[Piece], size: f32, color: &str) {
        let y = self.y;
        let mut cursor = x;
        let mut ops = format!("BT {color} rg ");
        for piece in pieces {
            ops.push_str(&format!(
                "/{} {size:.1} Tf 1 0 0 1 {cursor:.2} {y:.2} Tm {} Tj ",
                piece.font.name(),
                literal(&piece.text)
            ));
            cursor += width(&piece.text, piece.font, size);
        }
        ops.push_str("ET\n");
        self.page().push_str(&ops);
    }

    /// Wrap `spans` into lines within `max` points.
    fn wrap(spans: &[Span], size: f32, max: f32, force_bold: bool) -> Vec<Vec<Piece>> {
        let mut lines: Vec<Vec<Piece>> = vec![Vec::new()];
        let mut used = 0.0f32;
        for span in spans {
            let font = if force_bold {
                Font::of(span).bold()
            } else {
                Font::of(span)
            };
            // Words with the space before them, so spacing survives a span edge.
            let mut words: Vec<String> = Vec::new();
            for (i, part) in span.text.split(' ').enumerate() {
                words.push(if i == 0 {
                    part.to_owned()
                } else {
                    format!(" {part}")
                });
            }
            for word in words {
                if word.is_empty() {
                    continue;
                }
                let w = width(&word, font, size);
                if used + w > max && used > 0.0 {
                    lines.push(Vec::new());
                    used = 0.0;
                    let word = word.trim_start().to_owned();
                    let w = width(&word, font, size);
                    Self::push_word(&mut lines, word, font, size, max, &mut used, w);
                } else {
                    Self::push_word(&mut lines, word, font, size, max, &mut used, w);
                }
            }
        }
        lines
    }

    fn push_word(
        lines: &mut Vec<Vec<Piece>>,
        word: String,
        font: Font,
        size: f32,
        max: f32,
        used: &mut f32,
        w: f32,
    ) {
        // A word longer than a line is cut.
        if w > max {
            let mut part = String::new();
            for c in word.chars() {
                part.push(c);
                if width(&part, font, size) + *used > max && part.chars().count() > 1 {
                    part.pop();
                    if let Some(line) = lines.last_mut() {
                        line.push(Piece {
                            text: std::mem::take(&mut part),
                            font,
                        });
                    }
                    lines.push(Vec::new());
                    *used = 0.0;
                    part.push(c);
                }
            }
            *used += width(&part, font, size);
            if let Some(line) = lines.last_mut() {
                line.push(Piece { text: part, font });
            }
            return;
        }
        *used += w;
        if let Some(line) = lines.last_mut() {
            match line.last_mut() {
                Some(last) if last.font == font => last.text.push_str(&word),
                _ => line.push(Piece { text: word, font }),
            }
        }
    }

    fn text(&mut self, spans: &[Span], size: f32, x: f32, max: f32, bold: bool, color: &str) {
        let leading = size * 1.38;
        for line in Self::wrap(spans, size, max, bold) {
            self.need(leading);
            self.y -= leading;
            self.line_out(x, &line, size, color);
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: &str) {
        self.page()
            .push_str(&format!("{fill} rg {x:.2} {y:.2} {w:.2} {h:.2} re f\n"));
    }

    fn rule(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        self.page().push_str(&format!(
            "0.82 0.84 0.86 RG 0.6 w {x1:.2} {y1:.2} m {x2:.2} {y2:.2} l S\n"
        ));
    }
}

const TEXT: &str = "0.12 0.14 0.17";
const MUTED: &str = "0.29 0.33 0.39";

fn span(text: &str) -> Span {
    Span {
        text: text.to_owned(),
        ..Span::default()
    }
}

/// The PDF bytes.
pub fn write(title: &str, blocks: &[Block]) -> Vec<u8> {
    let mut l = Layout::new();
    let first_heading =
        matches!(blocks.first(), Some(Block::Heading(1, t)) if crate::same_title(&plain(t), title));
    if !title.is_empty() && !first_heading {
        l.text(&[span(title)], 22.0, MARGIN, TEXT_W, true, TEXT);
        l.gap(10.0);
    }
    for block in blocks {
        match block {
            Block::Heading(level, spans) => {
                let size = match level {
                    1 => 20.0,
                    2 => 15.0,
                    _ => 12.5,
                };
                l.gap(size * 0.6);
                // Keep a heading with the line after it.
                l.need(size * 1.4 + 30.0);
                l.text(spans, size, MARGIN, TEXT_W, true, TEXT);
                l.gap(4.0);
            }
            Block::Paragraph(spans) => {
                l.text(spans, 11.0, MARGIN, TEXT_W, false, TEXT);
                l.gap(7.0);
            }
            Block::List { ordered, items } => {
                for (n, item) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{}.", n + 1)
                    } else {
                        "•".to_owned()
                    };
                    let leading = 11.0 * 1.38;
                    for (k, line) in Layout::wrap(item, 11.0, TEXT_W - 18.0, false)
                        .iter()
                        .enumerate()
                    {
                        l.need(leading);
                        l.y -= leading;
                        // The marker goes with the item's first line.
                        if k == 0 {
                            let piece = Piece {
                                text: marker.clone(),
                                font: Font::Regular,
                            };
                            l.line_out(MARGIN + 4.0, &[piece], 11.0, MUTED);
                        }
                        l.line_out(MARGIN + 18.0, line, 11.0, TEXT);
                    }
                    l.gap(2.0);
                }
                l.gap(6.0);
            }
            Block::Code(code) => {
                let size = 9.5;
                let leading = size * 1.4;
                l.gap(2.0);
                for line in code.lines() {
                    let pieces = Layout::wrap(
                        &[Span {
                            text: line.to_owned(),
                            code: true,
                            ..Span::default()
                        }],
                        size,
                        TEXT_W - 16.0,
                        false,
                    );
                    for piece in pieces {
                        l.need(leading);
                        l.rect(MARGIN, l.y - leading, TEXT_W, leading, "0.95 0.96 0.97");
                        l.y -= leading;
                        l.line_out(MARGIN + 8.0, &piece, size, TEXT);
                    }
                }
                l.gap(10.0);
            }
            Block::Table(rows) => table(&mut l, rows),
            Block::Rule => {
                l.gap(6.0);
                l.need(8.0);
                let y = l.y;
                l.rule(MARGIN, y, PAGE_W - MARGIN, y);
                l.gap(10.0);
            }
        }
    }
    finish(title, &l.pages)
}

fn table(l: &mut Layout, rows: &[Vec<Vec<Span>>]) {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let col_w = TEXT_W / columns as f32;
    let size = 10.0;
    let leading = size * 1.35;
    l.gap(4.0);
    for (r, row) in rows.iter().enumerate() {
        let cells: Vec<Vec<Vec<Piece>>> = (0..columns)
            .map(|c| {
                Layout::wrap(
                    row.get(c).map_or(&[][..], Vec::as_slice),
                    size,
                    col_w - 10.0,
                    r == 0,
                )
            })
            .collect();
        let lines = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let height = lines as f32 * leading + 8.0;
        l.need(height);
        let top = l.y;
        if r == 0 {
            l.rect(MARGIN, top - height, TEXT_W, height, "0.95 0.96 0.97");
        }
        for (c, cell) in cells.iter().enumerate() {
            let x = MARGIN + c as f32 * col_w + 5.0;
            for (i, line) in cell.iter().enumerate() {
                l.y = top - 4.0 - (i as f32 + 1.0) * leading + 2.0;
                l.line_out(x, line, size, TEXT);
            }
        }
        l.y = top - height;
        l.rule(MARGIN, top, PAGE_W - MARGIN, top);
        l.rule(MARGIN, l.y, PAGE_W - MARGIN, l.y);
        for c in 0..=columns {
            let x = MARGIN + c as f32 * col_w;
            l.rule(x, top, x, top - height);
        }
    }
    l.gap(12.0);
}

/// The PDF file around the pages' content streams.
fn finish(title: &str, pages: &[String]) -> Vec<u8> {
    let pages: Vec<&String> = pages.iter().filter(|p| !p.is_empty()).collect();
    let pages: Vec<String> = if pages.is_empty() {
        vec![String::new()]
    } else {
        pages.into_iter().cloned().collect()
    };
    let total = pages.len();
    let mut objects: Vec<String> = Vec::new();
    // 1 catalog, 2 pages, 3..=7 fonts, 8 info, then page + content pairs.
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_owned());
    let kids: Vec<String> = (0..total).map(|i| format!("{} 0 R", 9 + 2 * i)).collect();
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {total} >>",
        kids.join(" ")
    ));
    for base in [
        "Helvetica",
        "Helvetica-Bold",
        "Helvetica-Oblique",
        "Helvetica-BoldOblique",
        "Courier",
    ] {
        objects.push(format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /{base} /Encoding /WinAnsiEncoding >>"
        ));
    }
    objects.push(format!(
        "<< /Title {} /Creator (Narrivy) /Producer (Narrivy) >>",
        literal(title)
    ));
    for (i, content) in pages.iter().enumerate() {
        let mut stream = content.clone();
        if total > 1 {
            let number = format!("{} / {total}", i + 1);
            let x = PAGE_W / 2.0 - width(&number, Font::Regular, 8.5) / 2.0;
            stream.push_str(&format!(
                "BT {MUTED} rg /F1 8.5 Tf 1 0 0 1 {x:.2} {:.2} Tm {} Tj ET\n",
                MARGIN / 2.0,
                literal(&number)
            ));
        }
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R /F4 6 0 R /F5 7 0 R >> >> /Contents {} 0 R >>",
            10 + 2 * i
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}endstream",
            stream.len()
        ));
    }
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (n, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", n + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 8 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_escaped_and_encoded() {
        assert_eq!(literal("a(b)\\ é • ي"), "(a\\(b\\)\\\\ \\351 \\225 ?)");
    }

    #[test]
    fn long_text_wraps_and_flows_onto_new_pages() {
        let long = "word ".repeat(4000);
        let pdf = write("Long", &crate::parse(&long));
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count "));
        assert!(!text.contains("/Count 1 "));
        assert!(text.trim_end().ends_with("%%EOF"));
    }

    #[test]
    fn widths_follow_the_font() {
        assert!(width("WWW", Font::Bold, 10.0) > width("iii", Font::Regular, 10.0));
        assert!((width("abc", Font::Mono, 10.0) - 18.0).abs() < 0.01);
    }
}
