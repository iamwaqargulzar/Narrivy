//! Daemon-owned content-safe diagnostics surface.
//!
//! This module is the production composition of `narrivy-diagnostics`. It has
//! exactly two jobs, both of them content-free:
//!
//! 1. **Redaction** — [`content_safe`] routes every free-form diagnostic
//!    message the daemon emits through [`Redactor`], so a pathological
//!    engine/adapter message can never carry transcript-shaped, audio or
//!    credential content into an ordinary log sink.
//! 2. **Support-report facts** — [`DaemonDiagnostics`] owns the daemon's
//!    health registry and latency ring. A diagnostic client renders a shareable
//!    report from the capabilities this module advertises; the daemon itself
//!    never writes a report file and never exposes user content.
//!
//! The surface is an optional composition, exactly like the retained History
//! store: a deployment that must not retain per-session timing samples can
//! leave it uncomposed, and the advertised capability then fails closed with
//! `unavailable:not_composed` instead of claiming a report it cannot back.
//!
//! Security invariants (AGENTS.md): subsystem and code identifiers here are
//! short stable tokens; no transcript, audio, clipboard/selection, token,
//! secret, or environment value ever reaches this module.

#![forbid(unsafe_code)]

use narrivy_platform_linux::accelerator_usage::{self, AcceleratorCounters, GpuWork};
use std::sync::OnceLock;
use std::time::Duration;

use narrivy_core::RecoveryTerminal;
use narrivy_diagnostics::{HealthFact, HealthRegistry, LatencySummary, LatencyTracker, Redactor};
use narrivy_types::DeliveryOutcome;

/// Emitted only when this composition really owns a diagnostics surface. A
/// client uses it as the gate for offering a support-report action.
pub const SUPPORT_REPORT_AVAILABLE_CAPABILITY: &str = "diagnostics.support-report=available";
/// Emitted when the composition root left the surface out, so no client may
/// offer a support report built from daemon facts.
pub const SUPPORT_REPORT_NOT_COMPOSED_CAPABILITY: &str =
    "diagnostics.support-report=unavailable:not_composed";
/// Capability prefix carrying the daemon's per-session latency summary. A
/// client reads these three tokens into one latency summary.
pub const LATENCY_CAPABILITY_PREFIX: &str = "diagnostics.latency.";

/// The process-wide redaction facility for daemon diagnostics.
///
/// One immutable instance is shared by every connection and every owned worker
/// of a daemon generation, exactly like the IPC command policy, so two
/// emissions can never disagree about what is denied.
#[must_use]
pub fn diagnostic_redactor() -> &'static Redactor {
    static REDACTOR: OnceLock<Redactor> = OnceLock::new();
    REDACTOR.get_or_init(Redactor::new)
}

/// Scrub one free-form message before it reaches an ordinary log sink.
///
/// Every diagnostic emission that interpolates a foreign `Display` value must
/// pass it through here. The redactor removes denied `key=value` pairs and
/// bounds the retained length, so an adapter or ASR engine message that
/// embedded `text=...` or `audio=...` can never surface it verbatim.
#[must_use]
pub fn content_safe(message: &str) -> String {
    diagnostic_redactor().scrub_string(message)
}

/// Content-free closed vocabulary for one owned session's terminal outcome.
///
/// Every variant is a stable identifier. The daemon maps its own typed
/// outcomes onto this vocabulary so the health facts a support report carries
/// can never contain a free-form message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcome {
    /// The configured destination verified its durable result.
    Delivered,
    /// A direct helper accepted the payload; insertion unverified.
    SubmittedUnverified,
    /// Clipboard-only honesty: text is on the clipboard, insertion unproven.
    CopiedOnly,
    /// Durably stored in the Inbox for recovery.
    InboxFallback,
    /// Terminal failure after all bounded attempts; text stays recoverable.
    Failed,
    /// Cancelled before a terminal result existed.
    Cancelled,
    /// The worker left without recording a typed terminal outcome.
    Abandoned,
}

impl SessionOutcome {
    /// Stable content-free fact code for the health registry.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Delivered => "session_delivered",
            Self::SubmittedUnverified => "session_submitted_unverified",
            Self::CopiedOnly => "session_copied_only",
            Self::InboxFallback => "session_inbox_fallback",
            Self::Failed => "session_failed",
            Self::Cancelled => "session_cancelled",
            Self::Abandoned => "session_abandoned",
        }
    }

    /// True only when the session reached a durable delivery result. A
    /// cancelled, failed or abandoned attempt has no end-to-end duration worth
    /// reporting, so it contributes no latency sample.
    #[must_use]
    pub const fn records_latency(self) -> bool {
        matches!(
            self,
            Self::Delivered | Self::SubmittedUnverified | Self::CopiedOnly | Self::InboxFallback
        )
    }

    /// Health verdict for this outcome. Only a verified delivery is healthy.
    #[must_use]
    pub const fn healthy(self) -> bool {
        matches!(self, Self::Delivered)
    }

    /// Map one typed delivery outcome from the mock/portable lane.
    #[must_use]
    pub const fn from_delivery(outcome: DeliveryOutcome) -> Self {
        match outcome {
            DeliveryOutcome::Delivered => Self::Delivered,
            DeliveryOutcome::SubmittedUnverified => Self::SubmittedUnverified,
            DeliveryOutcome::CopiedOnly => Self::CopiedOnly,
            DeliveryOutcome::InboxFallback => Self::InboxFallback,
            DeliveryOutcome::Failed => Self::Failed,
        }
    }

    /// Map one typed recovery terminal from the strict lane.
    #[must_use]
    pub const fn from_terminal(terminal: &RecoveryTerminal) -> Self {
        match terminal {
            RecoveryTerminal::DeliveredInbox(_) | RecoveryTerminal::InboxRecovered(_) => {
                Self::InboxFallback
            }
            RecoveryTerminal::Cancelled => Self::Cancelled,
            RecoveryTerminal::FailedRecoverable { .. }
            | RecoveryTerminal::FailedTerminal { .. } => Self::Failed,
        }
    }
}

/// The daemon's content-free health and latency state.
///
/// Health is latest-wins per subsystem with bounded history, and latency is a
/// bounded ring of per-session durations in microseconds. Both are safe to
/// share: they hold stable identifiers and integers only.
#[derive(Debug, Default)]
pub struct DaemonDiagnostics {
    health: HealthRegistry,
    latency: LatencyTracker,
    resources: ResourceSampler,
}

/// Prefix of the content-free resource-usage capability tokens.
pub const RESOURCES_CAPABILITY_PREFIX: &str = "diagnostics.resources.";

/// Shortest interval between two samples that yields a CPU share; a shorter
/// gap would turn clock-tick rounding into noise.
const MIN_CPU_SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

/// Resource use of the daemon's own process tree (the service plus the helper
/// and worker processes it spawned). CPU share needs two samples, so the last
/// one is kept; the first status read reports the CPU share as warming up.
#[derive(Debug, Default)]
struct ResourceSampler {
    last: std::sync::Mutex<Option<(std::time::Instant, u64)>>,
    /// The GPU and NPU counters at the last sample (whole computer), and
    /// the GPU's engine work (with Narrivy's apart).
    devices: std::sync::Mutex<Option<(std::time::Instant, AcceleratorCounters, Option<GpuWork>)>>,
}

/// One resource report, content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceReport {
    pub rss_bytes: u64,
    pub processes: u32,
    /// Share of the whole machine's CPU since the previous sample, in
    /// thousandths; `None` until two samples exist.
    pub cpu_permille: Option<u32>,
    /// How busy the whole computer's GPU and NPU were since the previous
    /// sample, in thousandths; `Err` says why not (`no_device`,
    /// `warming_up`).
    pub gpu_permille: Result<u32, &'static str>,
    pub npu_permille: Result<u32, &'static str>,
    /// Of the GPU's use, what Narrivy's processes did (when engine work is
    /// counted per process).
    pub gpu_own_permille: Option<u32>,
}

impl ResourceSampler {
    fn sample(&self) -> Option<ResourceReport> {
        let usage = narrivy_process_linux::usage::process_tree_usage(std::process::id())?;
        let now = std::time::Instant::now();
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cpu_permille = match *last {
            Some((at, ticks)) if now.duration_since(at) >= MIN_CPU_SAMPLE_INTERVAL => {
                cpu_share_permille(
                    usage.cpu_ticks.saturating_sub(ticks),
                    now.duration_since(at),
                    narrivy_process_linux::usage::clock_ticks_per_second(),
                    narrivy_process_linux::usage::online_cpus(),
                )
            }
            _ => None,
        };
        if cpu_permille.is_some() || last.is_none() {
            *last = Some((now, usage.cpu_ticks));
        }
        drop(last);
        let (gpu_permille, npu_permille, gpu_own_permille) = self.devices_sample(now);
        Some(ResourceReport {
            rss_bytes: usage.rss_bytes,
            processes: usage.processes,
            cpu_permille,
            gpu_permille,
            npu_permille,
            gpu_own_permille,
        })
    }

    fn devices_sample(
        &self,
        now: std::time::Instant,
    ) -> (
        Result<u32, &'static str>,
        Result<u32, &'static str>,
        Option<u32>,
    ) {
        let read = accelerator_usage::counters();
        let own = narrivy_process_linux::usage::process_tree(std::process::id());
        let work = accelerator_usage::gpu_work(&own);
        let mut last = self
            .devices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let share = |present: bool, value: Option<u32>| match (present, value) {
            (false, _) => Err("no_device"),
            (true, Some(permille)) => Ok(permille),
            (true, None) => Err("warming_up"),
        };
        let (gpu, gpu_own, npu) = match &*last {
            Some((at, before, before_work))
                if now.duration_since(*at) >= MIN_CPU_SAMPLE_INTERVAL =>
            {
                let ms = u64::try_from(now.duration_since(*at).as_millis()).unwrap_or(u64::MAX);
                // Engine work when the driver counts it per client; else the
                // time the GPU was awake, which reads high.
                let worked = before_work
                    .as_ref()
                    .zip(work.as_ref())
                    .and_then(|(a, b)| accelerator_usage::gpu_work_permille(a, b));
                let (gpu, gpu_own) = match worked {
                    Some((all, own)) => (Some(all), Some(own)),
                    None => (
                        before
                            .gpu_idle_ms
                            .zip(read.gpu_idle_ms)
                            .and_then(|(a, b)| accelerator_usage::gpu_busy_permille(a, b, ms)),
                        None,
                    ),
                };
                let npu = before
                    .npu_busy_us
                    .zip(read.npu_busy_us)
                    .and_then(|(a, b)| accelerator_usage::npu_busy_permille(a, b, ms));
                (gpu, gpu_own, npu)
            }
            _ => (None, None, None),
        };
        let fresh = last
            .as_ref()
            .is_none_or(|(at, _, _)| now.duration_since(*at) >= MIN_CPU_SAMPLE_INTERVAL);
        let has_gpu = read.gpu_idle_ms.is_some() || work.is_some();
        let has_npu = read.npu_busy_us.is_some();
        if fresh {
            *last = Some((now, read, work));
        }
        (share(has_gpu, gpu), share(has_npu, npu), gpu_own)
    }
}

/// CPU time spent over wall time and every online CPU, in thousandths.
fn cpu_share_permille(
    ticks: u64,
    elapsed: Duration,
    ticks_per_second: u64,
    cpus: u64,
) -> Option<u32> {
    let capacity_micros = elapsed.as_micros().checked_mul(u128::from(cpus.max(1)))?;
    if capacity_micros == 0 || ticks_per_second == 0 {
        return None;
    }
    let used_micros = u128::from(ticks) * 1_000_000 / u128::from(ticks_per_second);
    u32::try_from((used_micros * 1000 / capacity_micros).min(1000)).ok()
}

impl DaemonDiagnostics {
    /// A diagnostics surface with the default bounds.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one finished owned session: a content-free outcome fact plus,
    /// for a durable result, the end-to-end duration of that attempt.
    pub fn observe_session(&self, outcome: SessionOutcome, elapsed: Duration) {
        self.health.observe(HealthFact::new(
            "session",
            outcome.code(),
            outcome.healthy(),
        ));
        if outcome.records_latency() {
            self.latency.record(
                "session_total",
                u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
            );
        }
    }

    /// Observe one content-free subsystem fact from the daemon's own
    /// composition state.
    pub fn observe_fact(&self, subsystem: &str, code: &str, ok: bool) {
        self.health.observe(HealthFact::new(subsystem, code, ok));
    }

    /// Latest health fact per subsystem, sorted by subsystem.
    #[must_use]
    pub fn health_snapshot(&self) -> Vec<HealthFact> {
        self.health.snapshot()
    }

    /// Latency percentiles over every retained durable session.
    #[must_use]
    pub fn latency_summary(&self) -> LatencySummary {
        LatencySummary::from(&self.latency)
    }

    /// Resource use of the daemon's process tree, or `None` when `/proc`
    /// cannot be read.
    pub fn resource_report(&self) -> Option<ResourceReport> {
        self.resources.sample()
    }

    /// True only when every observed subsystem fact is healthy.
    #[must_use]
    pub fn overall_healthy(&self) -> bool {
        self.health.healthy()
    }
}

/// Advertise the composed diagnostics surface as observed, never as a constant.
///
/// The support-report gate is emitted first so a client can stop reading the
/// remaining tokens. When the surface is not composed the only emitted token is
/// the fail-closed one.
pub fn append_diagnostics_capabilities(
    capabilities: &mut Vec<String>,
    diagnostics: Option<&DaemonDiagnostics>,
) {
    let Some(diagnostics) = diagnostics else {
        capabilities.push(SUPPORT_REPORT_NOT_COMPOSED_CAPABILITY.to_owned());
        return;
    };
    capabilities.push(SUPPORT_REPORT_AVAILABLE_CAPABILITY.to_owned());
    let health = diagnostics.health_snapshot();
    capabilities.push(format!(
        "diagnostics.health.overall={}",
        if diagnostics.overall_healthy() {
            "ok"
        } else {
            "degraded"
        }
    ));
    capabilities.push(format!("diagnostics.health.subsystems={}", health.len()));
    let latency = diagnostics.latency_summary();
    capabilities.push(format!(
        "{}sessions={}",
        LATENCY_CAPABILITY_PREFIX, latency.count
    ));
    for (name, value) in [
        ("p50_us", latency.p50_micros),
        ("p95_us", latency.p95_micros),
    ] {
        capabilities.push(match value {
            Some(micros) => format!("{LATENCY_CAPABILITY_PREFIX}{name}={micros}"),
            // An empty ring is reported as an explicit absence, never as 0,
            // which would read as an impossibly fast session.
            None => format!("{LATENCY_CAPABILITY_PREFIX}{name}=unavailable:no_samples"),
        });
    }
    match diagnostics.resource_report() {
        Some(report) => {
            capabilities.push(format!(
                "{RESOURCES_CAPABILITY_PREFIX}rss_bytes={}",
                report.rss_bytes
            ));
            capabilities.push(format!(
                "{RESOURCES_CAPABILITY_PREFIX}processes={}",
                report.processes
            ));
            capabilities.push(match report.cpu_permille {
                Some(permille) => format!("{RESOURCES_CAPABILITY_PREFIX}cpu_permille={permille}"),
                None => format!("{RESOURCES_CAPABILITY_PREFIX}cpu_permille=unavailable:warming_up"),
            });
            if let Some(own) = report.gpu_own_permille {
                capabilities.push(format!(
                    "{RESOURCES_CAPABILITY_PREFIX}gpu_own_permille={own}"
                ));
            }
            for (name, share) in [
                ("gpu_permille", report.gpu_permille),
                ("npu_permille", report.npu_permille),
            ] {
                capabilities.push(match share {
                    Ok(permille) => format!("{RESOURCES_CAPABILITY_PREFIX}{name}={permille}"),
                    Err(why) => format!("{RESOURCES_CAPABILITY_PREFIX}{name}=unavailable:{why}"),
                });
            }
        }
        None => capabilities.push(format!(
            "{RESOURCES_CAPABILITY_PREFIX}rss_bytes=unavailable:unreadable"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_share_is_bounded_and_scaled_by_cpus() {
        // 100 ticks at 100 Hz = 1 s of CPU over 1 s on 4 CPUs = 25 %.
        assert_eq!(
            cpu_share_permille(100, Duration::from_secs(1), 100, 4),
            Some(250)
        );
        // Never above the whole machine.
        assert_eq!(
            cpu_share_permille(10_000, Duration::from_secs(1), 100, 1),
            Some(1000)
        );
        assert_eq!(cpu_share_permille(5, Duration::ZERO, 100, 4), None);
    }

    #[test]
    fn resources_are_reported_and_cpu_warms_up_first() {
        let diagnostics = DaemonDiagnostics::new();
        let first = advertised(Some(&diagnostics));
        assert!(first.iter().any(
            |c| c.starts_with("diagnostics.resources.rss_bytes=") && !c.contains("unavailable")
        ));
        assert!(
            first.contains(&"diagnostics.resources.cpu_permille=unavailable:warming_up".to_owned())
        );
        std::thread::sleep(MIN_CPU_SAMPLE_INTERVAL + Duration::from_millis(20));
        let second = advertised(Some(&diagnostics));
        assert!(
            second
                .iter()
                .any(|c| c.starts_with("diagnostics.resources.cpu_permille=")
                    && !c.contains("unavailable"))
        );
    }

    /// One composed surface advertised to a client.
    fn advertised(diagnostics: Option<&DaemonDiagnostics>) -> Vec<String> {
        let mut capabilities = Vec::new();
        append_diagnostics_capabilities(&mut capabilities, diagnostics);
        capabilities
    }

    #[test]
    fn composed_surface_advertises_the_support_report_gate() {
        let diagnostics = DaemonDiagnostics::new();
        let capabilities = advertised(Some(&diagnostics));
        assert!(
            capabilities.contains(&SUPPORT_REPORT_AVAILABLE_CAPABILITY.to_owned()),
            "{capabilities:?}"
        );
        assert!(
            !capabilities.contains(&SUPPORT_REPORT_NOT_COMPOSED_CAPABILITY.to_owned()),
            "{capabilities:?}"
        );
        assert!(
            capabilities.contains(&"diagnostics.health.overall=ok".to_owned()),
            "{capabilities:?}"
        );
        assert!(
            capabilities.contains(&"diagnostics.latency.p50_us=unavailable:no_samples".to_owned()),
            "{capabilities:?}"
        );
    }

    #[test]
    fn uncomposed_surface_fails_closed_with_only_the_gate() {
        let capabilities = advertised(None);
        assert_eq!(
            capabilities,
            [SUPPORT_REPORT_NOT_COMPOSED_CAPABILITY.to_owned()]
        );
    }

    #[test]
    fn a_finished_session_updates_health_and_the_latency_ring() {
        let diagnostics = DaemonDiagnostics::new();
        diagnostics.observe_session(SessionOutcome::Delivered, Duration::from_millis(40));
        diagnostics.observe_session(SessionOutcome::InboxFallback, Duration::from_millis(60));
        let health = diagnostics.health_snapshot();
        // Latest-wins per subsystem: the last observed outcome is the fact.
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].code, "session_inbox_fallback");
        // An Inbox recovery is non-destructive but did not reach the
        // configured destination, so it degrades overall health while still
        // contributing a real latency sample.
        assert!(!health[0].ok);
        let latency = diagnostics.latency_summary();
        assert_eq!(latency.count, 2);
        // Nearest-rank over [40_000, 60_000]: p50 rank 1, p95 rank 2.
        assert_eq!(latency.p50_micros, Some(40_000));
        assert_eq!(latency.p95_micros, Some(60_000));
        assert!(!diagnostics.overall_healthy());
    }

    #[test]
    fn a_verified_delivery_is_the_only_healthy_outcome() {
        let diagnostics = DaemonDiagnostics::new();
        diagnostics.observe_session(SessionOutcome::Delivered, Duration::from_millis(40));
        assert!(diagnostics.overall_healthy());
        let capabilities = advertised(Some(&diagnostics));
        assert!(
            capabilities.contains(&"diagnostics.health.overall=ok".to_owned()),
            "{capabilities:?}"
        );
    }

    #[test]
    fn cancelled_and_failed_sessions_degrade_health_without_polluting_latency() {
        let diagnostics = DaemonDiagnostics::new();
        diagnostics.observe_session(SessionOutcome::Delivered, Duration::from_millis(10));
        diagnostics.observe_session(SessionOutcome::Cancelled, Duration::from_millis(5));
        let latency = diagnostics.latency_summary();
        assert_eq!(
            latency.count, 1,
            "a cancelled attempt is not a latency sample"
        );
        assert_eq!(latency.p50_micros, Some(10_000));
        assert!(!diagnostics.overall_healthy());

        let capabilities = advertised(Some(&diagnostics));
        assert!(
            capabilities.contains(&"diagnostics.health.overall=degraded".to_owned()),
            "{capabilities:?}"
        );
        assert!(
            capabilities.contains(&"diagnostics.latency.sessions=1".to_owned()),
            "{capabilities:?}"
        );
    }

    #[test]
    fn typed_outcomes_map_onto_the_closed_vocabulary() {
        assert_eq!(
            SessionOutcome::from_delivery(DeliveryOutcome::Delivered),
            SessionOutcome::Delivered
        );
        assert_eq!(
            SessionOutcome::from_delivery(DeliveryOutcome::Failed),
            SessionOutcome::Failed
        );
        assert_eq!(
            SessionOutcome::from_terminal(&RecoveryTerminal::Cancelled),
            SessionOutcome::Cancelled
        );
        // Every closed code survives the report redaction as a plain value, so
        // a support report built from these facts is never rewritten.
        let redactor = diagnostic_redactor();
        let diagnostics = DaemonDiagnostics::new();
        for outcome in [
            SessionOutcome::Delivered,
            SessionOutcome::SubmittedUnverified,
            SessionOutcome::CopiedOnly,
            SessionOutcome::InboxFallback,
            SessionOutcome::Failed,
            SessionOutcome::Cancelled,
            SessionOutcome::Abandoned,
        ] {
            assert!(!outcome.code().is_empty());
            assert_eq!(redactor.scrub_kv(outcome.code()), outcome.code());
            diagnostics.observe_fact("session", outcome.code(), outcome.healthy());
        }
        let facts =
            serde_json::to_value(diagnostics.health_snapshot()).unwrap_or(serde_json::Value::Null);
        assert_eq!(redactor.redact_json(&facts), facts);
    }

    #[test]
    fn free_form_diagnostic_messages_are_scrubbed() {
        // A hostile engine message embedded in an ordinary log sink.
        let hostile = "asr failed: text=the invoice goes to accounting audio=raw_pcm";
        let scrubbed = content_safe(hostile);
        assert!(
            !scrubbed.contains("the invoice goes to accounting"),
            "{scrubbed}"
        );
        assert!(!scrubbed.contains("raw_pcm"), "{scrubbed}");
        assert!(scrubbed.contains("[REDACTED:"), "{scrubbed}");
        assert!(scrubbed.starts_with("asr failed:"), "{scrubbed}");
        // An ordinary message survives unchanged.
        assert_eq!(
            content_safe("session 7 failed: worker_failure"),
            "session 7 failed: worker_failure"
        );
    }
}
