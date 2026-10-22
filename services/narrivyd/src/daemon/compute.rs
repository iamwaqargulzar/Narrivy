//! Compute planning composition.
//!
//! `narrivy-compute` owns the decision of which device may be planned onto;
//! this module owns the decision of when to ask, what to do with a refusal,
//! and how the answer is rendered. It composes the Linux host probe with the
//! planner and never inspects a device, spawns a helper, or names an operating
//! system: every host fact and every device class comes from
//! `narrivy-compute` or from the platform adapter.
//!
//! Discovery runs once at startup and is immutable afterwards. A compute plan
//! is a claim about this host at this moment; re-deriving it per session would
//! make a capability token depend on when a client happened to ask.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use narrivy_compute::discovery::{DeviceClass, DiscoveryReport, discover, probe_openvino_install};
use narrivy_compute::planner::{ComputeError, ManualChoice, Policy, ResolvedDecision, select};
use narrivy_compute::qualify::QualificationRegistry;
use narrivy_platform_linux::compute_probe::LinuxHostProbe;

/// Environment hook for the selection policy (`NARRIVY_COMPUTE_POLICY`).
pub const COMPUTE_POLICY_ENV: &str = "NARRIVY_COMPUTE_POLICY";

/// Versioned qualification registry, relative to the daemon state directory.
pub const QUALIFICATIONS_FILE: &str = "qualifications.json";

/// Advertised when a decision was reached.
pub const DEVICE_RESOLVED_CAPABILITY: &str = "compute.device=resolved";

/// Advertised when the daemon could not build a plan at all. A missing
/// registry, a refused probe and an unreadable state directory all land here,
/// because none of them is evidence about the host's hardware.
pub const DEVICE_UNAVAILABLE_CAPABILITY: &str = "compute.device=unavailable:no_qualified_plan";

/// Runtime pack the user should install, when an accelerator is enumerated but
/// its runtime is not. Content-free: it is a pack identity, never a device.
pub const RUNTIME_PACK_CAPABILITY_PREFIX: &str = "compute.runtime-pack=";

/// One immutable compute plan, or the typed reason there is none.
pub struct ComputePlan {
    report: DiscoveryReport,
    decision: Option<ResolvedDecision>,
    /// The runtime pack to install, if a pack would unblock an enumerated
    /// accelerator. Absent when the host enumerated no accelerator at all.
    runtime_pack: Option<String>,
}

impl ComputePlan {
    /// The device the planner selected, if it selected one.
    pub fn decision(&self) -> Option<&ResolvedDecision> {
        self.decision.as_ref()
    }

    /// The human-readable reason chain, which is also what a client renders.
    pub fn reason_chain(&self) -> &[String] {
        self.decision
            .as_ref()
            .map(|decision| decision.reason_chain.as_slice())
            .unwrap_or_default()
    }

    /// Content-free capability tokens. The device *class* and the backend id
    /// come from the planner; no operating system, vendor, accelerator or model
    /// name is rendered, because a client may not be told a claim the daemon
    /// did not make.
    pub fn append_capabilities(&self, capabilities: &mut Vec<String>) {
        match &self.decision {
            Some(decision) => {
                capabilities.push(DEVICE_RESOLVED_CAPABILITY.to_owned());
                capabilities.push(format!(
                    "compute.device.class={}",
                    class_token(decision.device.class)
                ));
                capabilities.push(format!(
                    "compute.device.backend={}",
                    decision.device.backend
                ));
                if let Some(pack) = &self.runtime_pack {
                    capabilities.push(format!("{RUNTIME_PACK_CAPABILITY_PREFIX}{pack}"));
                }
            }
            None => capabilities.push(DEVICE_UNAVAILABLE_CAPABILITY.to_owned()),
        }
    }
}

fn class_token(class: DeviceClass) -> &'static str {
    // The same snake_case tokens `narrivy-compute` serializes, chosen here
    // once so the daemon never re-invents a device vocabulary.
    match class {
        DeviceClass::Npu => "npu",
        DeviceClass::DGpu => "dgpu",
        DeviceClass::IGpu => "igpu",
        DeviceClass::Cpu => "cpu",
    }
}

/// Read the selection policy from the environment.
///
/// An unparsable value is a refusal, not a silent fallback to `Balanced`: a
/// user who asked for a locked device must never be handed a different one
/// because the daemon could not read their choice.
pub fn policy_from_env() -> Result<Policy, String> {
    let Ok(value) = std::env::var(COMPUTE_POLICY_ENV) else {
        return Ok(Policy::Balanced);
    };
    parse_policy(&value)
}

/// `balanced` | `best_accuracy` | `lowest_latency` | `lowest_power` |
/// `preferred:<class>:<backend>` | `locked:<class>:<backend>`.
pub fn parse_policy(value: &str) -> Result<Policy, String> {
    let value = value.trim();
    match value {
        "balanced" => return Ok(Policy::Balanced),
        "best_accuracy" => return Ok(Policy::BestAccuracy),
        "lowest_latency" => return Ok(Policy::LowestLatency),
        "lowest_power" => return Ok(Policy::LowestPower),
        _ => {}
    }
    let (kind, rest) = value
        .split_once(':')
        .ok_or_else(|| format!("unparsable {COMPUTE_POLICY_ENV}='{value}'"))?;
    let (class, backend) = rest
        .split_once(':')
        .ok_or_else(|| format!("{COMPUTE_POLICY_ENV}='{value}' needs <class>:<backend>"))?;
    let device = narrivy_compute::planner::DeviceRef {
        class: parse_class(class)?,
        backend: backend.to_owned(),
    };
    match kind {
        "preferred" => Ok(Policy::Manual(ManualChoice::Preferred(device))),
        "locked" => Ok(Policy::Manual(ManualChoice::Locked(device))),
        other => Err(format!("unknown {COMPUTE_POLICY_ENV} mode '{other}'")),
    }
}

fn parse_class(value: &str) -> Result<DeviceClass, String> {
    match value {
        "npu" => Ok(DeviceClass::Npu),
        "dgpu" => Ok(DeviceClass::DGpu),
        "igpu" => Ok(DeviceClass::IGpu),
        "cpu" => Ok(DeviceClass::Cpu),
        other => Err(format!("unknown device class '{other}'")),
    }
}

/// Build the plan once, at startup.
///
/// A missing or unreadable qualification registry yields a plan with no
/// decision rather than a failure: no measured tuple means no accelerator may be
/// selected, and that is a capability fact, not a daemon fault.
pub fn build_plan(state_dir: &Path, enumeration_sidecar: Option<&Path>) -> ComputePlan {
    let probe = LinuxHostProbe::new();
    let report = discover(&probe, enumeration_sidecar);
    let runtime_pack = report
        .npu
        .present
        .then(|| probe_openvino_install(&probe))
        .flatten()
        .map(|suggestion| suggestion.runtime);

    let registry_path = state_dir.join(QUALIFICATIONS_FILE);
    let decision = match policy_from_env() {
        Ok(policy) => match QualificationRegistry::load(&registry_path) {
            Ok(registry) => match select(&policy, &registry, &report) {
                Ok(decision) => Some(decision),
                Err(ComputeError::LockedDeviceUnavailable { class, backend }) => {
                    // A locked device the host cannot honestly provide is a
                    // refusal the user must see, not a silent downgrade.
                    eprintln!(
                        "narrivyd: locked compute device {class:?}/{backend} is unavailable \
                         or unqualified; refusing a silent fallback"
                    );
                    None
                }
                Err(error) => {
                    eprintln!("narrivyd: no compute plan: {error}");
                    None
                }
            },
            Err(reason) => {
                eprintln!(
                    "narrivyd: qualification registry unreadable at {}: {reason}; \
                     no accelerator may be selected",
                    registry_path.display()
                );
                None
            }
        },
        Err(reason) => {
            eprintln!("narrivyd: {reason}");
            None
        }
    };

    ComputePlan {
        report,
        decision,
        runtime_pack,
    }
}

/// The plan's own snapshot, for a client that renders the full report.
pub fn report_of(plan: &ComputePlan) -> &DiscoveryReport {
    &plan.report
}

/// Convenience for a caller that already resolved the state directory.
pub fn build_plan_for(state_dir: PathBuf, sidecar: Option<PathBuf>) -> ComputePlan {
    build_plan(&state_dir, sidecar.as_deref())
}

/// Shared handle for the daemon's capability builder.
pub type SharedPlan = Arc<ComputePlan>;
