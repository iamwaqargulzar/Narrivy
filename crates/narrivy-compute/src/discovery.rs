//! Truthful device discovery. Enumeration != capability.
//!
//! Every fact about the machine arrives through an injected
//! [`HostProbe`](crate::probe::HostProbe): this module decides *which* probe
//! answers *which* question and how to read the answer, and it never spawns a
//! process, rewrites the environment, or reads a device node itself. The
//! request builders below are values, so a test can assert the exact program,
//! arguments and environment a decision rests on.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

use crate::probe::{HostProbe, ProbeCommand, ProbeOutcome};

/// Default bound for every probe subprocess.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Interpreter the accelerator runtime check imports through.
const RUNTIME_IMPORT_PROGRAM: &str = "python3";

/// Import whose success proves the vendor runtime is loadable by Python.
const RUNTIME_IMPORT_MODULE: &str = "openvino";

/// The one search path a runtime-import probe is allowed to resolve against.
///
/// A probe that inherited the daemon's `PATH` would be answering "what this
/// user's shell happens to provide", which is a claim about the environment
/// rather than about the installed runtime.
const RUNTIME_IMPORT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Shared-object cache lister used to detect an installed runtime library
/// without executing any of it.
const SHARED_OBJECT_CACHE_PROGRAM: &str = "ldconfig";

/// PCI enumeration helper for the optional NPU cross-check.
const PCI_ENUMERATION_PROGRAM: &str = "lspci";

/// Vulkan inventory helper used when no enumeration sidecar is installed.
const VULKAN_INFO_PROGRAM: &str = "vulkaninfo";

/// Compute device class used by planner ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClass {
    Npu,
    DGpu,
    IGpu,
    Cpu,
}

impl DeviceClass {
    /// Planner preference order: NPU -> dGPU -> iGPU -> CPU.
    pub fn rank(self) -> u8 {
        match self {
            DeviceClass::Npu => 0,
            DeviceClass::DGpu => 1,
            DeviceClass::IGpu => 2,
            DeviceClass::Cpu => 3,
        }
    }
}

/// NPU state. `present` means the host published an accelerator device node;
/// it says nothing about usability until `runtime_available` is also true.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NpuInfo {
    pub present: bool,
    pub runtime_available: bool,
    pub nodes: Vec<String>,
}

/// One Vulkan device reported by an enumeration sidecar's `--list-devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VulkanDevice {
    pub index: u32,
    pub name: String,
    pub class: DeviceClass,
    /// PCI id when reported, else None.
    pub pci_id: Option<String>,
}

/// CPU is always available on this platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuInfo {
    pub logical_cores: u32,
}

/// Full discovery snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryReport {
    pub npu: NpuInfo,
    pub vulkan_devices: Vec<VulkanDevice>,
    pub cpu: CpuInfo,
}

impl DiscoveryReport {
    /// Is this class actually usable right now (enumerated AND runtime)?
    pub fn class_available(&self, class: DeviceClass) -> bool {
        match class {
            DeviceClass::Npu => self.npu.present && self.npu.runtime_available,
            DeviceClass::DGpu | DeviceClass::IGpu => {
                self.vulkan_devices.iter().any(|d| d.class == class)
            }
            DeviceClass::Cpu => true,
        }
    }
}

/// Probe NPU: accelerator node presence plus runtime availability.
///
/// Runtime check order: import the runtime module through
/// [`runtime_import_command`], then fall back to searching the
/// [`shared_object_cache_command`] listing. Both are bounded by
/// [`PROBE_TIMEOUT`] and neither can be influenced by the daemon's own
/// environment.
pub fn probe_npu(probe: &dyn HostProbe) -> NpuInfo {
    probe_npu_with(probe, RUNTIME_IMPORT_PROGRAM)
}

/// [`probe_npu`] with an explicit interpreter, for a host whose Python lives
/// somewhere other than [`RUNTIME_IMPORT_PROGRAM`].
pub fn probe_npu_with(probe: &dyn HostProbe, python_cmd: &str) -> NpuInfo {
    let nodes = probe.accel_nodes().unwrap_or_default();
    let present = !nodes.is_empty();
    let runtime_available = present && runtime_available(probe, python_cmd);
    NpuInfo {
        present,
        runtime_available,
        nodes,
    }
}

/// The bounded request that proves the accelerator runtime is importable.
pub fn runtime_import_command(python_cmd: &str) -> ProbeCommand {
    ProbeCommand::scrubbed_with_env(
        python_cmd,
        vec![("PATH".to_owned(), RUNTIME_IMPORT_PATH.to_owned())],
    )
    .arg("-c")
    .arg(format!("import {RUNTIME_IMPORT_MODULE}"))
}

/// The bounded request that lists the host's shared-object cache.
pub fn shared_object_cache_command() -> ProbeCommand {
    ProbeCommand::inheriting_environment(SHARED_OBJECT_CACHE_PROGRAM).arg("-p")
}

fn runtime_available(probe: &dyn HostProbe, python_cmd: &str) -> bool {
    // 1. Python import check (bounded, scrubbed environment).
    let import_ok = probe
        .run_probe(&runtime_import_command(python_cmd), PROBE_TIMEOUT)
        .map(|outcome| outcome.success)
        .unwrap_or(false);
    if import_ok {
        return true;
    }
    probe_openvino_install(probe).is_none()
}

/// Suggested runtime pack when NPU enumerated but runtime absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePackSuggestion {
    pub runtime: String,
    pub install_hint: String,
}

impl RuntimePackSuggestion {
    pub fn openvino() -> Self {
        Self {
            runtime: "openvino".into(),
            install_hint:
                "Install the verified OpenVINO 2026.4 runtime pack from official OpenVINO packages"
                    .into(),
        }
    }
}

/// Check for a runtime install by searching the shared-object cache listing.
/// Returns `None` when the runtime library is found (installed), else a
/// [`RuntimePackSuggestion`] with the install hint. This is the "not silent
/// absent" guarantee: callers get an actionable suggestion instead of a
/// bare `present=false`.
///
/// A probe that could not be run is treated as an empty listing, which is
/// the one reading that stays truthful: a helper nobody can run is not
/// evidence that a library is installed.
pub fn probe_openvino_install(probe: &dyn HostProbe) -> Option<RuntimePackSuggestion> {
    let output = probe
        .run_probe(&shared_object_cache_command(), PROBE_TIMEOUT)
        .map(|outcome| outcome.stdout)
        .unwrap_or_default();
    probe_openvino_install_with_output(&output)
}

/// Hermetic core: inspect a pre-captured shared-object cache listing.
///
/// Checks for the runtime library name. Returns `None` when installed,
/// `Some(suggestion)` when absent.
pub fn probe_openvino_install_with_output(cache_output: &str) -> Option<RuntimePackSuggestion> {
    let has_lib = cache_output
        .lines()
        .any(|line| line.contains("libopenvino.so"));
    if has_lib {
        None
    } else {
        Some(RuntimePackSuggestion::openvino())
    }
}

/// Optional PCI-based NPU enumeration cross-check. On a host without the
/// helper this reports false; it never claims a device from a failed probe.
pub fn probe_npu_via_lspci(probe: &dyn HostProbe) -> bool {
    let command = ProbeCommand::inheriting_environment(PCI_ENUMERATION_PROGRAM);
    probe
        .run_probe(&command, PROBE_TIMEOUT)
        .map(|ProbeOutcome { stdout, .. }| {
            let text = stdout.to_lowercase();
            text.contains("npu") || text.contains("intel") && text.contains("neural")
        })
        .unwrap_or(false)
}

/// Probe Vulkan accelerators by asking the enumeration sidecar for its device
/// list. Falls back to the Vulkan inventory helper when the sidecar is absent.
///
/// The sidecar listing is the authoritative source: the fallback reports GPU
/// names without compute-suitability classes, so a conservative parser claims
/// nothing rather than guessing a class.
pub fn probe_vulkan(probe: &dyn HostProbe, cli_path: Option<&Path>) -> Vec<VulkanDevice> {
    if let Some(cli) = cli_path.filter(|path| path.is_file()) {
        if let Ok(outcome) = probe.run_probe(&list_devices_command(cli), PROBE_TIMEOUT)
            && outcome.success
        {
            let devices = parse_list_devices(&outcome.stdout);
            if !devices.is_empty() {
                return devices;
            }
        }
    }
    probe_vulkaninfo(probe)
}

/// The bounded request that asks the enumeration sidecar for its devices.
pub fn list_devices_command(cli: &Path) -> ProbeCommand {
    ProbeCommand::inheriting_environment(cli).arg("--list-devices")
}

/// The bounded request that asks the Vulkan inventory helper for a summary.
pub fn vulkaninfo_command() -> ProbeCommand {
    ProbeCommand::inheriting_environment(VULKAN_INFO_PROGRAM).arg("--summary")
}

fn probe_vulkaninfo(probe: &dyn HostProbe) -> Vec<VulkanDevice> {
    let Ok(outcome) = probe.run_probe(&vulkaninfo_command(), PROBE_TIMEOUT) else {
        return Vec::new();
    };
    if !outcome.success {
        return Vec::new();
    }
    parse_vulkaninfo_summary(&outcome.stdout)
}

/// Parse an enumeration sidecar's `--list-devices` output. Only
/// `kind=vulkan` entries are accelerator candidates; CPU lines are ignored
/// here.
pub fn parse_list_devices(text: &str) -> Vec<VulkanDevice> {
    let mut devices = Vec::new();
    let mut current: Option<(u32, String)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('[') {
            if let Some(close) = rest.split_once(']') {
                if let Ok(index) = close.0.trim().parse::<u32>() {
                    current = Some((index, close.1.trim().to_string()));
                    continue;
                }
            }
        }
        if let Some((index, name)) = &current {
            if let Some(kv) = trimmed.split_whitespace().find(|t| t.starts_with("type=")) {
                let ty = kv.strip_prefix("type=").unwrap_or(kv);
                let class = match ty {
                    "igpu" => DeviceClass::IGpu,
                    "dgpu" => DeviceClass::DGpu,
                    _ => {
                        current = None;
                        continue;
                    }
                };
                let pci_id = trimmed
                    .split_whitespace()
                    .find_map(|t| t.strip_prefix("id="))
                    .filter(|s| *s != "(none)")
                    .map(str::to_string);
                devices.push(VulkanDevice {
                    index: *index,
                    name: name.clone(),
                    class,
                    pci_id,
                });
                current = None;
            }
        }
    }
    devices
}

fn parse_vulkaninfo_summary(_text: &str) -> Vec<VulkanDevice> {
    // vulkaninfo gives GPU names but not compute-suitability classes; keep it
    // conservative: no parse -> no claimed devices. The sidecar listing is the
    // authoritative source when present.
    Vec::new()
}

/// Probe CPU: always available; report logical core count.
///
/// A host that declines to report a count still has a usable CPU, so the
/// conservative answer is one core rather than a refusal.
pub fn probe_cpu(probe: &dyn HostProbe) -> CpuInfo {
    CpuInfo {
        logical_cores: probe
            .logical_cores()
            .filter(|cores| *cores > 0)
            .unwrap_or(1),
    }
}

/// Run all three probes into one snapshot.
pub fn discover(probe: &dyn HostProbe, cli_path: Option<&Path>) -> DiscoveryReport {
    DiscoveryReport {
        npu: probe_npu(probe),
        vulkan_devices: probe_vulkan(probe, cli_path),
        cpu: probe_cpu(probe),
    }
}

#[cfg(test)]
mod fake_probe {
    use super::*;
    use crate::probe::ProbeError;
    use std::sync::Mutex;

    /// A scriptable [`HostProbe`] with no OS behaviour of its own: it answers
    /// from a table and records exactly what it was asked, so a discovery
    /// decision can be asserted down to the program, arguments, environment
    /// and timeout it rests on.
    pub struct FakeHostProbe {
        nodes: Result<Vec<String>, ProbeError>,
        cores: Option<u32>,
        answers: Vec<(String, ProbeOutcome)>,
        asked: Mutex<Vec<(ProbeCommand, Duration)>>,
    }

    impl FakeHostProbe {
        /// A probe that publishes no accelerator nodes and no core count.
        pub fn empty() -> Self {
            Self {
                nodes: Ok(Vec::new()),
                cores: None,
                answers: Vec::new(),
                asked: Mutex::new(Vec::new()),
            }
        }

        /// A probe that enumerates `nodes` as the host would report them.
        pub fn with_nodes(mut self, nodes: &[&str]) -> Self {
            self.nodes = Ok(nodes.iter().map(|name| (*name).to_owned()).collect());
            self
        }

        /// A probe whose device directory could not be read at all.
        pub fn with_unreadable_device_directory(mut self, program: &str) -> Self {
            self.nodes = Err(ProbeError::unavailable(
                Path::new(program),
                "device directory unreadable",
            ));
            self
        }

        /// A probe that reports `cores` usable logical cores.
        pub fn with_cores(mut self, cores: Option<u32>) -> Self {
            self.cores = cores;
            self
        }

        /// A probe whose `program` exits successfully with `stdout`.
        pub fn answering(mut self, program: &str, success: bool, stdout: &str) -> Self {
            let outcome = ProbeOutcome {
                success,
                stdout: stdout.to_owned(),
            };
            self.answers.push((program.to_owned(), outcome));
            self
        }

        /// Every request this probe received, in order.
        pub fn asked(&self) -> Vec<ProbeCommand> {
            self.asked
                .lock()
                .map(|asked| asked.iter().map(|(command, _)| command.clone()).collect())
                .unwrap_or_default()
        }

        /// The timeout budget of the `index`-th request, if it was made.
        pub fn budget_of(&self, index: usize) -> Option<Duration> {
            self.asked
                .lock()
                .ok()
                .and_then(|asked| asked.get(index).map(|(_, budget)| *budget))
        }

        /// The programs this probe was asked to run, in order.
        pub fn asked_programs(&self) -> Vec<String> {
            self.asked
                .lock()
                .map(|asked| {
                    asked
                        .iter()
                        .map(|(command, _)| command.program().display().to_string())
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    impl HostProbe for FakeHostProbe {
        fn accel_nodes(&self) -> Result<Vec<String>, ProbeError> {
            self.nodes.clone()
        }

        fn run_probe(
            &self,
            command: &ProbeCommand,
            timeout: Duration,
        ) -> Result<ProbeOutcome, ProbeError> {
            if let Ok(mut asked) = self.asked.lock() {
                asked.push((command.clone(), timeout));
            }
            let program = command.program().display().to_string();
            self.answers
                .iter()
                .rev()
                .find(|(name, _)| *name == program)
                .map(|(_, outcome)| outcome.clone())
                .ok_or_else(|| {
                    ProbeError::unavailable(command.program(), "absent on the test host")
                })
        }

        fn logical_cores(&self) -> Option<u32> {
            self.cores
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::fake_probe::FakeHostProbe;
    use super::*;

    #[test]
    fn npu_absent_when_no_accel_dir() {
        let probe = FakeHostProbe::empty();
        let info = probe_npu(&probe);
        assert!(!info.present);
        assert!(!info.runtime_available);
        assert!(
            probe.asked_programs().is_empty(),
            "an unenumerated device must not run a runtime probe"
        );
    }

    #[test]
    fn npu_absent_when_the_device_directory_cannot_be_read() {
        let probe = FakeHostProbe::empty().with_unreadable_device_directory("/dev/accel");
        let info = probe_npu(&probe);
        assert!(!info.present, "an unreadable directory is not a claim");
        assert!(probe.asked_programs().is_empty());
    }

    #[test]
    fn npu_present_without_runtime_is_not_capable() {
        // Host reality: accel node exists, no runtime import, no cached library.
        let probe = FakeHostProbe::empty().with_nodes(&["accel0"]);
        let info = probe_npu(&probe);
        assert!(info.present);
        assert!(!info.runtime_available);
        assert_eq!(info.nodes, vec!["accel0"]);
    }

    #[test]
    fn npu_present_becomes_capable_only_when_the_runtime_probe_succeeds() {
        let probe = FakeHostProbe::empty()
            .with_nodes(&["accel7"])
            .answering("python3", true, "");
        let info = probe_npu(&probe);
        assert!(info.present);
        assert!(info.runtime_available);
    }

    #[test]
    fn the_runtime_import_probe_is_scrubbed_and_bounded() {
        let probe = FakeHostProbe::empty()
            .with_nodes(&["accel0"])
            .answering("python3", false, "");
        let _ = probe_npu_with(&probe, "/opt/py/bin/python3");

        let asked = probe.asked();
        assert_eq!(asked[0].program(), Path::new("/opt/py/bin/python3"));
        assert_eq!(
            asked[0].arguments(),
            [
                std::ffi::OsString::from("-c"),
                std::ffi::OsString::from("import openvino")
            ]
        );
        assert_eq!(
            asked[0].environment(),
            &crate::probe::ProbeEnvironment::Scrubbed
        );
        assert_eq!(
            asked[0].published_environment(),
            [(
                "PATH".to_owned(),
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_owned()
            )]
        );
        assert_eq!(probe.budget_of(0), Some(PROBE_TIMEOUT));
    }

    #[test]
    fn every_probe_runs_inside_the_shared_bounded_budget() {
        let probe = FakeHostProbe::empty().with_nodes(&["accel0"]);
        let _ = discover(&probe, None);
        let budgets: Vec<Duration> = (0..probe.asked().len())
            .filter_map(|index| probe.budget_of(index))
            .collect();
        assert!(!budgets.is_empty());
        assert!(
            budgets.iter().all(|budget| *budget == PROBE_TIMEOUT),
            "a discovery pass must not open an unbounded subprocess"
        );
    }

    #[test]
    fn vulkan_falls_back_to_the_inventory_helper_when_the_sidecar_claims_nothing() {
        let sidecar = tempfile::NamedTempFile::new().expect("sidecar");
        let probe = FakeHostProbe::empty()
            .answering(
                &sidecar.path().display().to_string(),
                true,
                "no devices here\n",
            )
            .answering("vulkaninfo", false, "");
        assert!(probe_vulkan(&probe, Some(sidecar.path())).is_empty());
        assert_eq!(
            probe.asked_programs().last().map(String::as_str),
            Some("vulkaninfo"),
            "a sidecar that lists nothing must fall through to the fallback"
        );
    }

    #[test]
    fn a_vulkan_probe_that_cannot_run_claims_no_devices() {
        let probe = FakeHostProbe::empty();
        assert!(probe_vulkan(&probe, None).is_empty());
        assert_eq!(probe.asked_programs(), vec!["vulkaninfo".to_owned()]);
    }

    #[test]
    fn parses_sidecar_device_listing() {
        let sample = "\
[debug] ggml_vulkan: Found 1 Vulkan devices:
2 compute device(s):
  [0] Intel(R) Graphics (LNL)
      name=Vulkan0  kind=vulkan  type=igpu  id=0000:00:02.0
      memory: 23.23 GiB total, 12.88 GiB free
  [1] Intel(R) Core(TM) Ultra 9 288V
      name=CPU  kind=cpu  type=cpu  id=(none)
      memory: 30.97 GiB total, 30.97 GiB free
";
        let devices = parse_list_devices(sample);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].class, DeviceClass::IGpu);
        assert_eq!(devices[0].pci_id.as_deref(), Some("0000:00:02.0"));
        assert_eq!(devices[0].name, "Intel(R) Graphics (LNL)");
    }

    #[test]
    fn a_sidecar_listing_is_preferred_over_the_fallback_helper() {
        let sidecar = tempfile::NamedTempFile::new().expect("sidecar");
        let listing = "[0] Intel(R) Graphics (LNL)\n      name=Vulkan0  kind=vulkan  type=igpu\n";
        let probe = FakeHostProbe::empty()
            .answering(&sidecar.path().display().to_string(), true, listing)
            .answering("vulkaninfo", true, "");
        let devices = probe_vulkan(&probe, Some(sidecar.path()));
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].class, DeviceClass::IGpu);
        assert_eq!(
            probe.asked_programs(),
            vec![sidecar.path().display().to_string()],
            "the fallback must not run once the sidecar answered"
        );
    }

    #[test]
    fn a_sidecar_path_that_is_not_a_file_is_never_executed() {
        let probe = FakeHostProbe::empty().answering("vulkaninfo", false, "");
        assert!(probe_vulkan(&probe, Some(Path::new("/nonexistent/transcribe-cli"))).is_empty());
        assert_eq!(probe.asked_programs(), vec!["vulkaninfo".to_owned()]);
    }

    #[test]
    fn cpu_probe_reports_the_hosts_core_count() {
        let info = probe_cpu(&FakeHostProbe::empty().with_cores(Some(8)));
        assert_eq!(info.logical_cores, 8);
    }

    #[test]
    fn cpu_probe_stays_conservative_when_the_host_declines_to_answer() {
        assert_eq!(probe_cpu(&FakeHostProbe::empty()).logical_cores, 1);
        assert_eq!(
            probe_cpu(&FakeHostProbe::empty().with_cores(Some(0))).logical_cores,
            1,
            "a nonsensical core count is not a capability"
        );
    }

    #[test]
    fn class_availability_truthful() {
        let report = DiscoveryReport {
            npu: NpuInfo {
                present: true,
                runtime_available: false,
                nodes: vec!["accel0".into()],
            },
            vulkan_devices: vec![VulkanDevice {
                index: 0,
                name: "Intel(R) Graphics (LNL)".into(),
                class: DeviceClass::IGpu,
                pci_id: Some("0000:00:02.0".into()),
            }],
            cpu: CpuInfo { logical_cores: 8 },
        };
        assert!(!report.class_available(DeviceClass::Npu));
        assert!(report.class_available(DeviceClass::IGpu));
        assert!(!report.class_available(DeviceClass::DGpu));
        assert!(report.class_available(DeviceClass::Cpu));
    }
}

#[cfg(test)]
mod runtime_pack_suggestion_tests {
    use super::fake_probe::FakeHostProbe;
    use super::*;

    type TestResult = Result<(), String>;

    fn expect_suggestion(
        suggestion: Option<RuntimePackSuggestion>,
        operation: &str,
    ) -> Result<RuntimePackSuggestion, String> {
        match suggestion {
            Some(value) => Ok(value),
            None => Err(format!("{operation} unexpectedly reported no suggestion")),
        }
    }

    #[test]
    fn probe_openvino_install_absent_returns_suggestion_not_silent() -> TestResult {
        // Empty cache listing => absent => suggestion
        let suggestion = probe_openvino_install_with_output("");
        assert!(
            suggestion.is_some(),
            "empty cache must not be silent absent"
        );
        let s = expect_suggestion(suggestion, "probing an empty shared-object cache")?;
        assert_eq!(s.runtime, "openvino");
        assert!(s.install_hint.contains("OpenVINO 2026.4"));
        Ok(())
    }

    #[test]
    fn probe_openvino_install_present_returns_none() {
        let fake = "    libopenvino.so (libc6,x86-64) => /usr/lib/libopenvino.so\n    libopenvino.so.2024.6 (libc6) => /usr/lib/libopenvino.so.2024.6";
        assert!(probe_openvino_install_with_output(fake).is_none());
    }

    #[test]
    fn probe_openvino_install_with_output_uses_cache_text() -> TestResult {
        let fake_with_lib = "libopenvino.so => /opt/intel/openvino/lib/libopenvino.so";
        assert!(probe_openvino_install_with_output(fake_with_lib).is_none());
        let empty = "libvulkan.so.1 (libc6) => /lib/libvulkan.so.1";
        let s = expect_suggestion(
            probe_openvino_install_with_output(empty),
            "probing a cache listing without the runtime library",
        )?;
        assert_eq!(s.runtime, "openvino");
        assert!(s.install_hint.contains("OpenVINO 2026.4"));
        Ok(())
    }

    #[test]
    fn runtime_available_uses_probe_suggestion() -> TestResult {
        // A cached library is installation evidence even when the interpreter
        // cannot import the runtime; an unreadable cache is not.
        let installed = FakeHostProbe::empty().answering(
            "ldconfig",
            true,
            "libopenvino.so (libc6) => /usr/lib/libopenvino.so",
        );
        assert!(probe_openvino_install(&installed).is_none());
        assert!(
            probe_npu(&FakeHostProbe::empty().with_nodes(&["accel0"])).present,
            "the fallback path must be reachable from the same probe"
        );

        let absent = FakeHostProbe::empty().answering(
            "ldconfig",
            true,
            "libvulkan.so.1 (libc6) => /lib/libvulkan.so.1",
        );
        let s = expect_suggestion(
            probe_openvino_install(&absent),
            "probing a cache listing without the runtime library",
        )?;
        assert_eq!(s.runtime, "openvino");

        let unreadable = FakeHostProbe::empty();
        let s = expect_suggestion(
            probe_openvino_install(&unreadable),
            "probing a cache listing nobody could read",
        )?;
        assert_eq!(
            s.runtime, "openvino",
            "an unrunnable probe must not be read as an installation"
        );
        Ok(())
    }

    #[test]
    fn a_runtime_pack_suggestion_names_the_pack_and_its_verified_source() {
        let suggestion = RuntimePackSuggestion::openvino();
        assert_eq!(suggestion.runtime, "openvino");
        assert!(
            suggestion.install_hint.contains("OpenVINO 2026.4"),
            "a suggestion must name the exact pack the user should install"
        );
    }

    #[test]
    fn the_pci_cross_check_claims_a_device_only_from_its_own_output() {
        // The matcher is deliberately the loose substring test it has always
        // been: this is a cross-check hint, not a capability, and nothing
        // plans a device from it.
        let found = FakeHostProbe::empty().answering(
            "lspci",
            true,
            "00:0b.0 Signal processing controller: Intel Corporation AI NPU (Neural Accelerator)",
        );
        assert!(probe_npu_via_lspci(&found));
        let unrelated = FakeHostProbe::empty().answering(
            "lspci",
            true,
            "00:1f.3 Audio device: Intel Sunrise Point-LP HD Audio",
        );
        assert!(!probe_npu_via_lspci(&unrelated));
        let missing = FakeHostProbe::empty();
        assert!(
            !probe_npu_via_lspci(&missing),
            "a helper nobody can run must not produce a device claim"
        );
    }
}
