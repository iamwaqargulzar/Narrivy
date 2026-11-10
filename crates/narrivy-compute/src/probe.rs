//! The host-fact seam for truthful compute discovery.
//!
//! This crate owns *what* a probe has to establish before a device may be
//! planned onto, and it owns nothing about the machine it runs on. No module
//! here spawns a process, reads or rewrites the environment, or touches a
//! device node, so the crate builds, lints and tests identically on every
//! target the workspace supports. Composition injects a [`HostProbe`]; the
//! product implementation is the Linux one in `adapters/platform-linux`, and
//! the demo implementation lives in `examples/support/host_probe.rs`.
//!
//! There is deliberately no default implementation. A zero-effort `HostProbe`
//! that answers "no devices, no runtime, no cores" is exactly the shape that
//! turns an unprobed host into a false negative claim, and
//! [`crate::discovery`] must never be able to reach one by accident.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The environment a probe subprocess is allowed to see.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProbeEnvironment {
    /// Start from an empty environment holding exactly these variables. This
    /// is the default because a probe that inherits everything also inherits
    /// whatever the daemon's own environment happens to contain.
    #[default]
    Scrubbed,
    /// Inherit the caller's environment unchanged. Only correct for helpers
    /// whose environment the caller has already reviewed.
    Inherited,
}

/// One bounded helper invocation, described as data.
///
/// The portable crate decides *which* probe answers a question; the injected
/// [`HostProbe`] decides *how* to run it. Keeping the request a value is what
/// lets a test assert the exact program, arguments and environment a decision
/// depends on, instead of only that "some command was run".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeCommand {
    program: PathBuf,
    args: Vec<OsString>,
    environment: ProbeEnvironment,
    published: Vec<(String, String)>,
}

impl ProbeCommand {
    /// A probe that starts from an empty environment.
    pub fn scrubbed(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            environment: ProbeEnvironment::Scrubbed,
            published: Vec::new(),
        }
    }

    /// A probe that starts from an empty environment publishing exactly
    /// `variables`.
    pub fn scrubbed_with_env(
        program: impl Into<PathBuf>,
        variables: Vec<(String, String)>,
    ) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            environment: ProbeEnvironment::Scrubbed,
            published: variables,
        }
    }

    /// A probe that inherits the caller's environment.
    pub fn inheriting_environment(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            environment: ProbeEnvironment::Inherited,
            published: Vec::new(),
        }
    }

    /// Add one argument. Arguments stay `OsString` so a non-UTF-8 path is
    /// passed through byte-for-byte instead of being lossily rewritten.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add several arguments in order.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Publish exactly one variable to a scrubbed probe.
    ///
    /// Returns `None` for a probe that inherits its environment, which has no
    /// environment left to publish into. Reporting that instead of accepting
    /// the value and dropping it keeps a request builder honest.
    pub fn publish_env(mut self, key: &str, value: &str) -> Option<Self> {
        if self.environment == ProbeEnvironment::Inherited {
            return None;
        }
        self.published.push((key.to_owned(), value.to_owned()));
        Some(self)
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The arguments, in order.
    pub fn arguments(&self) -> &[OsString] {
        &self.args
    }

    pub fn environment(&self) -> &ProbeEnvironment {
        &self.environment
    }

    /// The variables a scrubbed probe publishes, in order. Empty for a probe
    /// that inherits its environment.
    pub fn published_environment(&self) -> &[(String, String)] {
        &self.published
    }
}

/// What a completed probe reported.
///
/// Standard error is deliberately absent: no probe decision in this crate
/// reads it, and carrying it would invite a helper's diagnostics leaking into
/// ordinary logs, which [`crate::discovery`] must never do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    pub success: bool,
    pub stdout: String,
}

impl ProbeOutcome {
    /// A probe that ran to completion and reported failure.
    pub fn failed(stdout: impl Into<String>) -> Self {
        Self {
            success: false,
            stdout: stdout.into(),
        }
    }

    /// A probe that ran to completion and reported success.
    pub fn succeeded(stdout: impl Into<String>) -> Self {
        Self {
            success: true,
            stdout: stdout.into(),
        }
    }
}

/// A probe that could not answer.
///
/// Discovery treats every one of these exactly as it treats "the host does not
/// have it", which is the only truthful reading of a fact nobody managed to
/// observe: an unanswerable probe never becomes a capability claim.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    #[error("probe {program} could not be run: {reason}")]
    Unavailable { program: String, reason: String },
    #[error("probe {program} exceeded its {timeout_secs}s budget")]
    TimedOut { program: String, timeout_secs: u64 },
}

impl ProbeError {
    /// Build the "this program is not runnable here" error.
    pub fn unavailable(program: &Path, reason: impl Into<String>) -> Self {
        Self::Unavailable {
            program: program.display().to_string(),
            reason: reason.into(),
        }
    }

    /// Build the "this program outlived its budget" error.
    pub fn timed_out(program: &Path, timeout: Duration) -> Self {
        Self::TimedOut {
            program: program.display().to_string(),
            timeout_secs: timeout.as_secs(),
        }
    }
}

/// Every fact about the host that compute planning is allowed to observe.
///
/// Implementations live outside this crate. They must treat an unreadable
/// device directory, a missing helper and a refused core count as "unknown"
/// and report it that way, never as a claim.
pub trait HostProbe: Send + Sync {
    /// Accelerator device nodes the host exposes, in ascending order.
    ///
    /// A node name is enumeration and nothing more: it says the kernel
    /// published a device, not that any runtime can drive it.
    fn accel_nodes(&self) -> Result<Vec<String>, ProbeError>;

    /// Run one probe to completion, killing it once `timeout` elapses.
    fn run_probe(
        &self,
        command: &ProbeCommand,
        timeout: Duration,
    ) -> Result<ProbeOutcome, ProbeError>;

    /// Logical cores this process may use, or `None` when the host declined to
    /// say. A CPU-only device is always available regardless, so `None` costs
    /// an estimate, never a capability.
    fn logical_cores(&self) -> Option<u32>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubbed_requests_start_from_an_empty_environment() {
        let command = ProbeCommand::scrubbed("python3").arg("-c");
        assert_eq!(command.program(), Path::new("python3"));
        assert_eq!(command.arguments(), [OsString::from("-c")]);
        assert_eq!(command.environment(), &ProbeEnvironment::Scrubbed);
        assert!(command.published_environment().is_empty());
        assert_eq!(ProbeEnvironment::default(), ProbeEnvironment::Scrubbed);
    }

    #[test]
    fn published_variables_replace_rather_than_extend_the_inherited_ones() {
        let published = ProbeCommand::scrubbed("python3")
            .publish_env("PATH", "/usr/bin")
            .and_then(|command| command.publish_env("LANG", "C"))
            .map(|command| command.published_environment().to_vec());
        assert_eq!(
            published,
            Some(vec![
                ("PATH".to_owned(), "/usr/bin".to_owned()),
                ("LANG".to_owned(), "C".to_owned()),
            ])
        );
    }

    #[test]
    fn a_scrubbed_probe_can_be_built_with_its_whole_environment_at_once() {
        let command = ProbeCommand::scrubbed_with_env(
            "python3",
            vec![("PATH".to_owned(), "/usr/bin".to_owned())],
        );
        assert_eq!(command.environment(), &ProbeEnvironment::Scrubbed);
        assert_eq!(
            command.published_environment(),
            [("PATH".to_owned(), "/usr/bin".to_owned())]
        );
    }

    #[test]
    fn an_inherited_probe_refuses_to_publish_instead_of_dropping_the_value() {
        let probe = ProbeCommand::inheriting_environment("lspci");
        assert!(
            probe.clone().publish_env("PATH", "/usr/bin").is_none(),
            "an inheriting probe must not pretend it published anything"
        );
        assert!(
            probe.published_environment().is_empty(),
            "an inheriting probe publishes nothing of its own"
        );
    }

    #[test]
    fn inherited_requests_carry_every_argument_in_order() {
        let command = ProbeCommand::inheriting_environment("/usr/bin/ldconfig").args(["-p", "-v"]);
        assert_eq!(command.environment(), &ProbeEnvironment::Inherited);
        assert_eq!(
            command.arguments(),
            [OsString::from("-p"), OsString::from("-v")]
        );
    }

    #[test]
    fn arguments_are_carried_as_os_strings_so_a_path_is_never_lossily_rewritten() {
        // A model path reaches the sidecar byte for byte: the request holds
        // `OsString`, so no UTF-8 round trip can corrupt a file name.
        let model = "/models/whisper-\u{e9}.gguf";
        let command = ProbeCommand::scrubbed("x").arg(Path::new(model));
        assert_eq!(command.arguments(), [OsString::from(model)]);
        assert_eq!(
            command.arguments()[0].as_encoded_bytes(),
            model.as_bytes(),
            "an argument must reach the helper exactly as it was given"
        );
    }

    #[test]
    fn probe_errors_name_the_program_and_stay_typed() {
        let error = ProbeError::unavailable(Path::new("/usr/bin/ldconfig"), "not installed");
        assert_eq!(
            error.to_string(),
            "probe /usr/bin/ldconfig could not be run: not installed"
        );
        let timeout = ProbeError::timed_out(Path::new("lspci"), Duration::from_secs(10));
        assert_eq!(timeout.to_string(), "probe lspci exceeded its 10s budget");
    }

    #[test]
    fn outcomes_separate_reported_failure_from_unrunnable_probes() {
        assert!(!ProbeOutcome::failed("").success);
        assert!(ProbeOutcome::succeeded("libopenvino.so => /usr/lib").success);
    }
}
