//! Repair the tool calls a small local model gets slightly wrong, before
//! they fail: a misspelled or aliased tool name, argument names that are
//! near the real ones, arguments wrapped one level too deep or sent as a
//! JSON string, and values of the wrong type (numbers, booleans and lists
//! written as text, or text written as a number). Only unambiguous fixes
//! are made; anything else fails with a message that says what is wrong.
//! Ideas from open-source agent harnesses for local models; our own code.

#![forbid(unsafe_code)]

use serde_json::{Map, Value};

use super::assistant_tools::Tool;

/// What a name the model used means (lower case, `_` for spaces and `-`).
const TOOL_ALIASES: &[(&str, &str)] = &[
    ("search", "web_search"),
    ("websearch", "web_search"),
    ("google", "web_search"),
    ("browse", "open_page"),
    ("fetch", "open_page"),
    ("read_page", "open_page"),
    ("read", "read_file"),
    ("cat", "read_file"),
    ("open_file", "read_file"),
    ("write", "write_file"),
    ("create_file", "write_file"),
    ("save_file", "write_file"),
    ("edit", "edit_file"),
    ("replace", "edit_file"),
    ("patch", "edit_file"),
    ("str_replace", "edit_file"),
    ("ls", "list_folder"),
    ("list_dir", "list_folder"),
    ("list_directory", "list_folder"),
    ("list_files", "list_folder"),
    ("bash", "run_command"),
    ("shell", "run_command"),
    ("run", "run_command"),
    ("exec", "run_command"),
    ("execute", "run_command"),
    ("terminal", "run_command"),
    ("powershell", "run_command"),
    ("create_document", "make_document"),
    ("write_document", "make_document"),
    ("make_file", "make_document"),
    ("make_pdf", "make_document"),
    ("make_slides", "make_document"),
    ("create_presentation", "make_document"),
];

/// What an argument name the model used means.
const ARG_ALIASES: &[(&str, &str)] = &[
    ("file", "path"),
    ("filename", "path"),
    ("file_name", "path"),
    ("file_path", "path"),
    ("filepath", "path"),
    ("dir", "path"),
    ("directory", "path"),
    ("cmd", "command"),
    ("script", "command"),
    ("text", "content"),
    ("body", "content"),
    ("markdown", "content"),
    ("contents", "content"),
    ("data", "content"),
    ("q", "query"),
    ("search", "query"),
    ("search_query", "query"),
    ("link", "url"),
    ("href", "url"),
    ("address", "url"),
    ("old_text", "old"),
    ("old_string", "old"),
    ("find", "old"),
    ("search_text", "old"),
    ("original", "old"),
    ("new_text", "new"),
    ("new_string", "new"),
    ("replacement", "new"),
    ("replace_with", "new"),
    ("type", "format"),
    ("file_type", "format"),
    ("kind", "format"),
    ("name", "title"),
    ("heading", "title"),
];

/// Wrappers a model puts around the arguments.
const WRAPPERS: &[&str] = &["arguments", "parameters", "args", "input", "params"];

fn norm(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c == '-' || c == ' ' || c == '.' {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Edit distance, for names a letter or two off.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            prev = here;
        }
    }
    row[b.len()]
}

/// The one candidate within two edits, if exactly one is.
fn closest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let near: Vec<&str> = candidates
        .filter(|c| distance(name, &norm(c)) <= 2 && name.len() > 3)
        .collect();
    (near.len() == 1).then(|| near[0])
}

/// Which tool a name means: exact, then by spelling, alias or a close
/// misspelling. `None` when it is not clear.
pub fn tool_index(name: &str, tools: &[Tool]) -> Option<usize> {
    if let Some(i) = tools.iter().position(|t| t.name == name) {
        return Some(i);
    }
    // "functions.run_command": the part after the last dot.
    let wanted = norm(name.rsplit(['.', ':', '/']).next().unwrap_or(name));
    if let Some(i) = tools.iter().position(|t| norm(&t.name) == wanted) {
        return Some(i);
    }
    if let Some((_, real)) = TOOL_ALIASES.iter().find(|(alias, _)| *alias == wanted)
        && let Some(i) = tools.iter().position(|t| t.name == *real)
    {
        return Some(i);
    }
    let near = closest(&wanted, tools.iter().map(|t| t.name.as_str()))?;
    tools.iter().position(|t| t.name == near)
}

fn properties(tool: &Tool) -> Map<String, Value> {
    tool.schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn required(tool: &Tool) -> Vec<String> {
    tool.schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// An object, or a JSON string holding one.
fn as_object(value: &Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(map) => Some(map.clone()),
        Value::String(text) => serde_json::from_str::<Value>(text.trim())
            .ok()
            .and_then(|v| v.as_object().cloned()),
        _ => None,
    }
}

/// A value turned into the type the schema asks for, when it plainly is
/// that type written another way.
fn coerce(value: Value, kind: &str) -> Value {
    match (kind, value) {
        ("string", Value::Number(n)) => Value::String(n.to_string()),
        ("string", Value::Bool(b)) => Value::String(b.to_string()),
        ("string", v @ (Value::Array(_) | Value::Object(_))) => Value::String(v.to_string()),
        ("integer", Value::String(s)) => s
            .trim()
            .parse::<i64>()
            .map_or(Value::String(s), Value::from),
        ("number", Value::String(s)) => s
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::String(s), Value::Number),
        ("boolean", Value::String(s)) => match s.trim().to_lowercase().as_str() {
            "true" | "yes" | "1" => Value::Bool(true),
            "false" | "no" | "0" => Value::Bool(false),
            _ => Value::String(s),
        },
        ("array", Value::String(s)) => serde_json::from_str::<Value>(s.trim())
            .ok()
            .filter(Value::is_array)
            .unwrap_or_else(|| {
                Value::Array(
                    s.split(',')
                        .map(|p| Value::String(p.trim().to_owned()))
                        .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                        .collect(),
                )
            }),
        (_, v) => v,
    }
}

/// A call's arguments repaired for `tool`.
///
/// # Errors
/// What is missing, in words the model can act on.
pub fn arguments(tool: &Tool, given: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let props = properties(tool);
    let mut given = given.clone();
    // One level too deep: {"arguments": {...}} or {"<tool name>": {...}}.
    if given.len() == 1
        && let Some((key, value)) = given.iter().next()
        && !props.contains_key(key)
        && (WRAPPERS.contains(&norm(key).as_str()) || norm(key) == norm(&tool.name))
        && let Some(inner) = as_object(value)
    {
        given = inner;
    }
    // A tool with one argument that got one value under another name: it
    // is that argument.
    if props.len() == 1
        && given.len() == 1
        && let (Some(real), Some((key, _))) = (props.keys().next(), given.iter().next())
        && real != key
    {
        let real = real.clone();
        let value = given.values().next().cloned().unwrap_or(Value::Null);
        given = Map::from_iter([(real, value)]);
    }
    let mut out = Map::new();
    let mut unknown = Vec::new();
    for (key, value) in given {
        if props.contains_key(&key) {
            out.insert(key, value);
            continue;
        }
        let n = norm(&key);
        let by_spelling = props.keys().find(|p| norm(p) == n).cloned();
        let by_alias = ARG_ALIASES
            .iter()
            .find(|(alias, real)| *alias == n && props.contains_key(*real))
            .map(|(_, real)| (*real).to_owned());
        let free: Vec<&str> = props
            .keys()
            .filter(|p| !out.contains_key(p.as_str()))
            .map(String::as_str)
            .collect();
        let by_distance = closest(&n, free.into_iter()).map(str::to_owned);
        match by_spelling.or(by_alias).or(by_distance) {
            Some(real) if !out.contains_key(&real) => {
                out.insert(real, value);
            }
            _ => unknown.push(key),
        }
    }
    for (key, value) in &mut out {
        if let Some(kind) = props
            .get(key)
            .and_then(|p| p.get("type"))
            .and_then(Value::as_str)
        {
            *value = coerce(value.take(), kind);
        }
    }
    let missing: Vec<String> = required(tool)
        .into_iter()
        .filter(|r| {
            out.get(r)
                .is_none_or(|v| v.as_str().is_some_and(|s| s.trim().is_empty()) && r != "new")
        })
        .collect();
    if !missing.is_empty() {
        let names: Vec<&String> = props.keys().collect();
        return Err(format!(
            "The {} call is missing {}. Its arguments are: {}.",
            tool.name,
            missing.join(", "),
            names
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(out)
}

/// A call written as JSON instead of the XML-like form:
/// `{"name": ..., "arguments": {...}}` (arguments may be a JSON string).
pub fn json_call(block: &str) -> Option<(String, Map<String, Value>)> {
    let value: Value = serde_json::from_str(block.trim()).ok()?;
    let object = value.as_object()?;
    let name = object
        .get("name")
        .or_else(|| object.get("function"))
        .or_else(|| object.get("tool"))
        .and_then(Value::as_str)?
        .to_owned();
    let arguments = WRAPPERS
        .iter()
        .find_map(|w| object.get(*w).and_then(as_object))
        .unwrap_or_default();
    Some((name, arguments))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, schema: Value) -> Tool {
        Tool {
            server_id: "computer".to_owned(),
            server_name: "Computer".to_owned(),
            name: name.to_owned(),
            description: String::new(),
            schema,
        }
    }

    fn set() -> Vec<Tool> {
        vec![
            tool(
                "edit_file",
                serde_json::json!({"type": "object", "properties": {"path": {"type": "string"},
                    "old": {"type": "string"}, "new": {"type": "string"}}, "required": ["path", "old", "new"]}),
            ),
            tool(
                "view_page",
                serde_json::json!({"type": "object", "properties": {"target": {"type": "string"},
                    "width": {"type": "integer"}}, "required": ["target"]}),
            ),
            tool(
                "run_command",
                serde_json::json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}),
            ),
        ]
    }

    #[test]
    fn tool_names_are_found_through_spelling_aliases_and_typos() {
        let tools = set();
        assert_eq!(tool_index("edit_file", &tools), Some(0));
        assert_eq!(tool_index("Edit-File", &tools), Some(0));
        assert_eq!(tool_index("str_replace", &tools), Some(0));
        assert_eq!(tool_index("edit_fiel", &tools), Some(0));
        assert_eq!(tool_index("bash", &tools), Some(2));
        assert_eq!(tool_index("functions.run_command", &tools), Some(2));
        assert_eq!(tool_index("delete_everything", &tools), None);
    }

    #[test]
    fn argument_names_nesting_and_types_are_repaired() -> Result<(), String> {
        let tools = set();
        let given = serde_json::json!({"arguments": "{\"file_path\": \"a.rs\", \"old_string\": \"x\", \"new_string\": \"y\"}"});
        let fixed = arguments(&tools[0], given.as_object().ok_or("object")?)?;
        assert_eq!(fixed.get("path"), Some(&Value::from("a.rs")));
        assert_eq!(fixed.get("old"), Some(&Value::from("x")));
        assert_eq!(fixed.get("new"), Some(&Value::from("y")));
        let given = serde_json::json!({"targte": "https://a.b", "width": "390"});
        let fixed = arguments(&tools[1], given.as_object().ok_or("object")?)?;
        assert_eq!(fixed.get("target"), Some(&Value::from("https://a.b")));
        assert_eq!(fixed.get("width"), Some(&Value::from(390)));
        // A command that parsed as a number is text again.
        let given = serde_json::json!({"cmd": 42});
        let fixed = arguments(&tools[2], given.as_object().ok_or("object")?)?;
        assert_eq!(fixed.get("command"), Some(&Value::from("42")));
        Ok(())
    }

    #[test]
    fn what_cannot_be_repaired_is_said_plainly() {
        let tools = set();
        let given = serde_json::json!({"path": "a.rs"});
        let error = arguments(
            &tools[0],
            given.as_object().cloned().as_ref().unwrap_or(&Map::new()),
        );
        assert!(error.is_err_and(|e| e.contains("missing old, new")));
    }

    #[test]
    fn calls_written_as_json_are_read() {
        let (name, args) = json_call(r#"{"name": "run_command", "arguments": {"command": "ls"}}"#)
            .unwrap_or_default();
        assert_eq!(name, "run_command");
        assert_eq!(args.get("command"), Some(&Value::from("ls")));
        let (_, args) =
            json_call(r#"{"name": "x", "arguments": "{\"a\": 1}"}"#).unwrap_or_default();
        assert_eq!(args.get("a"), Some(&Value::from(1)));
        assert!(json_call("not json").is_none());
    }
}
