//! The assistant's built-in tools (D35): web research and the computer.
//!
//! Switched on per chat, like an MCP server: `web` (search, read pages) and
//! `computer` (files, commands, opening things, looking at the screen).
//! Web pages are fetched by the system's `curl` (no proxy, credential or
//! configuration inherited), only over http(s), never from this computer or
//! the local network. What each step finds goes to the activity panel as
//! `detail`; nothing is logged.

use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use narrivy_process_linux::{HelperEnvironment, TrustedExecutable};

use super::assistant_tools::Tool;

pub const WEB: &str = "web";
pub const COMPUTER: &str = "computer";

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";
const PAGE_TEXT: usize = 7000;
const PAGE_BYTES: &str = "4000000";
const SEARCH_RESULTS: usize = 8;
const FILE_TEXT: usize = 7000;
const COMMAND_BOUND: Duration = Duration::from_secs(120);
/// How long sharing the screen once may take, the portal's dialog included.
const SCREEN_BOUND: Duration = Duration::from_secs(120);
const OUTPUT_TEXT: usize = 6000;
const LIST_ENTRIES: usize = 200;

/// How a call is approved.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// Switching the tools on for the chat was the approval.
    Never,
    /// Asked, unless allowed for the rest of the chat.
    Ask,
    /// Asked every time: it changes something on the computer.
    Always,
}

/// What a built-in tool returns: text for the model, failure, a detail for
/// the activity panel, and a picture for the model to look at.
pub struct Outcome {
    pub text: String,
    pub failed: bool,
    pub detail: Option<serde_json::Value>,
    pub picture: Option<PathBuf>,
}

fn outcome(text: String, detail: serde_json::Value) -> Outcome {
    Outcome {
        text,
        failed: false,
        detail: Some(detail),
        picture: None,
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

fn tool(
    server: &str,
    server_name: &str,
    name: &str,
    description: &str,
    schema: serde_json::Value,
) -> Tool {
    Tool {
        server_id: server.to_owned(),
        server_name: server_name.to_owned(),
        name: name.to_owned(),
        description: description.to_owned(),
        schema,
    }
}

fn props(fields: &[(&str, &str, &str)], required: &[&str]) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    for (name, kind, description) in fields {
        properties.insert(
            (*name).to_owned(),
            serde_json::json!({"type": kind, "description": description}),
        );
    }
    serde_json::json!({"type": "object", "properties": properties, "required": required})
}

/// The built-in tools of `server` (`web` or `computer`).
#[must_use]
pub fn tools(server: &str) -> Vec<Tool> {
    match server {
        WEB => vec![
            tool(
                WEB,
                "Web",
                "web_search",
                "Search the web. Returns titles, links and snippets. Search again with other words when the results are thin.",
                props(&[("query", "string", "What to search for")], &["query"]),
            ),
            tool(
                WEB,
                "Web",
                "open_page",
                "Read a web page: its title and text. Use it on the most relevant results before answering.",
                props(
                    &[("url", "string", "The page's address, http or https")],
                    &["url"],
                ),
            ),
        ],
        COMPUTER => vec![
            tool(
                COMPUTER,
                "Computer",
                "list_folder",
                "List a folder's files and folders (~ is the home folder).",
                props(&[("path", "string", "The folder")], &["path"]),
            ),
            tool(
                COMPUTER,
                "Computer",
                "find_files",
                "Find files whose names contain some text, under a folder.",
                props(
                    &[
                        ("folder", "string", "Where to look"),
                        ("name", "string", "Text the file name contains"),
                    ],
                    &["folder", "name"],
                ),
            ),
            tool(
                COMPUTER,
                "Computer",
                "read_file",
                "Read a text file.",
                props(&[("path", "string", "The file")], &["path"]),
            ),
            tool(
                COMPUTER,
                "Computer",
                "write_file",
                "Write a text file (created or replaced). The user approves it first. \
For a long file (more than about 200 lines) write only its first part here, then add the rest with append_file.",
                props(
                    &[
                        ("path", "string", "The file"),
                        ("content", "string", "The new text"),
                    ],
                    &["path", "content"],
                ),
            ),
            tool(
                COMPUTER,
                "Computer",
                "append_file",
                "Add text to the end of a file, to write a long file in parts of about 200 lines. The user approves it first.",
                props(
                    &[
                        ("path", "string", "The file"),
                        ("content", "string", "The text to add"),
                    ],
                    &["path", "content"],
                ),
            ),
            tool(
                COMPUTER,
                "Computer",
                "run_command",
                "Run a shell command in the working folder and return its output. The user approves it first.",
                props(&[("command", "string", "The bash command")], &["command"]),
            ),
            tool(
                COMPUTER,
                "Computer",
                "open",
                "Open a link, file or app with the desktop's default app. The user approves it first.",
                props(
                    &[(
                        "target",
                        "string",
                        "A URL, a file path, or an app's .desktop name",
                    )],
                    &["target"],
                ),
            ),
            tool(
                COMPUTER,
                "Computer",
                "view_page",
                "Open a web page or a local HTML file in a browser of your own (out of sight, \
nothing signed in) and get a picture of the whole page and the errors in its console. Use it to check pages you make.",
                props(
                    &[
                        (
                            "target",
                            "string",
                            "An http(s) address, or an HTML file's path",
                        ),
                        (
                            "width",
                            "integer",
                            "The window's width in pixels (default 1280; 390 to see it as on a phone)",
                        ),
                    ],
                    &["target"],
                ),
            ),
            tool(
                COMPUTER,
                "Computer",
                "look_at_screen",
                "See the screen: the user chooses which screen or window to share, and you get a picture of it.",
                props(&[], &[]),
            ),
        ],
        _ => Vec::new(),
    }
}

/// How `name` of `server` is approved.
#[must_use]
pub fn approval(server: &str, name: &str) -> Approval {
    match (server, name) {
        // Web research and looking at the screen run without asking (the
        // screen-sharing dialog is the user's choice of what to show).
        (WEB, _) | (COMPUTER, "look_at_screen") => Approval::Never,
        (COMPUTER, "write_file" | "append_file" | "run_command" | "open") => Approval::Always,
        _ => Approval::Ask,
    }
}

/// Run a built-in tool. `folder` is the chat's working folder: paths that
/// are not absolute are in it, and commands run there (the home folder when
/// none is set or it is gone).
pub fn call(
    server: &str,
    name: &str,
    arguments: &serde_json::Value,
    scratch: &Path,
    folder: Option<&Path>,
) -> Outcome {
    let base = folder
        .filter(|f| f.is_absolute() && f.is_dir())
        .map_or_else(home, Path::to_path_buf);
    let base = base.as_path();
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match (server, name) {
        (WEB, "web_search") => web_search(&text("query")),
        (WEB, "open_page") => open_page(&text("url")),
        (COMPUTER, "list_folder") => list_folder(&text("path"), base),
        (COMPUTER, "find_files") => find_files(&text("folder"), &text("name"), base),
        (COMPUTER, "read_file") => read_file(&text("path"), base),
        (COMPUTER, "write_file") => write_file(&text("path"), &text("content"), base),
        (COMPUTER, "append_file") => append_file(&text("path"), &text("content"), base),
        (COMPUTER, "run_command") => run_command(&text("command"), base),
        (COMPUTER, "open") => open(&text("target"), base),
        (COMPUTER, "look_at_screen") => look_at_screen(scratch),
        (COMPUTER, "view_page") => view_page(
            &text("target"),
            arguments.get("width").and_then(serde_json::Value::as_u64),
            scratch,
            base,
        ),
        _ => failure("There is no such tool."),
    }
}

// ------------------------------------------------------------------ web

fn curl() -> Option<TrustedExecutable> {
    TrustedExecutable::resolve_system(OsStr::new("curl")).ok()
}

/// A URL the assistant may fetch: http(s), and not this computer or the
/// local network.
fn public_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .trim_start_matches('[');
    let host = host
        .split(']')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if host.is_empty()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || !host.contains('.')
    {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified())
            }
            std::net::IpAddr::V6(_) => false,
        };
    }
    true
}

/// GET `url` with curl; the body (cut at a few MB) and the final URL.
fn fetch(url: &str) -> Result<(String, String), &'static str> {
    fetch_with(url, None, false)
}

/// [`fetch`] with request headers passed on stdin (so a key never shows in
/// the process list), and, for the user's own search server, private
/// addresses allowed.
fn fetch_with(
    url: &str,
    headers: Option<&str>,
    allow_private: bool,
) -> Result<(String, String), &'static str> {
    let allowed = |url: &str| {
        public_url(url)
            || (allow_private && (url.starts_with("http://") || url.starts_with("https://")))
    };
    if !allowed(url) {
        return Err("Only public http(s) pages can be opened.");
    }
    let curl = curl().ok_or("The download tool (curl) is missing.")?;
    let mut command = curl
        .command(HelperEnvironment::HttpsDownload)
        .map_err(|_| "The download tool (curl) is missing.")?;
    command
        .args([
            "-q",
            "-sS",
            "-L",
            "--max-redirs",
            "5",
            "--proto",
            "=https,http",
        ])
        .args([
            "--proto-redir",
            "=https,http",
            "--compressed",
            "--max-time",
            "25",
        ])
        .args(["--max-filesize", PAGE_BYTES, "-A", USER_AGENT])
        .args([
            "-H",
            "Accept: text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.5",
        ])
        .args(["-H", "Accept-Language: en-US,en;q=0.8"])
        .args(["-w", "\n\u{1}%{url_effective}\u{2}%{http_code}"]);
    if headers.is_some() {
        command.args(["-H", "@-"]).stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    command
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| "The download tool (curl) failed.")?;
    if let (Some(headers), Some(mut stdin)) = (headers, child.stdin.take()) {
        use std::io::Write as _;
        let _ = stdin.write_all(headers.as_bytes());
    }
    let mut body = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(6_000_000).read_to_end(&mut body);
    }
    let status = child.wait().map_err(|_| "The page could not be fetched.")?;
    if !status.success() {
        return Err("The page could not be fetched.");
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    let (page, tail) = text.rsplit_once("\n\u{1}").unwrap_or((&text, url));
    let (effective, code) = tail.split_once('\u{2}').unwrap_or((tail, "200"));
    if !allowed(effective) {
        return Err("The page redirected to a private address.");
    }
    match code.trim().parse::<u16>().unwrap_or(200) {
        404 | 410 => Err("The page does not exist (404)."),
        401 | 403 => Err("The site refused to show the page (it may block automated reading)."),
        429 => Err("The site asked to slow down (429); try another source."),
        code if code >= 400 => Err("The site answered with an error."),
        _ => Ok((page.to_owned(), effective.to_owned())),
    }
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_owned(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `query` without the version numbers and years that `question` does not
/// contain: those come from the model's memory, which is out of date.
pub fn unguessed(query: &str, question: &str) -> String {
    let guessed = |word: &str| {
        let core = word.trim_matches(|c: char| !c.is_alphanumeric());
        let digits = core.chars().filter(char::is_ascii_digit).count();
        let version = digits > 0
            && core.contains('.')
            && core
                .trim_start_matches(['v', 'V'])
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.');
        let year =
            core.len() == 4 && digits == 4 && (core.starts_with("19") || core.starts_with("20"));
        (version || year) && !question.contains(core)
    };
    let kept: Vec<&str> = query.split_whitespace().filter(|w| !guessed(w)).collect();
    if kept.is_empty() {
        query.to_owned()
    } else {
        kept.join(" ")
    }
}

fn web_search(query: &str) -> Outcome {
    let query = query.trim();
    if query.is_empty() {
        return failure("The search needs words.");
    }
    let settings = web_settings();
    let mut tried = Vec::new();
    for provider in providers(&settings) {
        let found = match provider {
            "brave" => brave_search(query),
            "searxng" => searxng_search(&settings.searxng, query),
            "duckduckgo" => duckduckgo_search(query),
            _ => wikipedia_search(query),
        };
        match found {
            Ok(results) if !results.is_empty() => {
                let text = results
                    .iter()
                    .enumerate()
                    .map(|(i, (title, link, snippet))| {
                        format!("{}. {title}\n{link}\n{snippet}", i + 1)
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let listed: Vec<serde_json::Value> = results
                    .iter()
                    .map(|(title, link, snippet)| serde_json::json!({"title": title, "url": link, "snippet": snippet}))
                    .collect();
                return outcome(
                    text,
                    serde_json::json!({"kind": "search", "query": query, "results": listed, "engine": provider}),
                );
            }
            _ => tried.push(provider),
        }
    }
    Outcome {
        text:
            "No results. The search engines may be limiting searches for a while; try other words, \
or open a page you know."
                .to_owned(),
        failed: true,
        detail: Some(
            serde_json::json!({"kind": "search", "query": query, "results": [], "tried": tried}),
        ),
        picture: None,
    }
}

type Results = Vec<(String, String, String)>;

/// Where searches go, in order: the user's choice, then the others.
fn providers(settings: &WebSettings) -> Vec<&'static str> {
    let mut order: Vec<&'static str> = Vec::new();
    let has_key = brave_key().is_some();
    let preferred = match settings.provider.as_str() {
        "brave" if has_key => Some("brave"),
        "searxng" if !settings.searxng.is_empty() => Some("searxng"),
        "duckduckgo" => Some("duckduckgo"),
        "wikipedia" => Some("wikipedia"),
        _ => None,
    };
    order.extend(preferred);
    for provider in ["brave", "searxng", "duckduckgo", "wikipedia"] {
        let usable = match provider {
            "brave" => has_key,
            "searxng" => !settings.searxng.is_empty(),
            _ => true,
        };
        if usable && !order.contains(&provider) {
            order.push(provider);
        }
    }
    order
}

fn brave_search(query: &str) -> Result<Results, &'static str> {
    let key = brave_key().ok_or("no key")?;
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={SEARCH_RESULTS}",
        percent_encode(query)
    );
    let headers = format!("Accept: application/json\nX-Subscription-Token: {key}\n");
    let (body, _) = fetch_with(&url, Some(&headers), false)?;
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|_| "bad answer")?;
    Ok(value
        .pointer("/web/results")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|r| {
                    Some((
                        collapse(&decode_entities(&strip_tags(r.get("title")?.as_str()?))),
                        r.get("url")?.as_str()?.to_owned(),
                        collapse(&decode_entities(&strip_tags(
                            r.get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or_default(),
                        ))),
                    ))
                })
                .take(SEARCH_RESULTS)
                .collect()
        })
        .unwrap_or_default())
}

fn searxng_search(base: &str, query: &str) -> Result<Results, &'static str> {
    let url = format!(
        "{}/search?q={}&format=json",
        base.trim_end_matches('/'),
        percent_encode(query)
    );
    let (body, _) = fetch_with(&url, None, true)?;
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|_| "bad answer")?;
    Ok(value
        .get("results")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|r| {
                    Some((
                        r.get("title")?.as_str()?.to_owned(),
                        r.get("url")?.as_str()?.to_owned(),
                        r.get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                    ))
                })
                .take(SEARCH_RESULTS)
                .collect()
        })
        .unwrap_or_default())
}

fn duckduckgo_search(query: &str) -> Result<Results, &'static str> {
    for url in [
        format!(
            "https://lite.duckduckgo.com/lite/?q={}",
            percent_encode(query)
        ),
        format!(
            "https://html.duckduckgo.com/html/?q={}",
            percent_encode(query)
        ),
    ] {
        let Ok((page, _)) = fetch(&url) else {
            continue;
        };
        // A bot check instead of results.
        if page.contains("anomaly-modal") || page.contains("Unfortunately, bots use DuckDuckGo") {
            continue;
        }
        let results = if url.contains("lite.") {
            lite_results(&page)
        } else {
            search_results(&page)
        };
        if !results.is_empty() {
            return Ok(results);
        }
    }
    Err("blocked")
}

fn wikipedia_search(query: &str) -> Result<Results, &'static str> {
    let url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&list=search&format=json&srlimit={SEARCH_RESULTS}&srsearch={}",
        percent_encode(query)
    );
    let (body, _) = fetch(&url)?;
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|_| "bad answer")?;
    Ok(value
        .pointer("/query/search")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|r| {
                    let title = r.get("title")?.as_str()?.to_owned();
                    let link = format!(
                        "https://en.wikipedia.org/wiki/{}",
                        percent_encode(&title.replace(' ', "_")).replace('+', "_")
                    );
                    let snippet = collapse(&decode_entities(&strip_tags(
                        r.get("snippet")
                            .and_then(|s| s.as_str())
                            .unwrap_or_default(),
                    )));
                    Some((format!("{title} — Wikipedia"), link, snippet))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// (title, url, snippet) from DuckDuckGo's lite page.
fn lite_results(page: &str) -> Results {
    let mut results: Results = Vec::new();
    let mut rest = page;
    while let Some(at) = rest.find("result-link") {
        let before = &rest[..at];
        let tag_start = before.rfind("<a").unwrap_or(0);
        let tag = &rest[tag_start..];
        rest = &rest[at + "result-link".len()..];
        let href = tag
            .find("href=\"")
            .map(|h| &tag[h + 6..])
            .and_then(|h| h.split('"').next())
            .unwrap_or_default();
        let link = href
            .split("uddg=")
            .nth(1)
            .map(|r| percent_decode(r.split('&').next().unwrap_or_default()))
            .unwrap_or_else(|| decode_entities(href));
        let title = rest
            .split_once('>')
            .map(|(_, r)| r.split("</a>").next().unwrap_or_default())
            .map(|raw| collapse(&decode_entities(&strip_tags(raw))))
            .unwrap_or_default();
        let snippet = rest
            .split("result-snippet")
            .nth(1)
            .filter(|_| {
                // The snippet belongs to this result, not the next one.
                let next = rest.find("result-link").unwrap_or(usize::MAX);
                rest.find("result-snippet").is_some_and(|s| s < next)
            })
            .and_then(|r| r.split_once('>'))
            .map(|(_, r)| r.split("</td>").next().unwrap_or_default())
            .map(|raw| collapse(&decode_entities(&strip_tags(raw))))
            .unwrap_or_default();
        if link.starts_with("http") && !title.is_empty() && !link.contains("duckduckgo.com/y.js") {
            results.push((title, link, snippet));
        }
        if results.len() == SEARCH_RESULTS {
            break;
        }
    }
    results
}

// ------------------------------------------------------------------ settings

/// Where web searches go: `auto` (the best one available), `brave`,
/// `searxng`, `duckduckgo` or `wikipedia`; the user's SearXNG address.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WebSettings {
    pub provider: String,
    pub searxng: String,
}

const BRAVE_KEY: &str = "brave-search-key";

fn web_settings_path() -> Option<PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("web-research.json"))
}

fn web_settings() -> WebSettings {
    web_settings_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The Brave Search key, from the keyring.
#[must_use]
pub fn brave_key() -> Option<String> {
    narrivy_platform_linux::secrets::read(BRAVE_KEY)
        .ok()
        .flatten()
        .filter(|key| !key.trim().is_empty())
}

/// The web research settings for clients (never the key itself).
#[must_use]
pub fn web_settings_json() -> String {
    let settings = web_settings();
    serde_json::json!({
        "provider": if settings.provider.is_empty() { "auto" } else { settings.provider.as_str() },
        "searxng": settings.searxng,
        "braveKey": brave_key().is_some(),
    })
    .to_string()
}

/// Change the web research settings: `provider`, `searxng`, and a Brave
/// key to keep in the keyring (`braveKey`, empty to remove it).
///
/// # Errors
/// A short code when a value is not valid or cannot be kept.
pub fn set_web_settings(value: &str) -> Result<String, &'static str> {
    let change: serde_json::Value =
        serde_json::from_str(value).map_err(|_| "assistant_library_bad_value")?;
    let mut settings = web_settings();
    if let Some(provider) = change.get("provider").and_then(serde_json::Value::as_str) {
        if !matches!(
            provider,
            "auto" | "brave" | "searxng" | "duckduckgo" | "wikipedia"
        ) {
            return Err("assistant_library_bad_value");
        }
        settings.provider = provider.to_owned();
    }
    if let Some(url) = change.get("searxng").and_then(serde_json::Value::as_str) {
        let url = url.trim();
        if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("assistant_library_bad_value");
        }
        settings.searxng = url.to_owned();
    }
    if let Some(key) = change.get("braveKey").and_then(serde_json::Value::as_str) {
        let key = key.trim();
        let kept = if key.is_empty() {
            narrivy_platform_linux::secrets::remove(BRAVE_KEY)
        } else if key.len() > 200 || !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("assistant_library_bad_value");
        } else {
            narrivy_platform_linux::secrets::store(BRAVE_KEY, "Narrivy: Brave Search key", key)
        };
        kept.map_err(|error| match error {
            narrivy_platform_linux::secrets::SecretError::Locked => "keyring_locked",
            _ => "keyring_unavailable",
        })?;
    }
    let path = web_settings_path().ok_or("assistant_settings_unavailable")?;
    let bytes = serde_json::to_vec(&settings).map_err(|_| "assistant_settings_unavailable")?;
    std::fs::write(path, bytes).map_err(|_| "assistant_settings_unavailable")?;
    Ok(web_settings_json())
}

/// (title, url, snippet) from DuckDuckGo's HTML results.
fn search_results(page: &str) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    for block in page.split("class=\"result__a\"").skip(1) {
        let Some(href_at) = block.find("href=\"") else {
            continue;
        };
        let href = &block[href_at + 6..];
        let href = &href[..href.find('"').unwrap_or(0)];
        let link = href
            .split("uddg=")
            .nth(1)
            .map(|rest| percent_decode(rest.split('&').next().unwrap_or_default()))
            .unwrap_or_else(|| decode_entities(href));
        let title = block
            .split_once('>')
            .map(|(_, rest)| rest.split("</a>").next().unwrap_or_default())
            .map(|raw| collapse(&decode_entities(&strip_tags(raw))))
            .unwrap_or_default();
        let snippet = block
            .split("class=\"result__snippet\"")
            .nth(1)
            .and_then(|rest| rest.split_once('>'))
            .map(|(_, rest)| rest.split("</a>").next().unwrap_or_default())
            .map(|raw| collapse(&decode_entities(&strip_tags(raw))))
            .unwrap_or_default();
        // Adverts point back at the search engine.
        if link.starts_with("http") && !link.contains("duckduckgo.com/y.js") && !title.is_empty() {
            results.push((title, link, snippet));
        }
        if results.len() == SEARCH_RESULTS {
            break;
        }
    }
    results
}

fn open_page(url: &str) -> Outcome {
    let url = url.trim();
    let (page, effective) = match fetch(url) {
        Ok(found) => found,
        Err(reason) => return failure(reason),
    };
    let (title, text) = page_text(&page);
    if text.is_empty() {
        return failure(
            "The page has no readable text (it may need JavaScript, or not be a web page).",
        );
    }
    let mut cut: String = text.chars().take(PAGE_TEXT).collect();
    if text.chars().count() > PAGE_TEXT {
        cut.push_str(" […]");
    }
    let excerpt: String = text.chars().take(600).collect();
    outcome(
        format!("Title: {title}\nURL: {effective}\n\n{cut}"),
        serde_json::json!({
            "kind": "page",
            "url": effective,
            "title": title,
            "excerpt": excerpt,
            "chars": text.chars().count(),
        }),
    )
}

/// A page's title and readable text: scripts, styles and navigation left
/// out, blocks on their own lines.
fn page_text(html: &str) -> (String, String) {
    let (title, text) = page_text_skipping(html, true);
    if text.chars().count() < 60 {
        // Some pages keep everything in a header or navigation block.
        let (_, whole) = page_text_skipping(html, false);
        if whole.chars().count() > text.chars().count() {
            return (title, whole);
        }
    }
    (title, text)
}

fn page_text_skipping(html: &str, skip_chrome: bool) -> (String, String) {
    let lower = html.to_ascii_lowercase();
    let title = lower
        .find("<title")
        .and_then(|start| {
            let open = html[start..].find('>')? + start + 1;
            let close = lower[open..].find("</title>")? + open;
            Some(collapse(&decode_entities(&html[open..close])))
        })
        .unwrap_or_default();
    // Prefer the main content when the page marks it.
    let body_start = ["<main", "<article", "<body"]
        .iter()
        .find_map(|tag| lower.find(tag))
        .unwrap_or(0);
    let mut out = String::new();
    let mut i = body_start;
    let bytes = html.as_bytes();
    while i < html.len() {
        if bytes[i] == b'<' {
            let rest = &lower[i..];
            let chrome: &[&str] = if skip_chrome {
                &[
                    "<script",
                    "<style",
                    "<noscript",
                    "<svg",
                    "<nav",
                    "<footer",
                    "<header",
                    "<form",
                    "<template",
                ]
            } else {
                &["<script", "<style", "<noscript", "<svg", "<template"]
            };
            let skip = chrome.iter().find(|tag| {
                rest.starts_with(**tag)
                    && rest[tag.len()..].starts_with(|c: char| c == '>' || c.is_whitespace())
            });
            if let Some(tag) = skip {
                let name = &tag[1..];
                let close = format!("</{name}");
                match rest.find(&close) {
                    Some(end) => {
                        i += end;
                        i += lower[i..].find('>').map_or(lower.len() - i, |e| e + 1);
                    }
                    None => i = html.len(),
                }
                continue;
            }
            let end = lower[i..].find('>').map_or(html.len(), |e| i + e + 1);
            let tag = &lower[i..end];
            if [
                "<p",
                "<br",
                "<div",
                "<li",
                "<h1",
                "<h2",
                "<h3",
                "<h4",
                "<tr",
                "</p",
                "</div",
                "</li",
                "</h",
                "</tr",
                "<section",
                "</section",
            ]
            .iter()
            .any(|t| tag.starts_with(t))
            {
                out.push('\n');
            } else if tag.starts_with("<td") || tag.starts_with("<th") {
                out.push_str(" | ");
            }
            i = end;
            continue;
        }
        let next = html[i..].find('<').map_or(html.len(), |n| i + n);
        out.push_str(&html[i..next]);
        i = next;
    }
    let text = decode_entities(&out);
    let lines: Vec<String> = text
        .lines()
        .map(collapse)
        .filter(|line| line.chars().count() > 1)
        .collect();
    (title, lines.join("\n"))
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in html.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "rsquo" => Some('’'),
            "lsquo" => Some('‘'),
            "rdquo" => Some('”'),
            "ldquo" => Some('“'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ------------------------------------------------------------------ computer

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn expand(path: &str, base: &Path) -> PathBuf {
    let path = path.trim();
    if path.is_empty() || path == "." {
        return base.to_path_buf();
    }
    if path == "~" {
        return home();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home().join(rest);
    }
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn list_folder(path: &str, base: &Path) -> Outcome {
    let folder = expand(path, base);
    let Ok(read) = std::fs::read_dir(&folder) else {
        return failure("The folder could not be read.");
    };
    let mut entries: Vec<(String, bool, u64)> = read
        .filter_map(Result::ok)
        .map(|entry| {
            let meta = entry.metadata().ok();
            (
                entry.file_name().to_string_lossy().into_owned(),
                meta.as_ref().is_some_and(std::fs::Metadata::is_dir),
                meta.map_or(0, |m| m.len()),
            )
        })
        .collect();
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    let total = entries.len();
    let folders = entries.iter().filter(|e| e.1).count();
    entries.truncate(LIST_ENTRIES);
    let text = entries
        .iter()
        .map(|(name, dir, size)| {
            if *dir {
                format!("{name}/")
            } else {
                format!("{name} ({size} bytes)")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let names: Vec<String> = entries
        .iter()
        .map(|(name, dir, _)| {
            if *dir {
                format!("{name}/")
            } else {
                name.clone()
            }
        })
        .collect();
    outcome(
        format!(
            "{}: {} files and {folders} folders (not counting what is inside the folders)\n{text}{}",
            folder.display(),
            total - folders,
            if total > LIST_ENTRIES { "\n[…]" } else { "" }
        ),
        serde_json::json!({"kind": "files", "path": folder.display().to_string(), "entries": names, "total": total}),
    )
}

fn find_files(folder: &str, name: &str, base: &Path) -> Outcome {
    let root = expand(folder, base);
    let needle = name.trim().to_lowercase();
    if needle.is_empty() {
        return failure("Give part of the file name.");
    }
    let started = Instant::now();
    let mut found = Vec::new();
    let mut stack = vec![(root.clone(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if found.len() >= LIST_ENTRIES || started.elapsed() > Duration::from_secs(10) {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.filter_map(Result::ok) {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if file_name.to_lowercase().contains(&needle) {
                found.push(path.display().to_string());
            }
            if is_dir
                && depth < 8
                && !file_name.starts_with('.')
                && file_name != "node_modules"
                && file_name != "target"
            {
                stack.push((path, depth + 1));
            }
        }
    }
    outcome(
        if found.is_empty() {
            "Nothing found.".to_owned()
        } else {
            found.join("\n")
        },
        serde_json::json!({"kind": "files", "path": root.display().to_string(), "entries": found, "total": found.len()}),
    )
}

fn read_file(path: &str, base: &Path) -> Outcome {
    let path = expand(path, base);
    let Ok(file) = std::fs::File::open(&path) else {
        return failure("The file could not be opened.");
    };
    let mut bytes = Vec::new();
    let _ = file.take(512 * 1024).read_to_end(&mut bytes);
    if bytes.iter().take(4096).any(|b| *b == 0) {
        return failure("It is not a text file.");
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut cut: String = text.chars().take(FILE_TEXT).collect();
    if text.chars().count() > FILE_TEXT {
        cut.push_str("\n[…]");
    }
    let excerpt: String = text.chars().take(800).collect();
    outcome(
        cut,
        serde_json::json!({"kind": "file", "path": path.display().to_string(), "excerpt": excerpt, "chars": text.chars().count()}),
    )
}

fn write_file(path: &str, content: &str, base: &Path) -> Outcome {
    let path = expand(path, base);
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return failure("The folder could not be made.");
    }
    match std::fs::write(&path, content) {
        Ok(()) => outcome(
            format!("Wrote {} ({} bytes).", path.display(), content.len()),
            serde_json::json!({"kind": "write", "path": path.display().to_string(), "bytes": content.len()}),
        ),
        Err(_) => failure("The file could not be written."),
    }
}

fn append_file(path: &str, content: &str, base: &Path) -> Outcome {
    use std::io::Write;
    let path = expand(path, base);
    let written = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .and_then(|mut file| {
            file.write_all(content.as_bytes())
                .and_then(|()| file.metadata())
        });
    match written {
        Ok(meta) => outcome(
            format!(
                "Added {} bytes to {} (now {} bytes).",
                content.len(),
                path.display(),
                meta.len()
            ),
            serde_json::json!({"kind": "write", "path": path.display().to_string(), "bytes": content.len()}),
        ),
        Err(_) => failure("The file could not be added to. Write its first part with write_file."),
    }
}

fn run_command(command: &str, base: &Path) -> Outcome {
    if command.trim().is_empty() {
        return failure("No command.");
    }
    let child = std::process::Command::new("/bin/bash")
        .args(["-lc", command])
        .current_dir(base)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn();
    let Ok(mut child) = child else {
        return failure("The command could not start.");
    };
    // Each pipe is read on its own thread and handed back through a channel:
    // a background job the command left running keeps the pipe open, and
    // the reply must not wait for it.
    let read = |pipe: Option<Box<dyn Read + Send>>, cap: u64| {
        let (sender, receiver) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("narrivy-command-pipe".into())
            .spawn(move || {
                let mut out = Vec::new();
                if let Some(pipe) = pipe {
                    let _ = pipe.take(cap).read_to_end(&mut out);
                }
                let _ = sender.send(out);
            });
        receiver
    };
    let out_reader = read(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        1_000_000,
    );
    let err_reader = read(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        200_000,
    );
    let started = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() > COMMAND_BOUND => {
                // The command and everything it started in its group.
                let _ = std::process::Command::new("kill")
                    .args(kill_group_args(child.id()))
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break None,
        }
    };
    let drain = Duration::from_secs(2);
    let out = out_reader.recv_timeout(drain).unwrap_or_default();
    let err = err_reader.recv_timeout(drain).unwrap_or_default();
    let mut text = String::from_utf8_lossy(&out).into_owned();
    let err = String::from_utf8_lossy(&err);
    if !err.trim().is_empty() {
        text.push_str("\n[stderr]\n");
        text.push_str(&err);
    }
    let mut cut: String = text.chars().take(OUTPUT_TEXT).collect();
    if text.chars().count() > OUTPUT_TEXT {
        cut.push_str("\n[…]");
    }
    let exit = code.map_or_else(|| "stopped (took too long)".to_owned(), |c| c.to_string());
    Outcome {
        text: format!("Exit: {exit}\n{cut}"),
        failed: code != Some(0),
        detail: Some(
            serde_json::json!({"kind": "command", "command": command, "exit": exit, "output": cut}),
        ),
        picture: None,
    }
}

fn open(target: &str, base: &Path) -> Outcome {
    let target = target.trim();
    if target.is_empty() {
        return failure("Nothing to open.");
    }
    let (program, args): (&str, Vec<String>) = if let Some(app) = target.strip_suffix(".desktop") {
        ("gtk-launch", vec![app.to_owned()])
    } else if target.contains("://") {
        ("xdg-open", vec![target.to_owned()])
    } else {
        ("xdg-open", vec![expand(target, base).display().to_string()])
    };
    let started = std::process::Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match started {
        Ok(_) => outcome(
            format!("Opened {target}."),
            serde_json::json!({"kind": "open", "target": target}),
        ),
        Err(_) => failure("It could not be opened."),
    }
}

fn look_at_screen(scratch: &Path) -> Outcome {
    let (Some(script), python) = (
        std::env::var_os(super::meetings::SCREEN_SCRIPT_ENV),
        std::env::var_os(super::meetings::SCREEN_PYTHON_ENV)
            .unwrap_or_else(|| "/usr/bin/python3".into()),
    ) else {
        return failure("Looking at the screen is not set up.");
    };
    let folder = scratch.join("screen");
    let _ = std::fs::remove_dir_all(&folder);
    // The helper waits for the user to pick a screen in the portal's dialog;
    // one left unanswered must not hold the assistant's turn forever.
    let status = narrivy_platform_linux::bounded_command::status_within(
        std::process::Command::new(python)
            .arg(script)
            .arg(&folder)
            .arg("--once")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        SCREEN_BOUND,
    );
    let picture = folder.join("frame-0001.png");
    if !status.is_ok_and(|s| s.success()) || !picture.is_file() {
        return failure("The screen was not shared.");
    }
    Outcome {
        text: "Here is the screen the user shared (the picture comes with this message)."
            .to_owned(),
        failed: false,
        detail: Some(serde_json::json!({"kind": "screen"})),
        picture: Some(picture),
    }
}

/// How tall Chromium's window is: its picture is the top of the page.
const CHROMIUM_HEIGHT: u32 = 1600;

/// How long a page may take to open and be pictured.
const PAGE_BOUND: Duration = Duration::from_secs(40);

/// Firefox settings for the page viewer's own profile: the page's console
/// on standard output, and none of Firefox's own calls home.
const VIEWER_PREFS: &str = r#"user_pref("devtools.console.stdout.content", true);
user_pref("devtools.console.stdout.chrome", true);
user_pref("browser.dom.window.dump.enabled", true);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.region.network.url", "");
user_pref("browser.region.update.enabled", false);
user_pref("services.settings.server", "data:,");
user_pref("app.normandy.enabled", false);
user_pref("app.update.enabled", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("datareporting.healthreport.uploadEnabled", false);
user_pref("toolkit.telemetry.enabled", false);
user_pref("extensions.update.enabled", false);
user_pref("browser.safebrowsing.update.enabled", false);
"#;

/// Where a page is: an http(s) address as it is, a file as a file:// URL.
fn page_url(target: &str, base: &Path) -> Result<String, &'static str> {
    let target = target.trim();
    if target.starts_with("http://") || target.starts_with("https://") {
        return Ok(target.to_owned());
    }
    let path = expand(target.strip_prefix("file://").unwrap_or(target), base);
    if !path.is_file() {
        return Err("There is no such file.");
    }
    Ok(format!("file://{}", path.display()))
}

/// What the page wrote in its console: its errors and messages, not
/// Firefox's own.
fn page_console(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            // Chromium: `[…:INFO:CONSOLE:1] "text", source: url (line)`.
            if line.contains(":CONSOLE") {
                return line.split_once("] ").map(|(_, text)| text);
            }
            Some(line)
        })
        .filter(|line| {
            line.starts_with('"')
                || (line.starts_with("JavaScript error: ")
                    && !line.starts_with("JavaScript error: resource://")
                    && !line.starts_with("JavaScript error: chrome://"))
                || ["console.error: \"", "console.warn: \"", "console.log: \""]
                    .iter()
                    .any(|start| line.starts_with(start))
        })
        .take(30)
        .map(|line| line.chars().take(400).collect())
        .collect()
}

fn view_page(target: &str, width: Option<u64>, scratch: &Path, base: &Path) -> Outcome {
    // The Store's Chromium when installed, else an installed Firefox.
    let chromium = super::browser::executable();
    let firefox = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("firefox"))
            .find(|candidate| candidate.is_file())
    });
    view_page_in(chromium, firefox, target, width, scratch, base)
}

fn view_page_in(
    chromium: Option<PathBuf>,
    firefox: Option<PathBuf>,
    target: &str,
    width: Option<u64>,
    scratch: &Path,
    base: &Path,
) -> Outcome {
    let url = match page_url(target, base) {
        Ok(url) => url,
        Err(why) => return failure(why),
    };
    if chromium.is_none() && firefox.is_none() {
        return failure(
            "No browser to look with: add \"Chromium for the assistant\" in the Store, or install Firefox.",
        );
    }
    let width = width.unwrap_or(1280).clamp(320, 2560);
    // Each picture is kept for the rest of the turn (earlier messages
    // carry them); the profile is made fresh each time.
    let folder = scratch.join("page");
    let profile = folder.join("profile");
    let _ = std::fs::remove_dir_all(&profile);
    if std::fs::create_dir_all(&profile).is_err()
        || (chromium.is_none() && std::fs::write(profile.join("user.js"), VIEWER_PREFS).is_err())
    {
        return failure("The page could not be opened.");
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let picture = folder.join(format!("page-{stamp}.png"));
    let (mut command, seen) = match (&chromium, &firefox) {
        (Some(chromium), _) => {
            // Chromium's own sandbox stays on. Its time runs ahead so timers
            // and loading finish before the picture; the picture is the top
            // of the page, this tall.
            let mut command = std::process::Command::new(chromium);
            command
                .arg(format!("--user-data-dir={}", profile.display()))
                .args([
                    "--no-first-run",
                    "--no-default-browser-check",
                    "--disable-background-networking",
                    "--disable-component-update",
                    "--disable-sync",
                    "--hide-scrollbars",
                    "--virtual-time-budget=3000",
                ])
                .arg(format!("--window-size={width},{CHROMIUM_HEIGHT}"))
                .arg(format!("--screenshot={}", picture.display()))
                .arg(&url);
            (
                command,
                format!("the top {CHROMIUM_HEIGHT} pixels of the page"),
            )
        }
        (None, Some(firefox)) => {
            let mut command = std::process::Command::new(firefox);
            // A crashed Firefox otherwise starts its crash reporter, which
            // outlives it holding the console pipes open.
            command
                .env("MOZ_CRASHREPORTER_DISABLE", "1")
                .args(["--headless", "--no-remote", "-profile"])
                .arg(&profile)
                .arg(format!("--window-size={width},900"))
                .arg("--screenshot")
                .arg(&picture)
                .arg(&url);
            (command, "the whole page".to_owned())
        }
        (None, None) => return failure("No browser to look with."),
    };
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn();
    let Ok(mut child) = child else {
        return failure("The browser could not start.");
    };
    // The console comes on both. Each pipe is read on its own thread and
    // handed back through a channel, so a helper that escaped the browser's
    // process group and still holds a pipe cannot block this call (seen:
    // Firefox's crash reporter kept a test waiting forever).
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(pipe) = pipe {
                let _ = pipe.take(500_000).read_to_end(&mut out);
            }
            let _ = sender.send(out);
        });
        receiver
    };
    let out_reader = read(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err_reader = read(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) if started.elapsed() > PAGE_BOUND => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    // The browser and every page process it left in its group, whether it
    // finished or ran out of time.
    let _ = std::process::Command::new("kill")
        .args(kill_group_args(child.id()))
        .stderr(Stdio::null())
        .status();
    let _ = child.wait();
    let drain = Duration::from_secs(2);
    let mut bytes = out_reader.recv_timeout(drain).unwrap_or_default();
    bytes.push(b'\n');
    bytes.extend(err_reader.recv_timeout(drain).unwrap_or_default());
    let output = String::from_utf8_lossy(&bytes).into_owned();
    let _ = std::fs::remove_dir_all(&profile);
    if !picture.is_file() {
        return failure("The page did not open in time.");
    }
    let console = page_console(&output);
    let mut text = format!(
        "Here is {url} at {width} pixels wide (a picture of {seen} comes with this message)."
    );
    if console.is_empty() {
        text.push_str(" Its console has no errors.");
    } else {
        text.push_str("\nIts console:\n");
        text.push_str(&console.join("\n"));
    }
    Outcome {
        text,
        failed: false,
        detail: Some(serde_json::json!({"kind": "view", "url": url, "output": console.join("\n")})),
        picture: Some(picture),
    }
}

/// `kill` arguments that signal the process group `group` and nothing else.
/// The `--` matters: without it procps `kill` reads a group such as `-14`
/// or `-1028059` as the option `-1` and signals every process of the user,
/// which ends the whole desktop session.
fn kill_group_args(group: u32) -> [String; 3] {
    ["-KILL".to_owned(), "--".to_owned(), format!("-{group}")]
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_group_kill_can_never_be_read_as_every_process() {
        for group in [1_u32, 14, 7, 1_028_059] {
            let args = kill_group_args(group);
            assert_eq!(args[1], "--");
            assert_eq!(args[2], format!("-{group}"));
        }
    }

    use super::*;

    /// A helper that leaves the browser's process group still holding the
    /// console pipe (Firefox's crash reporter did) cannot hang the call.
    #[test]
    fn a_helper_holding_the_console_open_does_not_hang_the_picture() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir()?;
        std::fs::write(folder.path().join("index.html"), "<h1>Hi</h1>")?;
        let browser = folder.path().join("fake-firefox");
        std::fs::write(
            &browser,
            "#!/bin/sh\nwhile [ \"$1\" != --screenshot ]; do shift; done\n\
             printf png > \"$2\"\nsetsid sleep 30 &\nexit 0\n",
        )?;
        std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755))?;
        let started = Instant::now();
        let seen = view_page_in(
            None,
            Some(browser),
            "index.html",
            Some(390),
            folder.path(),
            folder.path(),
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "took {:?}",
            started.elapsed()
        );
        assert!(!seen.failed, "{}", seen.text);
        Ok(())
    }

    /// Where Firefox is there, a page is pictured with its console errors.
    #[test]
    fn a_page_is_pictured_with_its_errors() -> std::io::Result<()> {
        if !Path::new("/usr/bin/firefox").is_file() {
            return Ok(());
        }
        let folder = tempfile::tempdir()?;
        let page = folder.path().join("index.html");
        std::fs::write(&page, "<h1>Hi</h1><script>missing();</script>")?;
        let firefox = Some(PathBuf::from("/usr/bin/firefox"));
        let seen = view_page_in(
            None,
            firefox,
            "index.html",
            Some(390),
            folder.path(),
            folder.path(),
        );
        assert!(!seen.failed, "{}", seen.text);
        assert!(seen.picture.is_some_and(|p| p.is_file()));
        assert!(
            seen.text.contains("missing is not defined"),
            "{}",
            seen.text
        );
        Ok(())
    }

    /// Where a Chrome for Testing headless shell is at hand (a developer's
    /// Playwright cache), the Chromium path is pictured with its errors too.
    #[test]
    fn a_page_is_pictured_in_chromium_with_its_errors() -> std::io::Result<()> {
        let Some(shell) = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".cache/ms-playwright"))
            .and_then(|cache| std::fs::read_dir(cache).ok())
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| {
                entry
                    .path()
                    .join("chrome-headless-shell-linux64/chrome-headless-shell")
            })
            .find(|path| path.is_file())
        else {
            return Ok(());
        };
        let folder = tempfile::tempdir()?;
        std::fs::write(
            folder.path().join("index.html"),
            "<h1>Hi</h1><script>console.error('boom'); setTimeout(() => missing(), 500);</script>",
        )?;
        let seen = view_page_in(
            Some(shell),
            None,
            "index.html",
            None,
            folder.path(),
            folder.path(),
        );
        assert!(!seen.failed, "{}", seen.text);
        assert!(seen.picture.is_some_and(|p| p.is_file()));
        assert!(seen.text.contains("\"boom\""), "{}", seen.text);
        assert!(
            seen.text.contains("missing is not defined"),
            "{}",
            seen.text
        );
        Ok(())
    }

    #[test]
    fn a_page_is_found_and_only_its_own_console_is_kept() -> std::io::Result<()> {
        let folder = tempfile::tempdir()?;
        std::fs::write(folder.path().join("index.html"), "<h1>Hi</h1>")?;
        assert_eq!(
            page_url("index.html", folder.path()),
            Ok(format!(
                "file://{}",
                folder.path().join("index.html").display()
            ))
        );
        assert_eq!(
            page_url("https://example.org/", folder.path()),
            Ok("https://example.org/".to_owned())
        );
        assert!(page_url("missing.html", folder.path()).is_err());
        let output = "*** You are running in headless mode.\n\
console.error: Region.sys.mjs: \"Error fetching region\"\n\
console.error: \"boom\"\n\
JavaScript error: file:///a/index.html, line 1: ReferenceError: f is not defined\n\
JavaScript error: resource://gre/modules/X.sys.mjs, line 2: TypeError\n\
[1014/071337.016757:INFO:CONSOLE:1] \"Uncaught ReferenceError: g is not defined\", source: file:///a/index.html (1)\n\
[1014/071336.980557:WARNING:sandbox/policy/linux/sandbox_linux.cc:405] InitializeSandbox()\n";
        assert_eq!(
            page_console(output),
            vec![
                "console.error: \"boom\"".to_owned(),
                "JavaScript error: file:///a/index.html, line 1: ReferenceError: f is not defined"
                    .to_owned(),
                "\"Uncaught ReferenceError: g is not defined\", source: file:///a/index.html (1)"
                    .to_owned(),
            ]
        );
        Ok(())
    }

    #[test]
    fn paths_and_commands_are_in_the_working_folder() -> std::io::Result<()> {
        let folder = tempfile::tempdir()?;
        std::fs::write(folder.path().join("notes.txt"), "hello")?;
        assert_eq!(
            expand("notes.txt", folder.path()),
            folder.path().join("notes.txt")
        );
        assert_eq!(
            expand("/etc/hosts", folder.path()),
            PathBuf::from("/etc/hosts")
        );
        assert_eq!(expand("", folder.path()), folder.path());
        let args = serde_json::json!({"command": "pwd"});
        let out = call(
            COMPUTER,
            "run_command",
            &args,
            folder.path(),
            Some(folder.path()),
        );
        assert!(
            out.text.contains(&folder.path().display().to_string()),
            "{}",
            out.text
        );
        let read = call(
            COMPUTER,
            "read_file",
            &serde_json::json!({"path": "notes.txt"}),
            folder.path(),
            Some(folder.path()),
        );
        assert!(read.text.contains("hello"), "{}", read.text);
        Ok(())
    }

    #[test]
    fn remembered_versions_leave_the_query() {
        let question = "What is the latest stable Rust, and when was it released?";
        assert_eq!(
            unguessed("Rust 1.85 release date stable 2025", question),
            "Rust release date stable"
        );
        assert_eq!(
            unguessed("Python 3.12 changes", "What changed in Python 3.12?"),
            "Python 3.12 changes"
        );
        assert_eq!(unguessed("1.85", question), "1.85");
    }

    #[test]
    fn only_public_web_addresses_are_fetched() {
        assert!(public_url("https://en.wikipedia.org/wiki/Lahore"));
        assert!(public_url("http://example.com:8080/a?b"));
        assert!(!public_url("file:///etc/passwd"));
        assert!(!public_url("http://localhost:8080/"));
        assert!(!public_url("http://127.0.0.1/"));
        assert!(!public_url("http://192.168.1.4/router"));
        assert!(!public_url("http://[::1]/"));
        assert!(!public_url("http://printer/"));
        assert!(!public_url("https://user@10.0.0.2/"));
    }

    #[test]
    fn duckduckgo_results_are_read() {
        let page = r#"<a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fen.wikipedia.org%2Fwiki%2FLahore&amp;rut=x">Lahore - <b>Wikipedia</b></a>
<a class="result__snippet" href="x">Lahore is the capital of &amp; Punjab.</a>"#;
        let results = search_results(page);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "Lahore - Wikipedia");
        assert_eq!(results[0].1, "https://en.wikipedia.org/wiki/Lahore");
        assert_eq!(results[0].2, "Lahore is the capital of & Punjab.");
    }

    #[test]
    fn page_text_keeps_the_words_and_drops_scripts() {
        let html = "<html><head><title>A &amp; B</title><script>var x=1;</script></head><body><nav>Menu</nav><p>Hello <b>world</b>, this is the page's main text.</p><p>Second&nbsp;line of it, long enough to keep.</p></body></html>";
        let (title, text) = page_text(html);
        assert_eq!(title, "A & B");
        assert_eq!(
            text,
            "Hello world, this is the page's main text.\nSecond line of it, long enough to keep."
        );
        // A page whose only words are in its header still reads.
        let (_, text) =
            page_text("<body><header><h1>404</h1><p>This page does not exist.</p></header></body>");
        assert_eq!(text, "404\nThis page does not exist.");
    }
}
