//! `narrivy-cli support-report`: the user-reachable support report (REL-008).
//!
//! The CLI is the diagnostic client. The daemon is long-running and owns the
//! diagnostics state, and it advertises that state as content-free capability
//! strings on `GetStatus`. This module renders exactly those advertised facts
//! into a `narrivy_diagnostics::SupportReport`:
//!
//! * `diagnostics.support-report=available` is the gate. When the daemon does
//!   not compose the surface it reports
//!   `diagnostics.support-report=unavailable:not_composed`, and the CLI fails
//!   closed instead of publishing a report a daemon cannot back.
//! * `health` is one fact per advertised capability: the first dotted segment
//!   is the subsystem, the remainder is the code, and the value's closed status
//!   token decides the verdict.
//! * `latency` is the daemon's advertised end-to-end percentile summary.
//! * `environment` holds only explicitly chosen client-side facts. The CLI
//!   never dumps its own process environment.
//!
//! The report is content-free by construction: `SupportReport::to_json_pretty`
//! applies `narrivy_diagnostics::Redactor` to every value, and
//! `SupportReport::content_check` is the pre-share verdict this command fails
//! closed on. No transcript, audio, clipboard/selection, token, secret, or
//! environment value can appear in the output.
//!
//! This complements `scripts/doctor.py`, which probes the *host* (desktop
//! session, portal, PipeWire, toolchain, disk) and never contacts the daemon.
//! The support report is the daemon-side half: composition, health, and
//! measured latency.

#![forbid(unsafe_code)]

use std::path::Path;

use narrivy_diagnostics::{ContentCheck, HealthFact, LatencySummary, SupportReport};

/// The subcommand a user runs to produce a shareable support report.
pub const SUPPORT_REPORT_COMMAND: &str = "support-report";

/// The capability a client gates its Diagnostics action on. It is emitted by
/// the daemon only when it really composes the content-free surface.
pub const SUPPORT_REPORT_AVAILABLE_CAPABILITY: &str = "diagnostics.support-report=available";
/// The fail-closed token for a daemon without that surface.
pub const SUPPORT_REPORT_UNAVAILABLE_CAPABILITY: &str =
    "diagnostics.support-report=unavailable:not_composed";
/// Prefix of the daemon's per-session latency percentiles.
pub const LATENCY_CAPABILITY_PREFIX: &str = "diagnostics.latency.";

/// Upper bound on health facts carried by one report, so a hostile or
/// future-peered daemon cannot inflate the output.
const MAX_HEALTH_FACTS: usize = 64;

/// The closed status vocabulary that makes a capability fact unhealthy.
///
/// Only the segment before the first `:` is compared, so
/// `unavailable:pipewire_recorder_missing` is judged by `unavailable` and
/// `disabled:not_composed` by `disabled`. Any other value — including a
/// presence-only capability with no `=` at all — is a healthy observation.
const UNHEALTHY_STATUS_TOKENS: [&str; 11] = [
    "unavailable",
    "disabled",
    "unverified",
    "unqualified",
    "invalid",
    "failed",
    "missing",
    "absent",
    "degraded",
    "not_composed",
    "rejected",
];

/// Value of one advertised capability, or `None` when it is not advertised.
#[must_use]
pub fn capability_value<'a>(capabilities: &'a [String], name: &str) -> Option<&'a str> {
    capabilities
        .iter()
        .filter_map(|capability| capability.split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

/// True when the daemon advertises a composed support-report surface.
#[must_use]
pub fn advertises_support_report(capabilities: &[String]) -> bool {
    capabilities
        .iter()
        .any(|capability| capability == SUPPORT_REPORT_AVAILABLE_CAPABILITY)
}

/// Verdict for one advertised capability value.
fn value_is_healthy(value: &str) -> bool {
    let head = value.split(':').next().unwrap_or(value);
    !UNHEALTHY_STATUS_TOKENS.contains(&head)
}

/// One health fact from one advertised capability.
///
/// `name.value` becomes `subsystem` = the first dotted segment, `code` = the
/// rest, and the value's closed status token becomes the verdict. The value
/// itself is **never copied** into the report: a fact is an identity plus a
/// boolean, so no daemon-supplied string of any shape can reach a shareable
/// report through this path. A presence-only capability (no `=`) is judged
/// healthy: the daemon advertising it at all is the observation. The
/// `diagnostics.latency.*` tokens are skipped because they feed the latency
/// summary instead.
fn health_fact_from_capability(capability: &str) -> Option<HealthFact> {
    if capability.starts_with(LATENCY_CAPABILITY_PREFIX) {
        return None;
    }
    let (name, value) = capability.split_once('=').unwrap_or((capability, ""));
    if name.is_empty() {
        return None;
    }
    let (subsystem, code) = match name.split_once('.') {
        Some((subsystem, code)) => (subsystem, code),
        None => ("daemon", name),
    };
    Some(HealthFact::new(subsystem, code, value_is_healthy(value)))
}

/// The daemon's advertised latency summary, or the empty summary when the
/// daemon has not retained a durable session yet.
#[must_use]
pub fn latency_from_capabilities(capabilities: &[String]) -> LatencySummary {
    let count = capability_value(capabilities, "diagnostics.latency.sessions")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    LatencySummary {
        count,
        p50_micros: latency_micros(capabilities, "diagnostics.latency.p50_us"),
        p95_micros: latency_micros(capabilities, "diagnostics.latency.p95_us"),
    }
}

fn latency_micros(capabilities: &[String], name: &str) -> Option<u64> {
    capability_value(capabilities, name).and_then(|value| value.parse::<u64>().ok())
}

/// Build the report a client renders from one status snapshot.
///
/// The caller has already authenticated the daemon and holds a content-free
/// [`narrivy_ipc::StatusDto`]; nothing here reads the journal, the retained
/// History, or any transcript.
///
/// # Errors
/// Returns a content-free message when the daemon does not compose the
/// support-report surface, so the caller fails closed.
pub fn report_from_status(
    status: &narrivy_ipc::StatusDto,
    generated_unix_ms: u64,
) -> Result<SupportReport, String> {
    if !advertises_support_report(&status.capabilities) {
        return Err(format!(
            "the daemon does not compose the support-report surface ({SUPPORT_REPORT_UNAVAILABLE_CAPABILITY}); refusing to publish a partial report"
        ));
    }
    let mut report = SupportReport::new(status.daemon_build.clone(), generated_unix_ms);
    let mut health: Vec<HealthFact> = status
        .capabilities
        .iter()
        .filter_map(|capability| health_fact_from_capability(capability))
        .collect();
    health.truncate(MAX_HEALTH_FACTS);
    // Deterministic order so two reports from the same status compare equal.
    health.sort_by(|left, right| {
        (left.subsystem.as_str(), left.code.as_str())
            .cmp(&(right.subsystem.as_str(), right.code.as_str()))
    });
    report.set_health(health);
    report.set_latency(latency_from_capabilities(&status.capabilities));
    // Explicitly chosen client-side facts only. This is never an environment
    // dump: the process environment is not read at all. The active pipeline id
    // is deliberately absent — it is a client-supplied identity string, and the
    // selected composition is already visible through the `pipeline.*` facts.
    report.add_environment("client", "narrivy-cli");
    report.add_environment("client_version", env!("CARGO_PKG_VERSION"));
    report.add_environment("os", std::env::consts::OS);
    report.add_environment("arch", std::env::consts::ARCH);
    report.add_environment("daemon_build", status.daemon_build.clone());
    report.add_environment("daemon_protocol_minor", status.protocol_minor.to_string());
    report.add_environment(
        "daemon_active_session",
        status
            .active_session
            .as_ref()
            .map_or("none", |active| active.state.as_str()),
    );
    Ok(report)
}

/// Serialize one report and verify it is shareable before anything is written.
///
/// The redactor runs inside `to_json_pretty`; the content check then states
/// that the report itself needed no redaction at all. A report that needed
/// redaction is refused rather than shipped, so a caller mistake can never
/// reach a bug report as a field of markers.
///
/// # Errors
/// Returns a content-free message when the report is not content-free, or when
/// it cannot be serialized.
pub fn render_shareable(report: &SupportReport) -> Result<(String, ContentCheck), String> {
    let text = report
        .to_json_pretty()
        .map_err(|error| format!("support report serialization failed: {error}"))?;
    let check = report.content_check();
    if !check.content_free {
        return Err(format!(
            "support report withheld: {} fields carry denied content",
            check.denied_fields.len()
        ));
    }
    Ok((text, check))
}

/// Write the rendered report to `output`, or return it for stdout.
pub fn emit(text: &str, check: &ContentCheck, output: Option<&Path>) -> Result<(), String> {
    match output {
        None => {
            print!("{text}");
            println!();
            eprintln!(
                "narrivy-cli {}: {}",
                SUPPORT_REPORT_COMMAND,
                check.summary()
            );
            Ok(())
        }
        Some(path) => write_report_file(path, text),
    }
}

/// Write the report with owner-only permissions: it is a file a user attaches
/// to a bug report, so it must not be world-readable by default.
fn write_report_file(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|error| {
            format!(
                "support report file {} could not be opened ({:?})",
                path.display(),
                error.kind()
            )
        })?;
    file.write_all(text.as_bytes()).map_err(|error| {
        format!(
            "support report file {} could not be written ({:?})",
            path.display(),
            error.kind()
        )
    })?;
    file.write_all(b"\n").map_err(|error| {
        format!(
            "support report file {} could not be written ({:?})",
            path.display(),
            error.kind()
        )
    })?;
    set_owner_only(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        format!(
            "support report file {} permissions could not be restricted ({:?})",
            path.display(),
            error.kind()
        )
    })
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const CANARY: &str = "the invoice goes to accounting on thursday";

    /// A daemon status with a composed support-report surface.
    fn composed_status(capabilities: &[&str]) -> narrivy_ipc::StatusDto {
        narrivy_ipc::StatusDto {
            daemon_build: "0.0.0".to_owned(),
            protocol_minor: narrivy_ipc::PROTOCOL_MINOR,
            active_session: None,
            last_session_timings_us: None,
            capabilities: capabilities
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            installed_model: None,
            portal_keyboard: None,
            model_provisioning: None,
        }
    }

    /// A realistic strict-lane capability set with real unavailable facts.
    fn default_capabilities() -> Vec<&'static str> {
        vec![
            "mock.asr",
            "null.delivery",
            "history.optional=enabled",
            "history.retention=7d",
            "activation.atomic-command=available",
            "activation.portal-global-shortcuts=unavailable:not_probed",
            "asr.transcribe-cpp.cpu=unavailable:resident_not_initialized",
            "asr.transcribe-cpp.runtime=resident-worker",
            "delivery.inbox-recovery=enabled",
            "diagnostics.support-report=available",
            "diagnostics.health.overall=ok",
            "diagnostics.health.subsystems=2",
            "diagnostics.latency.sessions=3",
            "diagnostics.latency.p50_us=120000",
            "diagnostics.latency.p95_us=480000",
        ]
    }

    #[test]
    fn an_advertised_surface_produces_a_report() {
        let report = report_from_status(&composed_status(&default_capabilities()), 42)
            .expect("a composed surface must produce a report");
        assert_eq!(report.generated_unix_ms, 42);
        assert_eq!(report.build_id, "0.0.0");
        assert_eq!(report.latency.count, 3);
        assert_eq!(report.latency.p50_micros, Some(120_000));
        assert_eq!(report.latency.p95_micros, Some(480_000));
        assert!(report.environment.contains_key("os"));
        assert!(report.environment.contains_key("arch"));
        // Every advertised capability but the three latency tokens becomes one
        // health fact.
        assert_eq!(report.health.len(), default_capabilities().len() - 3);
        let (text, check) = render_shareable(&report).expect("the report must be shareable");
        assert!(check.content_free, "{check:?}");
        assert!(text.contains("\"schema_version\": 1"), "{text}");
    }

    #[test]
    fn an_uncomposed_surface_fails_closed() {
        let error = report_from_status(
            &composed_status(&[
                "mock.asr",
                "diagnostics.support-report=unavailable:not_composed",
            ]),
            42,
        )
        .expect_err("an uncomposed surface must not produce a report");
        assert!(error.contains("not_composed"), "{error}");
        assert!(
            !error.contains(CANARY),
            "the failure message must stay content-free: {error}"
        );
    }

    #[test]
    fn health_facts_split_domain_and_code_with_a_closed_verdict() {
        let report = report_from_status(&composed_status(&default_capabilities()), 1)
            .expect("a composed surface must produce a report");
        let fact = report
            .health
            .iter()
            .find(|fact| fact.subsystem == "asr" && fact.code == "transcribe-cpp.cpu")
            .expect("the ASR fact must be present");
        assert!(!fact.ok, "a missing engine is not healthy");

        let unavailable = report
            .health
            .iter()
            .find(|fact| fact.code == "portal-global-shortcuts")
            .expect("the portal fact must be present");
        assert!(!unavailable.ok, "an unprobed portal is not healthy");

        let presence = report
            .health
            .iter()
            .find(|fact| fact.code == "asr" && fact.subsystem == "mock")
            .expect("the presence-only ASR fact must be present");
        assert!(
            presence.ok,
            "a presence-only capability is a healthy observation"
        );

        let degraded = report
            .health
            .iter()
            .find(|fact| fact.subsystem == "diagnostics" && fact.code == "health.overall")
            .expect("the overall health fact must be present");
        assert!(degraded.ok);
    }

    #[test]
    fn the_report_carries_no_transcript_even_when_a_canary_is_advertised() {
        // A misbehaving daemon that puts content into a capability value can
        // never get it into a shareable report: the derivation keeps only the
        // fact's domain, code and verdict, never the raw value.
        let mut capabilities = default_capabilities();
        capabilities.push("diagnostics.notes=text=the invoice goes to accounting on thursday");
        capabilities.push("diagnostics.raw=words=the invoice goes to accounting on thursday");
        let mut status = composed_status(&capabilities);
        active_session_with_canary_identity(&mut status);

        let report = report_from_status(&status, 7).expect("a composed surface produces a report");
        let (text, check) = render_shareable(&report).expect("the report must be shareable");
        assert!(check.content_free, "{check:?}");
        assert!(!text.contains(CANARY), "{text}");
        // The fact itself is still reported, without its value.
        assert!(
            report
                .health
                .iter()
                .any(|fact| fact.subsystem == "diagnostics" && fact.code == "notes"),
            "{:?}",
            report.health
        );
        // The active transaction and pipeline identities are never read.
        assert!(!text.contains("tx-"), "{text}");
    }

    #[test]
    fn a_denied_value_that_does_reach_the_report_is_withheld() {
        // `daemon_build` is copied verbatim, so the fail-closed content check
        // is the last line of defence for any daemon-composed string.
        let mut status = composed_status(&default_capabilities());
        status.daemon_build = format!("0.0.0 text={CANARY}");
        let report = report_from_status(&status, 7).expect("a composed surface produces a report");
        let error = render_shareable(&report).expect_err("a denied report must be withheld");
        assert!(error.contains("denied content"), "{error}");
        // Even the serialization the caller could have written carries none.
        let text = report
            .to_json_pretty()
            .unwrap_or_else(|error| format!("serialization failed: {error}"));
        assert!(!text.contains(CANARY), "{text}");
    }

    /// Put the canary in the active-session identity fields too: an active
    /// session id, transaction id and pipeline id are the only other
    /// client-visible values a report could plausibly copy.
    fn active_session_with_canary_identity(status: &mut narrivy_ipc::StatusDto) {
        status.active_session = Some(narrivy_ipc::ActiveSessionDto {
            session_id: 7,
            transaction_id: Some(format!("tx-{CANARY}")),
            pipeline_id: Some(CANARY.to_owned()),
            state: "listening".to_owned(),
            stage_timings_us: None,
        });
    }

    #[test]
    fn active_session_identity_never_reaches_the_report() {
        let mut status = composed_status(&default_capabilities());
        active_session_with_canary_identity(&mut status);
        let report = report_from_status(&status, 7).expect("a composed surface produces a report");
        let (text, check) = render_shareable(&report).expect("the report must be shareable");
        assert!(check.content_free, "{check:?}");
        assert!(!text.contains(CANARY), "{text}");
        assert!(
            text.contains("\"daemon_active_session\": \"listening\""),
            "{text}"
        );
    }

    #[test]
    fn a_real_report_never_dumps_the_process_environment() {
        let report = report_from_status(&composed_status(&default_capabilities()), 7)
            .expect("a composed surface produces a report");
        // Exactly the explicitly chosen facts, and nothing else: the process
        // environment is never read, so no variable can reach the report.
        assert_eq!(
            report
                .environment
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "arch",
                "client",
                "client_version",
                "daemon_active_session",
                "daemon_build",
                "daemon_protocol_minor",
                "os",
            ]
        );
        let (text, check) = render_shareable(&report).expect("the report must be shareable");
        assert!(check.content_free, "{check:?}");
        assert!(!text.contains(CANARY), "{text}");
        // No environment-style `KEY=value` pair other than the chosen facts.
        assert!(!text.to_lowercase().contains("narrivy_socket"), "{text}");
    }

    #[test]
    fn latency_facts_come_only_from_the_advertised_percentiles() {
        let capabilities: Vec<String> = default_capabilities()
            .iter()
            .map(|value| (*value).to_owned())
            .collect();
        let summary = latency_from_capabilities(&capabilities);
        assert_eq!(summary.count, 3);
        assert_eq!(summary.p50_micros, Some(120_000));
        assert_eq!(summary.p95_micros, Some(480_000));

        // No retained session yet: an explicit absence, never a zero that
        // would read as an impossibly fast session.
        let empty = latency_from_capabilities(&["diagnostics.latency.sessions=0".to_owned()]);
        assert_eq!(empty.count, 0);
        assert_eq!(empty.p50_micros, None);
        assert_eq!(empty.p95_micros, None);
        // Latency tokens never become health facts.
        let report = report_from_status(
            &composed_status(&[
                "diagnostics.support-report=available",
                "diagnostics.latency.sessions=1",
            ]),
            1,
        )
        .expect("a composed surface produces a report");
        assert_eq!(report.health.len(), 1, "{:?}", report.health);
        assert!(
            report
                .health
                .iter()
                .all(|fact| !fact.code.starts_with("latency")),
            "{:?}",
            report.health
        );
    }

    #[test]
    fn a_written_report_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("support-report.json");
        let report = report_from_status(&composed_status(&default_capabilities()), 7)
            .expect("a composed surface produces a report");
        let (text, check) = render_shareable(&report).expect("the report must be shareable");
        emit(&text, &check, Some(&path)).expect("the report must be written");
        let written = std::fs::read_to_string(&path).expect("the report file must exist");
        assert!(written.contains("\"schema_version\": 1"), "{written}");
        assert!(!written.contains(CANARY), "{written}");
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600, "a shareable report is owner-only");
    }

    #[test]
    fn the_capability_gate_is_read_from_the_daemon_not_assumed() {
        assert!(advertises_support_report(&[
            SUPPORT_REPORT_AVAILABLE_CAPABILITY.to_owned()
        ]));
        assert!(!advertises_support_report(&[
            SUPPORT_REPORT_UNAVAILABLE_CAPABILITY.to_owned()
        ]));
        assert!(!advertises_support_report(&["mock.asr".to_owned()]));
        // A value that merely looks similar is not the gate.
        assert!(!advertises_support_report(&[
            "diagnostics.support-report=available:soon".to_owned()
        ]));
        assert!(!advertises_support_report(&[
            "xdiagnostics.support-report=available".to_owned()
        ]));
    }
}
