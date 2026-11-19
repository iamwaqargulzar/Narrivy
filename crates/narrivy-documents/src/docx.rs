//! Word documents (`.docx`, Office Open XML WordprocessingML).

use crate::xml::escape;
use crate::{Block, Span, plain, right_to_left, zip};

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#;

const DOCUMENT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/></Relationships>"#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Arial"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="240"/></w:pPr><w:rPr><w:b/><w:color w:val="1F2937"/><w:sz w:val="48"/><w:szCs w:val="48"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="360" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:color w:val="1F2937"/><w:sz w:val="36"/><w:szCs w:val="36"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="280" w:after="100"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:color w:val="374151"/><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:color w:val="4B5563"/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="60"/><w:ind w:left="720"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:pPr><w:shd w:val="clear" w:color="auto" w:fill="F3F4F6"/><w:spacing w:after="160" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="19"/></w:rPr></w:style><w:style w:type="table" w:styleId="Grid"><w:name w:val="Table Grid"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="D1D5DB"/><w:left w:val="single" w:sz="4" w:space="0" w:color="D1D5DB"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="D1D5DB"/><w:right w:val="single" w:sz="4" w:space="0" w:color="D1D5DB"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="D1D5DB"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="D1D5DB"/></w:tblBorders><w:tblCellMar><w:top w:w="60" w:type="dxa"/><w:left w:w="100" w:type="dxa"/><w:bottom w:w="60" w:type="dxa"/><w:right w:w="100" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style></w:styles>"#;

/// The `.docx` bytes.
pub fn write(title: &str, blocks: &[Block]) -> Vec<u8> {
    let mut body = String::new();
    // A title that is not already the first heading opens the document.
    let first_heading =
        matches!(blocks.first(), Some(Block::Heading(1, t)) if crate::same_title(&plain(t), title));
    if !title.is_empty() && !first_heading {
        body.push_str(&paragraph(
            "Title",
            &[Span {
                text: title.to_owned(),
                ..Span::default()
            }],
            None,
        ));
    }
    // Ordered lists restart at 1: each gets its own numbering instance.
    let mut ordered_lists = 0u32;
    for block in blocks {
        match block {
            Block::Heading(level, spans) => {
                let style = if *level == 1 && first_heading && body.is_empty() {
                    "Title".to_owned()
                } else {
                    format!("Heading{level}")
                };
                body.push_str(&paragraph(&style, spans, None));
            }
            Block::Paragraph(spans) => body.push_str(&paragraph("Normal", spans, None)),
            Block::List { ordered, items } => {
                let num = if *ordered {
                    ordered_lists += 1;
                    1 + ordered_lists
                } else {
                    1
                };
                for item in items {
                    body.push_str(&paragraph("ListParagraph", item, Some(num)));
                }
            }
            Block::Code(code) => {
                let mut runs = String::new();
                for (i, line) in code.lines().enumerate() {
                    if i > 0 {
                        runs.push_str("<w:r><w:br/></w:r>");
                    }
                    runs.push_str(&format!(
                        r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
                        escape(line)
                    ));
                }
                body.push_str(&format!(r#"<w:p><w:pPr><w:pStyle w:val="Code"/></w:pPr>{runs}</w:p>"#));
            }
            Block::Table(rows) => body.push_str(&table(rows)),
            Block::Rule => body.push_str(
                r#"<w:p><w:pPr><w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1" w:color="D1D5DB"/></w:pBdr></w:pPr></w:p>"#,
            ),
        }
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1300" w:right="1300" w:bottom="1300" w:left="1300" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    zip::write(&[
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes().to_vec()),
        ("_rels/.rels", ROOT_RELS.as_bytes().to_vec()),
        ("docProps/core.xml", core(title).into_bytes()),
        ("word/document.xml", document.into_bytes()),
        (
            "word/_rels/document.xml.rels",
            DOCUMENT_RELS.as_bytes().to_vec(),
        ),
        ("word/styles.xml", STYLES.as_bytes().to_vec()),
        ("word/numbering.xml", numbering(ordered_lists).into_bytes()),
    ])
}

fn core(title: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{}</dc:title><dc:creator>Narrivy</dc:creator></cp:coreProperties>"#,
        escape(title)
    )
}

/// Bullets are numbering 1; each ordered list `2..` restarts decimals.
fn numbering(ordered_lists: u32) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#,
    );
    for n in 0..ordered_lists {
        out.push_str(&format!(
            r#"<w:num w:numId="{}"><w:abstractNumId w:val="1"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride></w:num>"#,
            n + 2
        ));
    }
    out.push_str("</w:numbering>");
    out
}

fn paragraph(style: &str, spans: &[Span], num: Option<u32>) -> String {
    let rtl = right_to_left(&plain(spans));
    let mut p = format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/>"#);
    if let Some(num) = num {
        p.push_str(&format!(
            r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{num}"/></w:numPr>"#
        ));
    }
    if rtl {
        p.push_str("<w:bidi/>");
    }
    p.push_str("</w:pPr>");
    p.push_str(&runs(spans, rtl));
    p.push_str("</w:p>");
    p
}

fn runs(spans: &[Span], rtl: bool) -> String {
    let mut out = String::new();
    for span in spans {
        let mut props = String::new();
        if span.code {
            props.push_str(r#"<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:shd w:val="clear" w:color="auto" w:fill="F3F4F6"/>"#);
        }
        if span.bold {
            props.push_str("<w:b/><w:bCs/>");
        }
        if span.italic {
            props.push_str("<w:i/><w:iCs/>");
        }
        if rtl {
            props.push_str("<w:rtl/>");
        }
        let props = if props.is_empty() {
            String::new()
        } else {
            format!("<w:rPr>{props}</w:rPr>")
        };
        out.push_str(&format!(
            r#"<w:r>{props}<w:t xml:space="preserve">{}</w:t></w:r>"#,
            escape(&span.text)
        ));
    }
    out
}

fn table(rows: &[Vec<Vec<Span>>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    // 9306 twentieths of a point: the text width of an A4 page here.
    let width = 9306 / columns;
    let mut out = String::from(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/><w:tblW w:w="5000" w:type="pct"/></w:tblPr><w:tblGrid>"#,
    );
    for _ in 0..columns {
        out.push_str(&format!(r#"<w:gridCol w:w="{width}"/>"#));
    }
    out.push_str("</w:tblGrid>");
    for (r, row) in rows.iter().enumerate() {
        out.push_str("<w:tr>");
        if r == 0 {
            out.push_str("<w:trPr><w:tblHeader/></w:trPr>");
        }
        for c in 0..columns {
            let cell = row.get(c).cloned().unwrap_or_default();
            let cell: Vec<Span> = if r == 0 {
                cell.into_iter().map(|s| Span { bold: true, ..s }).collect()
            } else {
                cell
            };
            let shade = if r == 0 {
                r#"<w:shd w:val="clear" w:color="auto" w:fill="F3F4F6"/>"#
            } else {
                ""
            };
            out.push_str(&format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/>{shade}</w:tcPr><w:p><w:pPr><w:spacing w:after="0"/></w:pPr>{}</w:p></w:tc>"#,
                runs(&cell, right_to_left(&plain(&cell)))
            ));
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl><w:p/>");
    out
}
