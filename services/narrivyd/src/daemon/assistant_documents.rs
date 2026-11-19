//! Documents the assistant makes in a chat (D55): Word, PDF, slides and
//! Markdown, written by `narrivy-documents` (no other program). Each is kept
//! with its chat in Narrivy's own storage (`assistant/documents/<chat>/`),
//! so making one asks nothing; the app shows it as a card to preview,
//! download (a copy in the Downloads folder) or open. Deleting the chat
//! deletes them. Nothing about a document is logged.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use narrivy_documents::{Format, pdf_missing, render};
use serde::{Deserialize, Serialize};

use super::assistant_builtin::Outcome;
use super::assistant_tools::Tool;

/// The built-in tool server's id (always offered when a document is asked
/// for, or the chat already has one).
pub const DOCUMENTS: &str = "documents";
/// The most documents one chat keeps; the oldest go first.
const MAX_PER_CHAT: usize = 100;
/// How much of the Markdown a preview carries.
const PREVIEW_BYTES: usize = 64 * 1024;

/// What is kept beside each document.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentMeta {
    pub id: String,
    pub chat: String,
    pub title: String,
    /// `docx`, `pdf`, `pptx` or `md`.
    pub format: String,
    /// The name it is downloaded as.
    pub name: String,
    pub size: u64,
    pub created_at: u64,
    pub markdown: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn root() -> Result<PathBuf, &'static str> {
    super::host::state_dir()
        .map(|dir| dir.join("assistant").join("documents"))
        .map_err(|_| "document_storage_unavailable")
}

fn chat_dir(root: &Path, chat: &str) -> Result<PathBuf, &'static str> {
    if !valid_id(chat) {
        return Err("document_bad_id");
    }
    Ok(root.join(chat))
}

/// The tool the model is offered.
#[must_use]
pub fn tools() -> Vec<Tool> {
    vec![Tool {
        server_id: DOCUMENTS.to_owned(),
        server_name: "Documents".to_owned(),
        name: "make_document".to_owned(),
        description: "Make a file the user can download: a Word document (docx), a PDF (pdf), \
a slide presentation (pptx) or Markdown (md). Write the whole content in Markdown: # headings, \
paragraphs, - and 1. lists, **bold**, *italic*, | tables |, ``` code ```. For slides, each ## \
heading starts a new slide (keep 3 to 6 short bullet points per slide); --- also starts one. The \
file shows in the chat as a card; do not repeat its content in your answer. To change a document, \
make it again with the changes."
            .to_owned(),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "format": {"type": "string", "enum": ["docx", "pdf", "pptx", "md"], "description": "The file type"},
                "title": {"type": "string", "description": "The document's title, also its file name"},
                "content": {"type": "string", "description": "The whole document in Markdown"}
            },
            "required": ["format", "title", "content"]
        }),
    }]
}

/// Whether a question asks for a document, so the tool is offered.
#[must_use]
pub fn wanted(question: &str) -> bool {
    let q = question.to_lowercase();
    let formats = [
        "docx",
        ".doc",
        "word doc",
        "word file",
        "pdf",
        "pptx",
        "powerpoint",
        "slides",
        "slide deck",
        "presentation",
        "deck",
        "markdown file",
        ".md",
    ];
    let makes = [
        "document", "report", "file", "handout", "brochure", "letter", "proposal", "cv", "resume",
        "résumé",
    ];
    let verbs = [
        "make", "create", "write", "generate", "draft", "prepare", "produce", "turn", "convert",
        "export", "save", "put", "give me", "build",
    ];
    formats.iter().any(|f| q.contains(f))
        || (makes.iter().any(|m| q.contains(m)) && verbs.iter().any(|v| q.contains(v)))
}

/// The format a question asks for: slides, PDF, Markdown, else Word.
#[must_use]
pub fn format_asked(question: &str) -> Format {
    let q = question.to_lowercase();
    if ["slide", "presentation", "pptx", "powerpoint", "deck"]
        .iter()
        .any(|w| q.contains(w))
    {
        Format::Pptx
    } else if q.contains("pdf") {
        Format::Pdf
    } else if q.contains("markdown") || q.contains(".md") {
        Format::Markdown
    } else {
        Format::Docx
    }
}

/// A title for a document made from a reply: its first heading, else a
/// "titled …" in the question, else the question's first words.
#[must_use]
pub fn title_for(question: &str, reply: &str) -> String {
    if let Some(heading) = reply
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim().trim_matches('*').trim())
        .filter(|h| !h.is_empty())
    {
        return heading.chars().take(80).collect();
    }
    let lower = question.to_lowercase();
    for marker in ["titled ", "called ", "named "] {
        if let Some(at) = lower.find(marker) {
            let rest = &question[at + marker.len()..];
            let end = rest
                .find([',', '.', '\n', '(', ':'])
                .map_or(rest.len(), |e| e.min(rest.len()));
            let words: Vec<&str> = rest[..end].split_whitespace().take(8).collect();
            let mut title = words.join(" ");
            // "… with an agenda": the title stops before "with".
            if let Some(cut) = title.find(" with ") {
                title.truncate(cut);
            }
            let title = title.trim_matches(['"', '\'', '“', '”']).trim().to_owned();
            if !title.is_empty() {
                return title;
            }
        }
    }
    let words: Vec<&str> = question.split_whitespace().take(6).collect();
    words.join(" ").chars().take(60).collect()
}

/// When the user asked for a file and the model wrote the content into
/// the chat instead of calling `make_document` (small models do), the
/// reply is made into the document: the user gets the file they asked
/// for. `None` when the reply is too short to be a document.
#[must_use]
pub fn from_reply(chat: &str, question: &str, reply: &str) -> Option<(serde_json::Value, Outcome)> {
    let content = reply.trim();
    if !looks_like_document(content) {
        return None;
    }
    // A last line pointing at other apps is not part of the document.
    let content: Vec<&str> = content
        .lines()
        .filter(|l| {
            let l = l.to_lowercase();
            !(l.contains("switch on") || l.contains("paste this") || l.contains("copy this"))
        })
        .collect();
    let content = content.join("\n");
    let mut format = format_asked(question);
    if format == Format::Pdf && !pdf_missing(&content).is_empty() {
        format = Format::Docx;
    }
    let arguments = serde_json::json!({
        "format": format.extension(),
        "title": title_for(question, &content),
        "content": content,
    });
    let outcome = make(chat, &arguments);
    Some((arguments, outcome))
}

/// What the model is told when it answered a file request without making
/// the file.
pub const NUDGE: &str = "You have not made the file yet: nothing was created. Call make_document \
now with the format asked for, a title and the full content in Markdown.";

/// Whether text is document content, not a sentence about one: headings,
/// lists or a table, or long prose.
#[must_use]
pub fn looks_like_document(text: &str) -> bool {
    let text = text.trim();
    let structured = text
        .lines()
        .filter(|l| {
            let l = l.trim_start();
            l.starts_with('#')
                || l.starts_with("- ")
                || l.starts_with("* ")
                || l.starts_with('|')
                || l.split_once(". ").is_some_and(|(n, _)| {
                    !n.is_empty() && n.len() < 3 && n.bytes().all(|b| b.is_ascii_digit())
                })
        })
        .count();
    (structured >= 2 && text.chars().count() >= 50) || text.chars().count() >= 600
}

/// Whether the chat already has documents (follow-up changes need the tool).
#[must_use]
pub fn has_any(chat: &str) -> bool {
    root().is_ok_and(|r| has_any_in(&r, chat))
}

fn has_any_in(root: &Path, chat: &str) -> bool {
    chat_dir(root, chat)
        .is_ok_and(|dir| std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()))
}

/// What the model is told when documents are offered.
#[must_use]
pub fn guidance() -> &'static str {
    "You have the make_document tool: when the user asks for a document, report, letter, PDF, Word \
file or presentation, call make_document with the full content in Markdown. Never tell the user to \
copy text into Word or another app: the file you make can be downloaded and opened directly. After the \
call, answer in one or two sentences saying what you made. "
}

/// A file name for `title`: no path or reserved characters, not too long.
fn file_name(title: &str, format: Format) -> String {
    let mut name: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    name = name.trim_matches('.').trim().chars().take(80).collect();
    if name.is_empty() {
        name = "Document".to_owned();
    }
    format!("{name}.{}", format.extension())
}

/// Make a document for `chat` from the model's call.
#[must_use]
pub fn make(chat: &str, arguments: &serde_json::Value) -> Outcome {
    match root() {
        Ok(root) => make_in(&root, chat, arguments),
        Err(_) => failure("The document could not be saved (no storage)."),
    }
}

fn make_in(root: &Path, chat: &str, arguments: &serde_json::Value) -> Outcome {
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let Some(format) = Format::from_name(&text("format")) else {
        return failure("Unknown format: use docx, pdf, pptx or md.");
    };
    let title = text("title").trim().chars().take(200).collect::<String>();
    let content = text("content");
    if format == Format::Pdf {
        let missing = pdf_missing(&content);
        if !missing.is_empty() {
            return failure(
                "A PDF cannot show this text's script (for example Urdu or Arabic letters). \
Make it as a Word document (docx) or slides (pptx) instead, which keep it.",
            );
        }
    }
    let bytes = match render(format, &title, &content) {
        Ok(bytes) => bytes,
        Err(narrivy_documents::DocumentError::Empty) => {
            return failure("The document is empty: write its content in Markdown.");
        }
        Err(narrivy_documents::DocumentError::TooLarge) => {
            return failure(
                "The document is too long (over 400 KB of text): make it shorter or split it.",
            );
        }
    };
    // Opened again and read back before it is shown: nothing cut or lost.
    let checked = narrivy_documents::check(format, &bytes, &narrivy_documents::parse(&content));
    if !checked.missing.is_empty() {
        return failure(&format!(
            "The file came out incomplete (missing: {}). Make it again, shorter or in another format.",
            checked
                .missing
                .join("; ")
                .chars()
                .take(300)
                .collect::<String>()
        ));
    }
    let units = match (format, checked.units) {
        (_, 0) => String::new(),
        (Format::Pptx, 1) => "1 slide, ".to_owned(),
        (Format::Pptx, n) => format!("{n} slides, "),
        (_, 1) => "1 page, ".to_owned(),
        (_, n) => format!("{n} pages, "),
    };
    match keep(root, chat, format, &title, &content, &bytes) {
        Ok(meta) => Outcome {
            text: format!(
                "Made {} ({units}{} KB), checked: all of its content is in the file. It shows in the chat as a card with Download and Open; say in a sentence what it contains.",
                meta.name,
                meta.size.div_ceil(1024)
            ),
            failed: false,
            detail: Some(serde_json::json!({
                "kind": "document",
                "id": meta.id,
                "chat": meta.chat,
                "title": meta.title,
                "format": meta.format,
                "name": meta.name,
                "size": meta.size,
                "units": checked.units,
            })),
            picture: None,
        },
        Err(_) => failure("The document could not be saved (no room or no access)."),
    }
}

fn failure(text: &str) -> Outcome {
    Outcome {
        text: text.to_owned(),
        failed: true,
        detail: None,
        picture: None,
    }
}

fn keep(
    root: &Path,
    chat: &str,
    format: Format,
    title: &str,
    markdown: &str,
    bytes: &[u8],
) -> Result<DocumentMeta, &'static str> {
    let dir = chat_dir(root, chat)?;
    super::host::fs::dir_builder(0o700)
        .create(&dir)
        .map_err(|_| "document_storage_unavailable")?;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let created_at = now_ms();
    let id = format!("doc-{created_at:x}-{n}");
    let meta = DocumentMeta {
        id: id.clone(),
        chat: chat.to_owned(),
        title: if title.is_empty() {
            "Document".to_owned()
        } else {
            title.to_owned()
        },
        format: format.extension().to_owned(),
        name: file_name(title, format),
        size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        created_at,
        markdown: markdown.to_owned(),
    };
    write_private(&dir.join(format!("{id}.{}", format.extension())), bytes)?;
    let json = serde_json::to_vec(&meta).map_err(|_| "document_storage_unavailable")?;
    write_private(&dir.join(format!("{id}.json")), &json)?;
    prune(&dir);
    Ok(meta)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    let part = path.with_extension("part");
    std::fs::write(&part, bytes).map_err(|_| "document_storage_unavailable")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&part, path).map_err(|_| "document_storage_unavailable")
}

fn list_in(dir: &Path) -> Vec<DocumentMeta> {
    let mut all: Vec<DocumentMeta> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    all.sort_by_key(|m: &DocumentMeta| m.created_at);
    all
}

/// The oldest documents beyond [`MAX_PER_CHAT`] are removed.
fn prune(dir: &Path) {
    let all = list_in(dir);
    for old in all.iter().take(all.len().saturating_sub(MAX_PER_CHAT)) {
        remove(dir, old);
    }
}

fn remove(dir: &Path, meta: &DocumentMeta) {
    let _ = std::fs::remove_file(dir.join(format!("{}.{}", meta.id, meta.format)));
    let _ = std::fs::remove_file(dir.join(format!("{}.json", meta.id)));
}

/// Remove every document of a deleted chat.
pub fn delete_chat(chat: &str) {
    if let Ok(root) = root() {
        delete_chat_in(&root, chat);
    }
}

fn delete_chat_in(root: &Path, chat: &str) {
    if let Ok(dir) = chat_dir(root, chat) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn find(root: &Path, chat: &str, id: &str) -> Result<(PathBuf, DocumentMeta), &'static str> {
    if !valid_id(id) {
        return Err("document_bad_id");
    }
    let dir = chat_dir(root, chat)?;
    let meta: DocumentMeta = std::fs::read(dir.join(format!("{id}.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or("document_not_found")?;
    let file = dir.join(format!("{id}.{}", meta.format));
    if !file.is_file() {
        return Err("document_not_found");
    }
    Ok((file, meta))
}

/// The user's Downloads folder (the home folder when there is none).
fn downloads() -> PathBuf {
    #[cfg(not(windows))]
    {
        let named = std::process::Command::new("xdg-user-dir")
            .arg("DOWNLOAD")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| p.is_absolute() && p.is_dir());
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        named.unwrap_or_else(|| {
            let d = home.join("Downloads");
            if d.is_dir() { d } else { home }
        })
    }
    #[cfg(windows)]
    {
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_default();
        let d = home.join("Downloads");
        if d.is_dir() { d } else { home }
    }
}

/// `name` in `folder`, with ` (2)`, ` (3)` … when it is taken.
fn free_name(folder: &Path, name: &str) -> PathBuf {
    let path = folder.join(name);
    if !path.exists() {
        return path;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    for n in 2..1000 {
        let candidate = folder.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    folder.join(format!("{stem} ({}).{ext}", now_ms()))
}

/// An `AssistantLibrary` request about documents (`kind` `document`):
/// `list` (id: the chat), `get`, `download`, `open` and `show` (id:
/// `<chat>/<document>`). Replies with JSON.
///
/// # Errors
/// A short code.
pub fn handle(op: &str, id: Option<&str>, value: Option<&str>) -> Result<String, &'static str> {
    handle_in(&root()?, op, id, value)
}

fn handle_in(
    root: &Path,
    op: &str,
    id: Option<&str>,
    value: Option<&str>,
) -> Result<String, &'static str> {
    let id = id.ok_or("document_bad_id")?;
    if op == "list" {
        let dir = chat_dir(root, id)?;
        let list: Vec<serde_json::Value> = list_in(&dir)
            .into_iter()
            .map(|m| {
                serde_json::json!({"id": m.id, "chat": m.chat, "title": m.title, "format": m.format,
                    "name": m.name, "size": m.size, "createdAt": m.created_at})
            })
            .collect();
        return serde_json::to_string(&list).map_err(|_| "document_storage_unavailable");
    }
    let (chat, doc) = id.split_once('/').ok_or("document_bad_id")?;
    let (file, meta) = find(root, chat, doc)?;
    match op {
        "get" => {
            let mut markdown = meta.markdown.clone();
            if markdown.len() > PREVIEW_BYTES {
                let mut cut = PREVIEW_BYTES;
                while !markdown.is_char_boundary(cut) {
                    cut -= 1;
                }
                markdown.truncate(cut);
                markdown.push_str("\n\n…");
            }
            Ok(
                serde_json::json!({"id": meta.id, "chat": meta.chat, "title": meta.title,
                "format": meta.format, "name": meta.name, "size": meta.size,
                "createdAt": meta.created_at, "markdown": markdown})
                .to_string(),
            )
        }
        "download" => {
            // A folder the user chose, else Downloads.
            let folder = value
                .map(PathBuf::from)
                .filter(|p| p.is_absolute() && p.is_dir())
                .unwrap_or_else(downloads);
            let target = free_name(&folder, &meta.name);
            std::fs::copy(&file, &target).map_err(|_| "document_download_failed")?;
            // A download is the user's ordinary file, not Narrivy's private copy.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644));
            }
            Ok(serde_json::json!({"path": target.display().to_string()}).to_string())
        }
        "open" => {
            if super::assistant_builtin::open_path(&file.display().to_string(), false) {
                Ok("{}".to_owned())
            } else {
                Err("document_open_failed")
            }
        }
        "show" => {
            // The downloaded copy when there is one (the value), else the kept file.
            let target = value
                .map(PathBuf::from)
                .filter(|p| p.is_absolute() && p.is_file())
                .unwrap_or(file);
            if super::assistant_builtin::open_path(&target.display().to_string(), true) {
                Ok("{}".to_owned())
            } else {
                Err("document_open_failed")
            }
        }
        _ => Err("assistant_library_bad_request"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_requests_are_recognised() {
        assert!(wanted("Make a PDF of these notes"));
        assert!(wanted("can you turn this into slides?"));
        assert!(wanted("Write a report on our Q3 sales"));
        assert!(wanted("draft a cover letter as a word file"));
        assert!(!wanted("What time is it in Lahore?"));
        assert!(!wanted("explain this error"));
    }

    #[test]
    fn the_format_and_title_come_from_the_question_and_reply() {
        assert_eq!(format_asked("make slides about it"), Format::Pptx);
        assert_eq!(format_asked("as a PDF please"), Format::Pdf);
        assert_eq!(format_asked("write a report"), Format::Docx);
        assert_eq!(title_for("q", "intro\n### **Team Plan**\nx"), "Team Plan");
        assert_eq!(
            title_for(
                "Make a Word document titled Team offsite plan with an agenda",
                "no heading"
            ),
            "Team offsite plan"
        );
        assert_eq!(
            title_for("Write a report on sales", "plain"),
            "Write a report on sales"
        );
    }

    #[test]
    fn a_claim_is_not_a_document() {
        assert!(!looks_like_document(
            "I've created a one-page PDF summary of the benefits of daily standups, including a heading and 4 bullet points."
        ));
        assert!(looks_like_document(
            "# Standups\n\n- Focus the day\n- Surface blockers early\n- Short and regular"
        ));
        assert!(
            from_reply(
                "chat-x",
                "make a pdf",
                "I made it for you, it has four points."
            )
            .is_none()
        );
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(
            file_name("Q3: sales/plan?", Format::Docx),
            "Q3 sales plan.docx"
        );
        assert_eq!(file_name("  ..  ", Format::Pdf), "Document.pdf");
        assert_eq!(file_name(&"x".repeat(200), Format::Pptx).len(), 85);
    }

    #[test]
    fn a_taken_name_gets_a_number() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("a.pdf"), b"x")?;
        assert_eq!(free_name(dir.path(), "a.pdf"), dir.path().join("a (2).pdf"));
        assert_eq!(free_name(dir.path(), "b.pdf"), dir.path().join("b.pdf"));
        Ok(())
    }

    #[test]
    fn a_pdf_in_urdu_is_refused_with_what_to_do_instead() {
        let root = Path::new("/nonexistent/narrivy-documents");
        let out = make_in(
            root,
            "chat-x",
            &serde_json::json!({"format": "pdf", "title": "t", "content": "یہ اردو ہے"}),
        );
        assert!(out.failed);
        assert!(out.text.contains("docx"));
    }

    #[test]
    fn bad_formats_and_ids_are_refused() {
        let root = Path::new("/nonexistent/narrivy-documents");
        let out = make_in(
            root,
            "chat-x",
            &serde_json::json!({"format": "exe", "title": "t", "content": "x"}),
        );
        assert!(out.failed);
        assert_eq!(
            handle_in(root, "get", Some("../x/y"), None),
            Err("document_bad_id")
        );
        assert_eq!(
            handle_in(root, "list", Some("a/b"), None),
            Err("document_bad_id")
        );
    }

    #[test]
    fn a_document_is_kept_listed_downloaded_and_deleted_with_its_chat() -> std::io::Result<()> {
        let state = tempfile::tempdir()?;
        let root = state.path();
        let downloads = tempfile::tempdir()?;
        let chat = "chat-docs";
        let out = make_in(
            root,
            chat,
            &serde_json::json!({"format": "docx", "title": "Plan", "content": "# Plan\n\n- a\n- b"}),
        );
        assert!(!out.failed, "{}", out.text);
        let detail = out.detail.unwrap_or_default();
        let id = detail["id"].as_str().unwrap_or_default().to_owned();
        assert_eq!(detail["name"], "Plan.docx");
        let list: Vec<serde_json::Value> = serde_json::from_str(
            &handle_in(root, "list", Some(chat), None).map_err(std::io::Error::other)?,
        )?;
        assert_eq!(list.len(), 1);
        let got: serde_json::Value = serde_json::from_str(
            &handle_in(root, "get", Some(&format!("{chat}/{id}")), None)
                .map_err(std::io::Error::other)?,
        )?;
        assert_eq!(got["markdown"], "# Plan\n\n- a\n- b");
        let folder = downloads.path().display().to_string();
        for expected in ["Plan.docx", "Plan (2).docx"] {
            let saved: serde_json::Value = serde_json::from_str(
                &handle_in(
                    root,
                    "download",
                    Some(&format!("{chat}/{id}")),
                    Some(&folder),
                )
                .map_err(std::io::Error::other)?,
            )?;
            assert!(
                saved["path"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with(expected)
            );
        }
        assert!(std::fs::read(downloads.path().join("Plan.docx"))?.starts_with(b"PK"));
        assert!(has_any_in(root, chat));
        delete_chat_in(root, chat);
        assert!(!has_any_in(root, chat));
        Ok(())
    }
}
