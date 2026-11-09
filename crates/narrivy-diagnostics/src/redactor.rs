//! Content-safe redaction primitives.
//!
//! The [`Redactor`] enforces a deny-list of field names and scrubs string
//! values. It is fail-safe by design: anything that looks like transcript,
//! audio, credential, or clipboard content is removed, never logged.

use std::collections::BTreeSet;

/// Default field names that must never appear in diagnostic output.
///
/// The list must stay at least as strict as the identifier vocabulary the
/// log-redaction gate treats as content- or credential-bearing, otherwise a
/// diagnostic that routes a value through this facility is exempt from the
/// gate while still leaking it. Compound names are covered by
/// [`Redactor::key_is_denied`] subtoken matching, so `last_delivered` also
/// denies `last-delivered` and `lastDelivered`.
pub const DEFAULT_DENIED_KEYS: [&str; 22] = [
    "transcript",
    "text",
    "audio",
    "samples",
    "clipboard",
    "token",
    "secret",
    "password",
    "authorization",
    "cookie",
    "last_delivered",
    "delivered",
    "words",
    "spoken",
    "utterance",
    "tokens",
    "context",
    "passphrase",
    "api_key",
    "apikey",
    "access_key",
    "private_key",
];

/// Default maximum retained length for a single string value.
pub const DEFAULT_MAX_STRING_LEN: usize = 256;

const TRUNCATION_MARKER_PREFIX: &str = "[TRUNCATED:";
const REDACTED_PREFIX: &str = "[REDACTED:";

/// Content-safety gate for diagnostic payloads.
///
/// Two independent layers:
///
/// 1. **Field-name deny-list**: JSON object keys are matched case-insensitively.
///    A key matches when its lowercased form equals a denied entry, or when any
///    alphanumeric subtoken of the key (split on non-alphanumeric separators
///    such as `_`, `-`, `.`, `/`, and on lower-to-upper case transitions) equals
///    a denied entry. Matched values are replaced with `[REDACTED:<len>]` where
///    `<len>` is the original serialized length.
/// 2. **Value scrubbing**: every surviving string has embedded `key=value` /
///    `key: value` pairs whose key matches the deny-list replaced with
///    `[REDACTED:<len>]`, and strings longer than [`Redactor::max_string_len`]
///    are truncated with an `[TRUNCATED:orig_len=N]` marker at a char boundary.
#[derive(Debug, Clone)]
pub struct Redactor {
    denied_keys: BTreeSet<String>,
    max_string_len: usize,
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

impl Redactor {
    /// Redactor with the default deny-list and default max string length.
    pub fn new() -> Self {
        Self {
            denied_keys: DEFAULT_DENIED_KEYS
                .iter()
                .map(|k| k.to_ascii_lowercase())
                .collect(),
            max_string_len: DEFAULT_MAX_STRING_LEN,
        }
    }

    /// Replace the deny-list entirely with the given entries (lowercased).
    #[must_use]
    pub fn with_denied_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.denied_keys = keys
            .into_iter()
            .map(|k| k.as_ref().to_ascii_lowercase())
            .collect();
        self
    }

    /// Add one entry to the deny-list.
    #[must_use]
    pub fn deny_key<S: AsRef<str>>(mut self, key: S) -> Self {
        self.denied_keys.insert(key.as_ref().to_ascii_lowercase());
        self
    }

    /// Set the maximum retained length for string values.
    ///
    /// # Errors
    /// Returns [`DiagnosticsError::InvalidConfig`] when `max` is 0.
    pub fn with_max_string_len(mut self, max: usize) -> Result<Self, crate::DiagnosticsError> {
        if max == 0 {
            return Err(crate::DiagnosticsError::InvalidConfig(
                "max_string_len must be > 0".to_owned(),
            ));
        }
        self.max_string_len = max;
        Ok(self)
    }

    /// Maximum retained length for string values.
    #[must_use]
    pub fn max_string_len(&self) -> usize {
        self.max_string_len
    }

    /// Case-insensitive deny-list match on a field name.
    ///
    /// Matches on whole lowercased key or on alphanumeric subtokens of the key,
    /// so `audio.samples` and `my_transcript_blob` match, and case conventions
    /// of the same name (`last_delivered`, `last-delivered`, `LastDelivered`)
    /// all match, while `texture` does not falsely match `text`.
    #[must_use]
    pub fn key_is_denied(&self, key: &str) -> bool {
        let lowered = key.to_ascii_lowercase();
        if self.denied_keys.contains(&lowered) {
            return true;
        }
        key_tokens(key).into_iter().any(|token| {
            self.denied_keys
                .contains(token.to_ascii_lowercase().as_str())
        })
    }

    /// Recursively scrub a JSON value.
    #[must_use]
    pub fn redact_json(&self, value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (key, val) in map {
                    if self.key_is_denied(key) {
                        out.insert(key.clone(), redacted_marker(serialized_len(val)));
                    } else {
                        out.insert(key.clone(), self.redact_json(val));
                    }
                }
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(|v| self.redact_json(v)).collect())
            }
            serde_json::Value::String(s) => serde_json::Value::String(self.scrub_string(s)),
            other => other.clone(),
        }
    }

    /// Scrub one string: first remove matching `key=value` pairs, then
    /// truncate to [`Self::max_string_len`] with a marker.
    #[must_use]
    pub fn scrub_string(&self, input: &str) -> String {
        let scrubbed = self.scrub_kv(input);
        truncate_with_marker(&scrubbed, self.max_string_len)
    }

    /// Remove only the embedded `key=value` / `key: value` pairs whose key
    /// matches the deny-list, without truncating.
    ///
    /// This is the content-safety half of [`Self::scrub_string`]: a caller that
    /// must distinguish "carries denied content" from "longer than the bound"
    /// compares this value, because [`Self::scrub_string`] also truncates.
    #[must_use]
    pub fn scrub_kv(&self, input: &str) -> String {
        self.scrub_kv_pairs(input)
    }

    fn scrub_kv_pairs(&self, input: &str) -> String {
        let bytes = input.as_bytes();
        let mut cuts: Vec<(usize, usize)> = Vec::new();
        for key in &self.denied_keys {
            let mut from = 0usize;
            while let Some(rel) = ci_find(bytes, from, key) {
                let after_key = rel + key.len();
                let mut i = skip_spaces(bytes, after_key);
                if i < bytes.len() && (bytes[i] == b'=' || bytes[i] == b':') {
                    i = skip_spaces(bytes, i + 1);
                    let val_start = i;
                    let end = scan_value_end(bytes, i);
                    if end > val_start {
                        cuts.push((val_start, end));
                    }
                    from = end.max(after_key);
                } else {
                    from = after_key;
                }
            }
        }
        if cuts.is_empty() {
            return input.to_owned();
        }
        cuts.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(cuts.len());
        for (start, end) in cuts {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        let mut out = String::with_capacity(input.len());
        let mut cursor = 0usize;
        for (start, end) in merged {
            out.push_str(&input[cursor..start]);
            out.push_str(REDACTED_PREFIX);
            // Length of the removed content, in UTF-8 code points.
            let len = input[start..end].chars().count();
            out.push_str(itoa(len).as_str());
            out.push(']');
            cursor = end;
        }
        out.push_str(&input[cursor..]);
        out
    }
}

fn redacted_marker(len: usize) -> serde_json::Value {
    serde_json::Value::String(format!("{REDACTED_PREFIX}{}]", itoa(len)))
}

/// Alphanumeric subtokens of a field name, split on non-alphanumeric
/// characters and on lower-to-upper case transitions so that every casing of
/// one name (`last_delivered`, `last-delivered`, `LastDelivered`) yields the
/// same subtokens. All subtokens are ASCII, so every slice is a char boundary.
fn key_tokens(key: &str) -> Vec<&str> {
    let mut tokens: Vec<&str> = Vec::new();
    let mut start: Option<usize> = None;
    let mut previous = '\0';
    for (index, ch) in key.char_indices() {
        if !ch.is_ascii_alphanumeric() {
            if let Some(begin) = start.take() {
                tokens.push(&key[begin..index]);
            }
        } else if let Some(begin) = start {
            if previous.is_ascii_lowercase() && ch.is_ascii_uppercase() {
                tokens.push(&key[begin..index]);
                start = Some(index);
            }
        } else {
            start = Some(index);
        }
        previous = ch;
    }
    if let Some(begin) = start.take() {
        tokens.push(&key[begin..]);
    }
    tokens
}

/// Serialized byte length used for the `[REDACTED:<len>]` marker.
fn serialized_len(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(s) => s.chars().count(),
        other => serde_json::to_string(other).map(|s| s.len()).unwrap_or(0),
    }
}

fn itoa(mut n: usize) -> String {
    if n == 0 {
        return "0".to_owned();
    }
    let mut buf = Vec::with_capacity(20);
    while n > 0 {
        buf.push(b'0' + (n % 10) as u8);
        n /= 10;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap_or_default()
}

/// Truncate to at most `max` UTF-8 bytes at a char boundary, appending a
/// marker that records the original length.
fn truncate_with_marker(input: &str, max: usize) -> String {
    if input.len() <= max {
        return input.to_owned();
    }
    let mut end = max;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 32);
    out.push_str(&input[..end]);
    out.push_str(TRUNCATION_MARKER_PREFIX);
    out.push_str("orig_len=");
    out.push_str(itoa(input.chars().count()).as_str());
    out.push(']');
    out
}

fn skip_spaces(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    i
}

/// ASCII case-insensitive search for `needle` in `haystack` starting at `from`.
fn ci_find(haystack: &[u8], from: usize, needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let last_start = haystack.len() - needle.len();
    let mut start = from.min(last_start + 1);
    while start <= last_start {
        let window = &haystack[start..start + needle.len()];
        if window
            .iter()
            .zip(needle.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(start);
        }
        start += 1;
    }
    None
}

/// Find the exclusive end index of a `key=value` value starting at `i`,
/// handling quoted values (with backslash escapes) and bare tokens ended by
/// common delimiters.
fn scan_value_end(bytes: &[u8], i: usize) -> usize {
    if i >= bytes.len() {
        return i;
    }
    let quote = bytes[i];
    if quote == b'"' || quote == b'\'' {
        let mut j = i + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' if j + 1 < bytes.len() => j += 2,
                c if c == quote => return (j + 1).min(bytes.len()),
                _ => j += 1,
            }
        }
        return bytes.len();
    }
    let mut j = i;
    while j < bytes.len() && !matches!(bytes[j], b' ' | b'\t' | b',' | b';' | b'&' | b'\n' | b'\r')
    {
        j += 1;
    }
    j
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contains_ci(haystack: &str, needle: &str) -> bool {
        ci_find(haystack.as_bytes(), 0, needle).is_some()
    }

    #[test]
    fn denies_exact_and_tokenized_keys_but_not_texture() {
        let r = Redactor::new();
        assert!(r.key_is_denied("transcript"));
        assert!(r.key_is_denied("TRANSCRIPT"));
        assert!(r.key_is_denied("audio_samples"));
        assert!(r.key_is_denied("auth.token.value"));
        assert!(!r.key_is_denied("texture"));
        assert!(!r.key_is_denied("stage_latency"));
    }

    #[test]
    fn redacted_object_value_never_survives() {
        let r = Redactor::new();
        let input = json!({
            "transcript": "patient says the password is hunter2",
            "nested": { "audio": vec![1, 2, 3] },
            "ok_field": "fine"
        });
        let out = r.redact_json(&input);
        let text = serde_json::to_string(&out).unwrap_or_default();
        assert!(!contains_ci(&text, "hunter2"));
        assert!(!text.contains("patient says"));
        let transcript_len = "patient says the password is hunter2".chars().count();
        assert_eq!(
            out["transcript"],
            json!(format!("[REDACTED:{transcript_len}]"))
        );
        let audio_len = serde_json::to_string(&json!(vec![1, 2, 3]))
            .unwrap_or_default()
            .len();
        assert_eq!(
            out["nested"]["audio"],
            json!(format!("[REDACTED:{audio_len}]"))
        );
        assert_eq!(out["ok_field"], json!("fine"));
    }

    #[test]
    fn kv_pairs_inside_strings_are_scrubbed() {
        let r = Redactor::new();
        let scrubbed = r.scrub_string("session ok; token=abcdef123; latency_ms=42");
        assert!(!contains_ci(&scrubbed, "abcdef123"));
        assert!(scrubbed.contains("[REDACTED:9]"));
        assert!(scrubbed.contains("latency_ms=42"));
    }

    #[test]
    fn quoted_and_colon_kv_forms_are_scrubbed_case_insensitively() {
        let r = Redactor::new();
        let scrubbed = r.scrub_string(r#"Authorization: "Bearer abc", PASSWORD='x y'"#);
        assert!(!contains_ci(&scrubbed, "Bearer"));
        assert!(!contains_ci(&scrubbed, "x y"));
        assert!(scrubbed.contains(REDACTED_PREFIX));
    }

    #[test]
    fn long_strings_are_truncated_at_char_boundary_with_marker() {
        let r = Redactor::new()
            .with_max_string_len(16)
            .unwrap_or_else(|_| Redactor::new());
        let long = "é".repeat(40);
        let out = r.scrub_string(&long);
        assert!(out.starts_with("é".repeat(8).as_str()));
        assert!(out.contains("[TRUNCATED:orig_len=40]"));
        assert!(out.len() <= 16 + "[TRUNCATED:orig_len=40]".len());
    }

    #[test]
    fn arrays_and_deep_nesting_are_recursed() {
        let r = Redactor::new();
        let input = json!({
            "events": [
                { "text": "spoken words here" },
                { "stage": "asr", "micros": 120 }
            ]
        });
        let out = r.redact_json(&input);
        assert_eq!(out["events"][0]["text"], json!("[REDACTED:17]"));
        assert_eq!(out["events"][1]["micros"], json!(120));
    }

    #[test]
    fn custom_deny_list_replaces_defaults() {
        let r = Redactor::new().with_denied_keys(["custom"]);
        assert!(r.key_is_denied("custom"));
        assert!(!r.key_is_denied("transcript"));
        let out = r.redact_json(&json!({ "custom": "a", "transcript": "keep?" }));
        assert_eq!(out["custom"], json!("[REDACTED:1]"));
        assert_eq!(out["transcript"], json!("keep?"));
    }

    #[test]
    fn denied_keys_cover_the_gate_content_and_credential_vocabulary() {
        let r = Redactor::new();
        for key in [
            "last_delivered",
            "delivered",
            "words",
            "spoken",
            "utterance",
            "tokens",
            "context",
            "passphrase",
            "api_key",
            "apikey",
            "access_key",
            "private_key",
        ] {
            assert!(r.key_is_denied(key), "{key} must be denied");
        }
    }

    #[test]
    fn delivered_speech_and_word_content_is_redacted() {
        let r = Redactor::new();
        let input = json!({
            "last_delivered": "the invoice goes to accounting on thursday",
            "words": ["hello", "world"],
            "spoken": "please send the invoice",
            "utterance": "and one more thing",
            "state": "delivered",
            "txid": "tx-1",
            "micros": 120
        });
        let out = r.redact_json(&input);
        let text = serde_json::to_string(&out).unwrap_or_default();
        assert!(!text.contains("the invoice goes to accounting"));
        assert!(!text.contains("hello"));
        assert!(!text.contains("please send the invoice"));
        assert!(!text.contains("and one more thing"));
        assert_eq!(out["last_delivered"], json!("[REDACTED:42]"), "{text}");
        assert_eq!(out["spoken"], json!("[REDACTED:23]"), "{text}");
        let words_len = serde_json::to_string(&json!(["hello", "world"]))
            .unwrap_or_default()
            .len();
        assert_eq!(
            out["words"],
            json!(format!("[REDACTED:{words_len}]")),
            "{text}"
        );
        assert_eq!(out["utterance"], json!("[REDACTED:18]"), "{text}");
        // A state name that merely contains a denied subtoken as its value
        // stays readable, and non-sensitive fields are untouched.
        assert_eq!(out["state"], json!("delivered"));
        assert_eq!(out["txid"], json!("tx-1"));
        assert_eq!(out["micros"], json!(120));
    }

    #[test]
    fn case_and_separator_variants_of_one_name_are_denied() {
        let r = Redactor::new();
        for key in [
            "last_delivered",
            "LastDelivered",
            "last-delivered",
            "LAST_DELIVERED",
            "last.Delivered",
            "journal.last_delivered",
        ] {
            assert!(r.key_is_denied(key), "{key} must be denied");
        }
        for key in ["words", "WordTokens", "WORDS", "spoken-text", "SPOKEN"] {
            assert!(r.key_is_denied(key), "{key} must be denied");
        }
    }

    #[test]
    fn non_sensitive_names_sharing_a_prefix_are_not_denied() {
        let r = Redactor::new();
        for key in [
            "texture",
            "stage_latency",
            "keyword",
            "word_count",
            "contextual_note",
            "delivery_state",
            "pipeline_id",
            "utterances_counted",
        ] {
            assert!(!r.key_is_denied(key), "{key} must not be denied");
        }
    }

    #[test]
    fn speech_and_delivery_subtokens_redact_absent_presence_qualifiers() {
        let r = Redactor::new();
        // A presence flag and a language tag carry no speech, but subtoken
        // matching cannot tell them apart from `has_delivered_text` carrying
        // the text itself, so they are denied too. Pin the fail-safe choice.
        assert!(r.key_is_denied("has_delivered_text"));
        assert!(r.key_is_denied("spoken_language"));
        assert!(r.key_is_denied("context_hint"));
    }

    #[test]
    fn kv_pairs_for_newly_denied_keys_are_scrubbed() {
        let r = Redactor::new();
        let scrubbed = r.scrub_string(r#"stage=asr words=hello spoken=hi delivered="the invoice""#);
        assert!(!contains_ci(&scrubbed, "hello"));
        assert!(!contains_ci(&scrubbed, "invoice"));
        assert!(scrubbed.contains("words=[REDACTED:5]"), "{scrubbed}");
        assert!(scrubbed.contains("spoken=[REDACTED:2]"), "{scrubbed}");
        assert!(scrubbed.contains("delivered=[REDACTED:13]"), "{scrubbed}");
        assert!(scrubbed.contains("stage=asr"), "{scrubbed}");
    }
}
