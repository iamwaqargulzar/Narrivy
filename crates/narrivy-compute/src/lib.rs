//! P5 compute planning primitives for Narrivy.
//!
//! This crate owns the compute-capability surface:
//!
//! - [`probe`]: the [`HostProbe`](probe::HostProbe) contract through which
//!   every fact about the host arrives. The crate itself contains no process
//!   spawning, no environment handling and no device probing, so it builds and
//!   tests on every target the workspace supports.
//! - [`discovery`]: truthful hardware/runtime enumeration (accelerator nodes,
//!   runtime presence, Vulkan devices via an enumeration sidecar or a Vulkan
//!   inventory helper, CPU). Enumeration is NOT capability: a device counts as
//!   usable only when its runtime stack is also present.
//! - [`qualify`]: real-decode qualification tuples persisted to a versioned
//!   `qualifications.json`. Only measured results are recorded.
//! - [`planner`]: policy-driven device selection over qualified tuples.
//!   Unqualified accelerators are never selected even when enumerated.
//! - [`language`]: per-model language/biasing capability truth.
//! - [`streaming`]: streaming-support honesty per engine.
//!
//! Every probe is a bounded-timeout request value; all errors are typed.

pub mod discovery;
pub mod language;
pub mod planner;
pub mod probe;
pub mod qualify;
pub mod streaming;

pub use discovery::{
    CpuInfo, DeviceClass, DiscoveryReport, NpuInfo, RuntimePackSuggestion, VulkanDevice,
};
pub use language::{LanguageCapabilities, LanguageRegistry};
pub use planner::{
    ComputeError, DeviceRef, ManualChoice, Planner, Policy, ResolvedDecision,
    estimated_battery_impact, estimated_memory_mb, select,
};
pub use probe::{HostProbe, ProbeCommand, ProbeEnvironment, ProbeError, ProbeOutcome};
pub use qualify::{QualificationMetrics, QualificationRegistry, QualificationTuple};
pub use streaming::{StreamingSupport, StreamingSupportRegistry};

/// Crate-local error type shared by registry IO helpers.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error in {path}: {source}")]
    Json {
        path: String,
        source: serde_json::Error,
    },
}
