//! Slides (`.pptx`, Office Open XML PresentationML), 16:9. Each slide has a
//! title and a body of text boxes Narrivy places itself, so it looks the
//! same in PowerPoint, LibreOffice and Keynote. A body too long for one
//! slide continues on the next.

use crate::xml::escape;
use crate::{Block, Span, plain, right_to_left, slides, zip};

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const SLIDE_W: i64 = 12_192_000;
const SLIDE_H: i64 = 6_858_000;
/// About how many body lines fit on a slide at the body size.
const LINES_PER_SLIDE: usize = 11;
/// About how many characters fit on a body line.
const CHARS_PER_LINE: usize = 88;

const THEME: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Narrivy"><a:themeElements><a:clrScheme name="Narrivy"><a:dk1><a:srgbClr val="1F2937"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="374151"/></a:dk2><a:lt2><a:srgbClr val="F3F4F6"/></a:lt2><a:accent1><a:srgbClr val="D4A24C"/></a:accent1><a:accent2><a:srgbClr val="2563EB"/></a:accent2><a:accent3><a:srgbClr val="16A34A"/></a:accent3><a:accent4><a:srgbClr val="DC2626"/></a:accent4><a:accent5><a:srgbClr val="7C3AED"/></a:accent5><a:accent6><a:srgbClr val="0891B2"/></a:accent6><a:hlink><a:srgbClr val="2563EB"/></a:hlink><a:folHlink><a:srgbClr val="7C3AED"/></a:folHlink></a:clrScheme><a:fontScheme name="Narrivy"><a:majorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Narrivy"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#;

/// The `.pptx` bytes.
pub fn write(title: &str, blocks: &[Block]) -> Vec<u8> {
    let mut deck = slides(blocks);
    // The title opens the deck unless the first slide already says it.
    let opens_with_title = deck
        .first()
        .is_some_and(|(t, _)| crate::same_title(&plain(t), title));
    if !title.is_empty() && !opens_with_title {
        deck.insert(
            0,
            (
                vec![Span {
                    text: title.to_owned(),
                    ..Span::default()
                }],
                Vec::new(),
            ),
        );
    }
    let mut pages: Vec<String> = Vec::new();
    for (n, (heading, body)) in deck.iter().enumerate() {
        if body.is_empty() {
            pages.push(title_slide(heading, n == 0));
            continue;
        }
        let parts = split(body);
        let count = parts.len();
        for (k, part) in parts.into_iter().enumerate() {
            let mut heading = heading.clone();
            if k > 0 && !heading.is_empty() {
                heading.push(Span {
                    text: format!(" ({}/{count})", k + 1),
                    ..Span::default()
                });
            }
            pages.push(content_slide(&heading, &part));
        }
    }
    if pages.is_empty() {
        pages.push(title_slide(&[], true));
    }
    let mut files: Vec<(String, Vec<u8>)> = vec![
        (
            "[Content_Types].xml".to_owned(),
            content_types(pages.len()).into_bytes(),
        ),
        ("_rels/.rels".to_owned(), ROOT_RELS.as_bytes().to_vec()),
        ("docProps/core.xml".to_owned(), core(title).into_bytes()),
        (
            "ppt/presentation.xml".to_owned(),
            presentation(pages.len()).into_bytes(),
        ),
        (
            "ppt/_rels/presentation.xml.rels".to_owned(),
            presentation_rels(pages.len()).into_bytes(),
        ),
        (
            "ppt/slideMasters/slideMaster1.xml".to_owned(),
            master().into_bytes(),
        ),
        (
            "ppt/slideMasters/_rels/slideMaster1.xml.rels".to_owned(),
            MASTER_RELS.as_bytes().to_vec(),
        ),
        (
            "ppt/slideLayouts/slideLayout1.xml".to_owned(),
            layout().into_bytes(),
        ),
        (
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels".to_owned(),
            LAYOUT_RELS.as_bytes().to_vec(),
        ),
        ("ppt/theme/theme1.xml".to_owned(), THEME.as_bytes().to_vec()),
    ];
    for (i, page) in pages.into_iter().enumerate() {
        files.push((format!("ppt/slides/slide{}.xml", i + 1), page.into_bytes()));
        files.push((
            format!("ppt/slides/_rels/slide{}.xml.rels", i + 1),
            SLIDE_RELS.as_bytes().to_vec(),
        ));
    }
    let refs: Vec<(&str, Vec<u8>)> = files.iter().map(|(n, d)| (n.as_str(), d.clone())).collect();
    zip::write(&refs)
}

/// How many lines a block takes, roughly.
fn lines(block: &Block) -> usize {
    let wrap = |text: &str| text.chars().count().div_ceil(CHARS_PER_LINE).max(1);
    match block {
        Block::Paragraph(s) | Block::Heading(_, s) => wrap(&plain(s)),
        Block::List { items, .. } => items.iter().map(|i| wrap(&plain(i))).sum(),
        Block::Code(code) => code.lines().count().max(1),
        Block::Table(rows) => rows.len(),
        Block::Rule => 1,
    }
}

/// A slide's body in parts that fit; lists and code are split by line.
fn split(body: &[Block]) -> Vec<Vec<Block>> {
    let mut parts: Vec<Vec<Block>> = vec![Vec::new()];
    let mut used = 0;
    let place = |block: Block, parts: &mut Vec<Vec<Block>>, used: &mut usize| {
        let n = lines(&block);
        if *used + n > LINES_PER_SLIDE && *used > 0 {
            parts.push(Vec::new());
            *used = 0;
        }
        *used += n;
        if let Some(last) = parts.last_mut() {
            last.push(block);
        }
    };
    for block in body {
        match block {
            Block::List { ordered, items } if lines(block) > LINES_PER_SLIDE => {
                for chunk in items.chunks(LINES_PER_SLIDE / 2) {
                    place(
                        Block::List {
                            ordered: *ordered,
                            items: chunk.to_vec(),
                        },
                        &mut parts,
                        &mut used,
                    );
                }
            }
            Block::Code(code) if lines(block) > LINES_PER_SLIDE => {
                let all: Vec<&str> = code.lines().collect();
                for chunk in all.chunks(LINES_PER_SLIDE) {
                    place(Block::Code(chunk.join("\n")), &mut parts, &mut used);
                }
            }
            Block::Table(rows) if rows.len() > LINES_PER_SLIDE => {
                let header = rows[0].clone();
                for chunk in rows[1..].chunks(LINES_PER_SLIDE - 2) {
                    let mut part = vec![header.clone()];
                    part.extend_from_slice(chunk);
                    place(Block::Table(part), &mut parts, &mut used);
                }
            }
            other => place(other.clone(), &mut parts, &mut used),
        }
    }
    parts.retain(|p| !p.is_empty());
    parts
}

fn runs(spans: &[Span], size: u32, bold_all: bool, color: &str) -> String {
    let mut out = String::new();
    for span in spans {
        let b = if span.bold || bold_all {
            r#" b="1""#
        } else {
            ""
        };
        let i = if span.italic { r#" i="1""# } else { "" };
        // Every run names its font, so one does not carry into the next.
        let font = if span.code {
            r#"<a:latin typeface="Consolas"/><a:cs typeface="Consolas"/>"#
        } else {
            r#"<a:latin typeface="+mn-lt"/><a:cs typeface="+mn-cs"/>"#
        };
        out.push_str(&format!(
            r#"<a:r><a:rPr lang="en-US" sz="{size}"{b}{i} dirty="0"><a:solidFill><a:srgbClr val="{color}"/></a:solidFill>{font}</a:rPr><a:t>{}</a:t></a:r>"#,
            escape(&span.text)
        ));
    }
    out
}

/// A paragraph: `attrs` go on `a:pPr` (with a leading space), `children`
/// inside it.
fn para(spans: &[Span], attrs: &str, children: &str, size: u32, bold: bool, color: &str) -> String {
    let rtl = if right_to_left(&plain(spans)) {
        r#" rtl="1" algn="r""#
    } else {
        ""
    };
    format!(
        r#"<a:p><a:pPr{attrs}{rtl}>{children}</a:pPr>{}<a:endParaRPr lang="en-US" sz="{size}" dirty="0"/></a:p>"#,
        runs(spans, size, bold, color)
    )
}

fn body_xml(blocks: &[Block], size: u32) -> String {
    let mut out = String::new();
    let space = r#"<a:spcBef><a:spcPts val="600"/></a:spcBef>"#;
    for block in blocks {
        match block {
            Block::Paragraph(s) => out.push_str(&para(
                s,
                "",
                &format!("{space}<a:buNone/>"),
                size,
                false,
                "374151",
            )),
            Block::Heading(_, s) => out.push_str(&para(
                s,
                "",
                &format!("{space}<a:buNone/>"),
                size + 200,
                true,
                "1F2937",
            )),
            Block::List { ordered, items } => {
                let bullet = if *ordered {
                    r#"<a:buFont typeface="+mj-lt"/><a:buAutoNum type="arabicPeriod"/>"#
                } else {
                    r#"<a:buFont typeface="Arial"/><a:buChar char="•"/>"#
                };
                for item in items {
                    out.push_str(&para(
                        item,
                        r#" marL="342900" indent="-342900""#,
                        &format!("{space}{bullet}"),
                        size,
                        false,
                        "374151",
                    ));
                }
            }
            Block::Code(code) => {
                for line in code.lines() {
                    let span = Span {
                        text: line.to_owned(),
                        code: true,
                        ..Span::default()
                    };
                    out.push_str(&para(
                        &[span],
                        "",
                        "<a:buNone/>",
                        size * 3 / 4,
                        false,
                        "1F2937",
                    ));
                }
            }
            Block::Table(rows) => {
                for (r, row) in rows.iter().enumerate() {
                    let mut spans = Vec::new();
                    for (c, cell) in row.iter().enumerate() {
                        if c > 0 {
                            spans.push(Span {
                                text: "   ·   ".to_owned(),
                                ..Span::default()
                            });
                        }
                        spans.extend(cell.iter().cloned());
                    }
                    out.push_str(&para(
                        &spans,
                        "",
                        &format!("{space}<a:buNone/>"),
                        1700,
                        r == 0,
                        "374151",
                    ));
                }
            }
            Block::Rule => {}
        }
    }
    if out.is_empty() {
        out.push_str("<a:p><a:endParaRPr lang=\"en-US\" dirty=\"0\"/></a:p>");
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn shape(
    id: u32,
    name: &str,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    anchor: &str,
    paragraphs: &str,
) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="square" lIns="0" rIns="0" anchor="{anchor}"><a:normAutofit/></a:bodyPr><a:lstStyle/>{paragraphs}</p:txBody></p:sp>"#
    )
}

fn slide(shapes: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {NS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

/// The deck's design rules live here, not in the model's prompt (a small
/// model cannot follow a long style guide): white background, no
/// decorative shapes, left-aligned text, titles much larger than the body,
/// a body that never runs past its box (shrunk to fit, and split onto more
/// slides before that), and a few layouts chosen by what a slide holds.
const TITLE_SLIDE: u32 = 5400;
const SECTION_SLIDE: u32 = 4400;
const SLIDE_TITLE: u32 = 3600;
const BODY: u32 = 2000;
/// A short list or one short statement is shown larger.
const BODY_LARGE: u32 = 2600;

/// The opening slide, or a section slide: the title alone, large, in the
/// middle of the slide's height.
fn title_slide(heading: &[Span], first: bool) -> String {
    let size = if first { TITLE_SLIDE } else { SECTION_SLIDE };
    let x = 914_400;
    let text = para(heading, r#" algn="l""#, "<a:buNone/>", size, true, "1F2937");
    slide(&shape(
        3,
        "Title",
        x,
        1_900_000,
        SLIDE_W - 2 * x,
        3_000_000,
        "ctr",
        &text,
    ))
}

/// How a slide's body is laid out.
#[derive(Debug, PartialEq, Eq)]
enum Layout {
    /// One short paragraph: large, in the middle.
    Statement,
    /// Three or fewer short points: larger text.
    FewPoints,
    /// Anything else.
    Content,
}

fn layout_of(body: &[Block]) -> Layout {
    match body {
        [Block::Paragraph(spans)] if plain(spans).chars().count() <= 160 => Layout::Statement,
        [Block::List { items, .. }]
            if items.len() <= 3 && items.iter().all(|i| plain(i).chars().count() <= 70) =>
        {
            Layout::FewPoints
        }
        _ => Layout::Content,
    }
}

fn table_frame(id: u32, x: i64, y: i64, w: i64, rows: &[Vec<Vec<Span>>]) -> (String, i64) {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let col_w = w / i64::try_from(columns).unwrap_or(1);
    let row_h: i64 = 370_840;
    let grid: String = (0..columns)
        .map(|_| format!(r#"<a:gridCol w="{col_w}"/>"#))
        .collect();
    let mut body = String::new();
    for (r, row) in rows.iter().enumerate() {
        body.push_str(&format!(r#"<a:tr h="{row_h}">"#));
        for c in 0..columns {
            let cell = row.get(c).cloned().unwrap_or_default();
            let fill = if r == 0 { "F3F4F6" } else { "FFFFFF" };
            let border = r#"<a:solidFill><a:srgbClr val="D1D5DB"/></a:solidFill>"#;
            body.push_str(&format!(
                r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr marL="91440" marR="91440" marT="45720" marB="45720"><a:lnL w="9525">{border}</a:lnL><a:lnR w="9525">{border}</a:lnR><a:lnT w="9525">{border}</a:lnT><a:lnB w="9525">{border}</a:lnB><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill></a:tcPr></a:tc>"#,
                para(&cell, "", "<a:buNone/>", 1600, r == 0, "1F2937")
            ));
        }
        body.push_str("</a:tr>");
    }
    let height = row_h * i64::try_from(rows.len()).unwrap_or(1);
    (
        format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="{id}" name="Table {id}"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{height}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1"/><a:tblGrid>{grid}</a:tblGrid>{body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
        ),
        height,
    )
}

fn content_slide(heading: &[Span], body: &[Block]) -> String {
    let x = 685_800;
    let w = SLIDE_W - 2 * x;
    let title = para(heading, "", "<a:buNone/>", SLIDE_TITLE, true, "1F2937");
    let mut shapes = shape(3, "Title", x, 400_000, w, 1_000_000, "b", &title);
    match layout_of(body) {
        Layout::Statement => {
            let text = body_xml(body, BODY_LARGE);
            shapes.push_str(&shape(4, "Body", x, 1_700_000, w, 3_800_000, "ctr", &text));
            return slide(&shapes);
        }
        Layout::FewPoints => {
            let text = body_xml(body, BODY_LARGE);
            shapes.push_str(&shape(4, "Body", x, 1_700_000, w, 3_800_000, "t", &text));
            return slide(&shapes);
        }
        Layout::Content => {}
    }
    // Text runs in boxes; each table is a real table below the text before it.
    let mut y: i64 = 1_600_000;
    let mut id = 4;
    let mut text: Vec<Block> = Vec::new();
    let flush = |text: &mut Vec<Block>, shapes: &mut String, y: &mut i64, id: &mut u32| {
        if text.is_empty() {
            return;
        }
        let lines: usize = text.iter().map(lines).sum();
        let height =
            (i64::try_from(lines).unwrap_or(1) * 520_000 + 200_000).min(SLIDE_H - *y - 300_000);
        shapes.push_str(&shape(
            *id,
            "Body",
            x,
            *y,
            w,
            height.max(400_000),
            "t",
            &body_xml(text, BODY),
        ));
        *id += 1;
        *y += height + 80_000;
        text.clear();
    };
    for block in body {
        if let Block::Table(rows) = block {
            flush(&mut text, &mut shapes, &mut y, &mut id);
            let (frame, height) = table_frame(id, x, y + 60_000, w, rows);
            shapes.push_str(&frame);
            id += 1;
            y += height + 200_000;
        } else {
            text.push(block.clone());
        }
    }
    flush(&mut text, &mut shapes, &mut y, &mut id);
    slide(&shapes)
}

fn content_types(slides: usize) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>"#,
    );
    for n in 1..=slides {
        out.push_str(&format!(
            r#"<Override PartName="/ppt/slides/slide{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
        ));
    }
    out.push_str("</Types>");
    out
}

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#;

const MASTER_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/></Relationships>"#;

const LAYOUT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#;

const SLIDE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#;

const EMPTY_TREE: &str = r#"<p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr></p:spTree>"#;

fn master() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>{EMPTY_TREE}</p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="3200"/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:defRPr sz="2000"/></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#
    )
}

fn layout() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {NS} type="blank" preserve="1"><p:cSld name="Blank">{EMPTY_TREE}</p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
    )
}

fn presentation(slides: usize) -> String {
    let ids: String = (0..slides)
        .map(|i| format!(r#"<p:sldId id="{}" r:id="rId{}"/>"#, 256 + i, 3 + i))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation {NS} saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="{SLIDE_W}" cy="{SLIDE_H}"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
    )
}

fn presentation_rels(slides: usize) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>"#,
    );
    for i in 0..slides {
        out.push_str(&format!(
            r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{}.xml"/>"#,
            3 + i,
            1 + i
        ));
    }
    out.push_str("</Relationships>");
    out
}

fn core(title: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{}</dc:title><dc:creator>Narrivy</dc:creator></cp:coreProperties>"#,
        escape(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slides_use_a_layout_that_fits_what_they_hold_and_no_decoration() {
        assert_eq!(
            layout_of(&crate::parse("One clear idea.")),
            Layout::Statement
        );
        assert_eq!(layout_of(&crate::parse("- a\n- b\n- c")), Layout::FewPoints);
        assert_eq!(
            layout_of(&crate::parse("- a\n- b\n- c\n- d")),
            Layout::Content
        );
        let deck =
            String::from_utf8_lossy(&write("Deck", &crate::parse("## One\n- a\n- b"))).into_owned();
        assert!(!deck.contains("Accent"));
    }

    #[test]
    fn a_long_list_continues_on_more_slides() {
        let items: String = (0..30).map(|i| format!("- point {i}\n")).collect();
        let blocks = crate::parse(&format!("## Points\n{items}"));
        let parts = split(&slides(&blocks)[0].1);
        assert!(parts.len() >= 3);
        assert!(
            parts
                .iter()
                .all(|p| p.iter().map(lines).sum::<usize>() <= LINES_PER_SLIDE)
        );
    }
}
