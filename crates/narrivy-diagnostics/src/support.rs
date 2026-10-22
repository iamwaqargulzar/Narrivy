//! Support report scaffold built only from explicitly supplied safe inputs.
//!
//! A support report is a bundle a user may attach to a bug report. It must
//! therefore contain nothing the caller did not explicitly pass in: no
//! environment variable dumps, no transcript/audio/context content, no
//! secrets. Callers choose every field; [`crate::Redactor`] can be applied to
//! the serialized report as a final content-safety gate.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::DiagnosticsError;
use crate::health::HealthFact;
use crate::latency::LatencySummary;

/// Current [`SupportReport`] schema version.
pub const SUPPORT_REPORT_SCHEMA_VERSION: u32 = 1;

/// A user-shareable diagnostics bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportReport {
    /// Schema version of this report.
    pub schema_version: u32,
    /// Generation time in Unix milliseconds (caller-supplied).
    pub generated_unix_ms: u64,
    /// Build identifier (caller-supplied, e.g. git short hash).
    pub build_id: String,
    /// Latest health facts per subsystem, sorted by subsystem.
    pub health: Vec<HealthFact>,
    /// Latency percentiles summary.
    pub latency: LatencySummary,
    /// Explicitly chosen environment entries only; never populated
    /// automatically from process environment variables.
    pub environment: BTreeMap<String, String>,
}

impl SupportReport {
    /// Start a report with `schema_version` set and empty collections.
    #[must_use]
    pub fn new(build_id: impl Into<String>, generated_unix_ms: u64) -> Self {
        Self {
            schema_version: SUPPORT_REPORT_SCHEMA_VERSION,
            generated_unix_ms,
            build_id: build_id.into(),
            health: Vec::new(),
            latency: LatencySummary::default(),
            environment: BTreeMap::new(),
        }
    }

    /// Attach latest health facts (already sorted via [`crate::HealthRegistry::snapshot`]).
    pub fn set_health(&mut self, health: Vec<HealthFact>) {
        self.health = health;
    }

    /// Attach a latency summary computed from a tracker.
    pub fn set_latency(&mut self, latency: LatencySummary) {
        self.latency = latency;
    }

    /// Insert one explicitly chosen environment entry.
    pub fn add_environment(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.environment.insert(key.into(), value.into());
    }

    /// Serialize to content-safe pretty JSON.
    ///
    /// The default output path always applies the diagnostics redactor. This
    /// keeps a caller mistake, such as placing transcript text in an
    /// environment field, from turning into a shareable plaintext report.
    ///
    /// # Errors
    /// Returns [`DiagnosticsError::Json`] on serialization failure.
    pub fn to_json_pretty(&self) -> Result<String, DiagnosticsError> {
        let value = serde_json::to_value(self)?;
        let redacted = crate::Redactor::new().redact_json(&value);
        Ok(serde_json::to_string_pretty(&redacted)?)
    }

    /// Parse a report from JSON text.
    ///
    /// # Errors
    /// Returns [`DiagnosticsError::Json`] on malformed input.
    pub fn from_json_str(text: &str) -> Result<Self, DiagnosticsError> {
        Ok(serde_json::from_str(text)?)
    }

    /// Verify that this report carries no content the redactor would have to
    /// remove, before it is handed to a user who may share it.
    ///
    /// [`Self::to_json_pretty`] redacts unconditionally, so its output is
    /// content-free by construction. This check is the stronger statement: it
    /// reports whether the *report itself* was built from content-free inputs,
    /// so a caller can fail closed instead of quietly shipping a report whose
    /// meaningful fields were all replaced by `[REDACTED:<len>]` markers.
    ///
    /// `content_free` is true only when the report is byte-identical to its own
    /// redaction: no field name is denied, and no string value embeds a denied
    /// `key=value`/`key: value` pair. Over-long values are reported separately
    /// because truncation is a fidelity loss, not a content leak.
    pub fn content_check(&self) -> ContentCheck {
        let Ok(value) = serde_json::to_value(self) else {
            // A report that cannot even be serialized is not shareable; the
            // caller must fail closed rather than treat this as clean.
            return ContentCheck {
                content_free: false,
                denied_fields: vec!["<report>:json_serialization_failed".to_owned()],
                overlong_fields: Vec::new(),
            };
        };
        let redactor = crate::Redactor::new();
        let mut denied_fields = Vec::new();
        let mut overlong_fields = Vec::new();
        collect_check_findings(
            &value,
            "",
            &redactor,
            &mut denied_fields,
            &mut overlong_fields,
        );
        ContentCheck {
            content_free: denied_fields.is_empty(),
            denied_fields,
            overlong_fields,
        }
    }
}

/// Verdict of [`SupportReport::content_check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentCheck {
    /// No denied field name and no denied `key=value` pair survived anywhere
    /// in the report. Only then may a caller treat it as shareable.
    pub content_free: bool,
    /// Dotted paths of the fields whose name the redactor denies.
    pub denied_fields: Vec<String>,
    /// Dotted paths of string values longer than the redactor's bound, which
    /// [`SupportReport::to_json_pretty`] would truncate.
    pub overlong_fields: Vec<String>,
}

impl ContentCheck {
    /// A short content-free one-line summary safe to render in a diagnostic.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "content_check={} denied_fields={} overlong_fields={}",
            if self.content_free { "clean" } else { "denied" },
            self.denied_fields.len(),
            self.overlong_fields.len(),
        )
    }
}

/// Depth-first scan recording denied field paths and over-long string values.
fn collect_check_findings(
    value: &serde_json::Value,
    path: &str,
    redactor: &crate::Redactor,
    denied_fields: &mut Vec<String>,
    overlong_fields: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if redactor.key_is_denied(key) {
                    denied_fields.push(child_path);
                    continue;
                }
                collect_check_findings(
                    child,
                    &child_path,
                    redactor,
                    denied_fields,
                    overlong_fields,
                );
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_check_findings(
                    child,
                    &format!("{path}[{index}]"),
                    redactor,
                    denied_fields,
                    overlong_fields,
                );
            }
        }
        serde_json::Value::String(text) => {
            if text.len() > redactor.max_string_len() {
                overlong_fields.push(path_of(path));
            }
            if redactor.scrub_kv(text) != *text {
                denied_fields.push(path_of(path));
            }
        }
        _ => {}
    }
}

fn path_of(path: &str) -> String {
    if path.is_empty() {
        "<root>".to_owned()
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::latency::LatencyTracker;

    fn sample_report() -> SupportReport {
        let mut report = SupportReport::new("build-abc123", 1_700_000_000_000);
        let health = crate::HealthRegistry::new();
        health.observe_fact("asr", "model_loaded", true);
        health.observe_fact("ipc", "server_up", false);
        report.set_health(health.snapshot());
        let tracker = LatencyTracker::new();
        for micros in [100u64, 200, 300, 400, 900] {
            tracker.record("asr_decode", micros);
        }
        report.set_latency(LatencySummary::from(&tracker));
        report.add_environment("os", "linux");
        report.add_environment("arch", "x86_64");
        report
    }

    #[test]
    fn schema_version_is_one() {
        assert_eq!(sample_report().schema_version, 1);
    }

    #[test]
    fn pretty_json_round_trip_preserves_report() {
        let report = sample_report();
        let text = report
            .to_json_pretty()
            .unwrap_or_else(|e| format!("serialize failed: {e}"));
        assert!(text.contains('\n'), "output should be pretty-printed");
        let back = SupportReport::from_json_str(&text)
            .unwrap_or_else(|e| SupportReport::new(format!("parse failed: {e}"), 0));
        assert_eq!(back, report);
    }

    #[test]
    fn default_serialization_scrubs_speech_carrying_environment_keys() {
        let mut report = sample_report();
        report.add_environment("last_delivered", "the invoice goes to accounting");
        report.add_environment("asr_words", "hello world");
        report.add_environment("spoken", "please send the invoice");
        let out_text = report
            .to_json_pretty()
            .unwrap_or_else(|e| format!("serialize failed: {e}"));

        assert!(!out_text.contains("the invoice goes to accounting"));
        assert!(!out_text.contains("please send the invoice"));
        assert!(
            out_text.contains("last_delivered\": \"[REDACTED:"),
            "{out_text}"
        );
        assert!(out_text.contains("spoken\": \"[REDACTED:"), "{out_text}");
        assert!(
            out_text.contains("asr_words\": \"[REDACTED:11]"),
            "{out_text}"
        );
        assert!(out_text.contains("\"os\": \"linux\""), "{out_text}");
    }

    #[test]
    fn malformed_json_is_a_typed_error() {
        let err = SupportReport::from_json_str("{ not json");
        assert!(matches!(err, Err(DiagnosticsError::Json(_))));
    }

    #[test]
    fn report_contains_only_explicit_inputs() {
        let text = sample_report()
            .to_json_pretty()
            .unwrap_or_else(|e| format!("serialize failed: {e}"));
        // No accidental environment dumps: only the two chosen keys exist.
        assert!(text.contains("\"os\""));
        assert!(text.contains("\"arch\""));
        assert!(!text.to_lowercase().contains("path"));
        assert!(!text.to_lowercase().contains("home"));
    }

    #[test]
    fn default_serialization_scrubs_transcript_fields() {
        let mut report = sample_report();
        // A caller mistake: sensitive content placed in the report.
        report.add_environment("transcript", "the user dictated secret words");
        report.add_environment("session_note", "audio=raw_pcm_bytes; ok=1");
        let out_text = report
            .to_json_pretty()
            .unwrap_or_else(|e| format!("serialize failed: {e}"));

        assert!(!out_text.contains("the user dictated secret words"));
        assert!(!out_text.contains("raw_pcm_bytes"));
        assert!(out_text.contains("[REDACTED:"));
        // Safe fields survive untouched.
        let output: serde_json::Value =
            serde_json::from_str(&out_text).unwrap_or(serde_json::Value::Null);
        assert_eq!(output["environment"]["os"], "linux");
        assert_eq!(output["build_id"], "build-abc123");
        // The scrubbed kv pair inside session_note lost its value.
        assert!(out_text.contains("audio=[REDACTED:"));
    }

    #[test]
    fn content_check_passes_for_a_report_built_from_content_free_inputs() {
        let check = sample_report().content_check();
        assert!(check.content_free, "{:?}", check.denied_fields);
        assert!(check.denied_fields.is_empty(), "{:?}", check.denied_fields);
        assert!(
            check.overlong_fields.is_empty(),
            "{:?}",
            check.overlong_fields
        );
        assert_eq!(
            check.summary(),
            "content_check=clean denied_fields=0 overlong_fields=0"
        );
    }

    #[test]
    fn content_check_reports_the_denied_field_that_would_be_redacted() {
        let mut report = sample_report();
        report.add_environment("last_delivered", "the invoice goes to accounting");
        report.add_environment("session_note", "audio=raw_pcm_bytes; ok=1");
        let check = report.content_check();
        assert!(!check.content_free, "{check:?}");
        assert!(
            check
                .denied_fields
                .contains(&"environment.last_delivered".to_owned()),
            "{:?}",
            check.denied_fields
        );
        // An embedded denied pair is found even under an innocent field name.
        assert!(
            check
                .denied_fields
                .contains(&"environment.session_note".to_owned()),
            "{:?}",
            check.denied_fields
        );
        assert!(
            check.summary().starts_with("content_check=denied"),
            "{}",
            check.summary()
        );
        // The same report still serializes to content-free text: the check
        // reports the caller mistake, the serializer removes it.
        let text = report
            .to_json_pretty()
            .unwrap_or_else(|e| format!("serialize failed: {e}"));
        assert!(!text.contains("the invoice goes to accounting"), "{text}");
        assert!(!text.contains("raw_pcm_bytes"), "{text}");
    }

    #[test]
    fn content_check_separates_truncation_from_a_content_leak() {
        let mut report = sample_report();
        report.add_environment("build_note", "a".repeat(1_000));
        let check = report.content_check();
        // Truncation is a fidelity loss, never a content leak.
        assert!(check.content_free, "{check:?}");
        assert!(check.denied_fields.is_empty(), "{:?}", check.denied_fields);
        assert_eq!(check.overlong_fields, ["environment.build_note"]);
        assert!(
            check.summary().ends_with("overlong_fields=1"),
            "{}",
            check.summary()
        );
    }

    #[test]
    fn content_check_walks_nested_health_and_latency_values() {
        let mut report = sample_report();
        // `code` is a HealthFact field: subtoken matching keeps it out of the
        // deny-list, so a free-form message there is what the check must catch.
        report.health.push(HealthFact::new("asr", "decode", true));
        let check = report.content_check();
        assert!(check.content_free, "{check:?}");

        report.health.clear();
        report
            .health
            .push(HealthFact::new("asr", "spoken=the invoice", true));
        let check = report.content_check();
        assert!(!check.content_free, "{check:?}");
        assert!(
            check.denied_fields.contains(&"health[0].code".to_owned()),
            "{:?}",
            check.denied_fields
        );
    }
}
