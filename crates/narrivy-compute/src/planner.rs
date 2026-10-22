//! Policy-driven compute device selection.
//!
//! Invariants (PORT-006):
//! - Hierarchy NPU -> dGPU -> iGPU -> CPU, but only among devices that are
//!   BOTH enumerated-and-runtime-available AND covered by a PASSING
//!   qualification tuple. An unqualified or runtime-less accelerator is never
//!   selected, even when enumerated.
//! - `Manual::Locked(device)` never silently falls back: it either selects the
//!   locked device or fails with a typed error.

use crate::discovery::{DeviceClass, DiscoveryReport};
use crate::qualify::QualificationRegistry;
use serde::{Deserialize, Serialize};

/// Selection policy surface exposed to settings/UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Policy {
    /// Class hierarchy, best measured RTF within the preferred class.
    Balanced,
    /// Prefer the class+tuple with the strongest accuracy evidence.
    BestAccuracy,
    /// Best RTF among qualified candidates regardless of class preference,
    /// then class hierarchy as tiebreak.
    LowestLatency,
    /// Strictest class hierarchy first (NPU lowest power), best RTF inside.
    LowestPower,
    /// Manual override. `Preferred` falls back on unavailability;
    /// `Locked` fails loudly instead.
    Manual(ManualChoice),
}

/// A concrete selectable device reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceRef {
    pub class: DeviceClass,
    /// Backend id, e.g. "vulkan", "cpu", "openvino".
    pub backend: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualChoice {
    Preferred(DeviceRef),
    Locked(DeviceRef),
}

/// Typed planner failure modes.
#[derive(Debug, thiserror::Error)]
pub enum ComputeError {
    /// A LOCKED device was unavailable/unqualified. Never silently degraded.
    #[error(
        "locked device {class:?}/{backend} unavailable or unqualified; refusing silent fallback"
    )]
    LockedDeviceUnavailable { class: DeviceClass, backend: String },
    #[error("no qualified and available device for any class in the hierarchy")]
    NoQualifiedDevice,
    #[error("requested combination unsupported: {0}")]
    UnsupportedCombo(String),
    #[error("sidecar spawn failed: {0}")]
    SidecarSpawn(String),
    #[error("sidecar exceeded timeout of {0}")]
    SidecarTimeout(String),
}

/// Result of a selection, with the full human-readable reason chain
/// (rendered in UI as e.g. "Auto -> ...").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedDecision {
    pub device: DeviceRef,
    pub reason_chain: Vec<String>,
    /// Estimated memory in MB for this device (stub: 0 for CPU, high for NPU).
    #[serde(default)]
    pub estimated_memory_mb: u32,
    /// Estimated battery impact (stub: "low" for CPU, "high" for NPU).
    #[serde(default = "default_battery_impact")]
    pub estimated_battery_impact: String,
}

fn default_battery_impact() -> String {
    "low".to_string()
}

impl ResolvedDecision {
    pub fn summary(&self) -> String {
        format!(
            "Auto -> {} ({})",
            backend_label(self.device.class),
            self.device.backend
        )
    }

    /// Human-readable explanation used by UI's `Auto → ...` label.
    /// Includes reason chain, estimated memory and battery impact.
    pub fn explain(&self) -> String {
        let base = self.summary();
        let mem = format!("{} MB", self.estimated_memory_mb);
        let batt = &self.estimated_battery_impact;
        if self.reason_chain.is_empty() {
            format!("{base} — {mem}, battery: {batt}")
        } else {
            format!(
                "{} — {} — {} MB, battery: {}",
                base,
                self.reason_chain.join(" | "),
                self.estimated_memory_mb,
                batt
            )
        }
    }
}

/// Planner facade for UI integration (P5 disruptor market-leading compute).
pub struct Planner;

impl Planner {
    /// Returns human-readable reason chain explanation for a decision.
    pub fn explain(decision: &ResolvedDecision) -> String {
        decision.explain()
    }
}

pub fn estimated_memory_mb(class: DeviceClass) -> u32 {
    match class {
        DeviceClass::Npu => 1500,
        DeviceClass::DGpu => 1200,
        DeviceClass::IGpu => 800,
        DeviceClass::Cpu => 0,
    }
}

pub fn estimated_battery_impact(class: DeviceClass) -> String {
    match class {
        DeviceClass::Npu => "high".to_string(),
        DeviceClass::DGpu => "high".to_string(),
        DeviceClass::IGpu => "medium".to_string(),
        DeviceClass::Cpu => "low".to_string(),
    }
}

fn backend_label(class: DeviceClass) -> String {
    match class {
        DeviceClass::Npu => "NPU".into(),
        DeviceClass::DGpu => "dGPU".into(),
        DeviceClass::IGpu => "iGPU".into(),
        DeviceClass::Cpu => "CPU".into(),
    }
}

/// Candidate = available per discovery AND passing tuple exists.
fn candidate_tuples<'r>(
    registry: &'r QualificationRegistry,
    discovery: &DiscoveryReport,
    class: DeviceClass,
) -> Vec<&'r crate::qualify::QualificationTuple> {
    if !discovery.class_available(class) {
        return Vec::new();
    }
    registry.passing_for_class(class, None)
}

fn rtf(tuple: &crate::qualify::QualificationTuple) -> f64 {
    tuple.metrics.rtf.unwrap_or(0.0)
}

/// Core selection entry point.
pub fn select(
    policy: &Policy,
    registry: &QualificationRegistry,
    discovery: &DiscoveryReport,
) -> Result<ResolvedDecision, ComputeError> {
    match policy {
        Policy::Manual(ManualChoice::Preferred(want)) => {
            let mut chain = vec![format!(
                "manual preferred {:?}/{}, checking availability",
                want.class, want.backend
            )];
            let mut classes = hierarchy();
            classes.sort_by_key(|c| c.rank());
            // Preferred device first, then normal hierarchy fallback.
            if let Some(tuples) = try_class(
                registry,
                discovery,
                want.class,
                Some(&want.backend),
                &mut chain,
            ) {
                if pick_best(&tuples, policy).is_some() {
                    chain.push(format!(
                        "selected preferred {}/{}",
                        backend_label(want.class),
                        want.backend
                    ));
                    return Ok(finish(want.clone(), chain));
                }
            }
            for class in classes {
                if let Some(tuples) = try_class(registry, discovery, class, None, &mut chain) {
                    if let Some(t) = pick_best(&tuples, policy) {
                        chain.push(format!(
                            "preferred unavailable; fell back to {}",
                            backend_label(t.device_class)
                        ));
                        return Ok(finish(
                            DeviceRef {
                                class: t.device_class,
                                backend: t.runtime_backend.clone(),
                            },
                            chain,
                        ));
                    }
                }
            }
            Err(ComputeError::NoQualifiedDevice)
        }
        Policy::Manual(ManualChoice::Locked(want)) => {
            let mut chain = vec![format!("locked to {:?}", want.class)];
            if !discovery.class_available(want.class) {
                return Err(ComputeError::LockedDeviceUnavailable {
                    class: want.class,
                    backend: want.backend.clone(),
                });
            }
            let tuples = try_class(
                registry,
                discovery,
                want.class,
                Some(&want.backend),
                &mut chain,
            )
            .ok_or_else(|| ComputeError::LockedDeviceUnavailable {
                class: want.class,
                backend: want.backend.clone(),
            })?;
            let t = tuples
                .first()
                .ok_or_else(|| ComputeError::LockedDeviceUnavailable {
                    class: want.class,
                    backend: want.backend.clone(),
                })?;
            chain.push(format!(
                "locked device qualified (rtf={:.2}); no fallback permitted",
                rtf(t)
            ));
            Ok(finish(want.clone(), chain))
        }
        policy @ (Policy::Balanced
        | Policy::BestAccuracy
        | Policy::LowestLatency
        | Policy::LowestPower) => {
            let mut chain: Vec<String> = Vec::new();
            if *policy == Policy::LowestLatency {
                let classes = hierarchy();
                // Global best-RTF scan across all classes, tiebroken by rank.
                let mut all: Vec<&crate::qualify::QualificationTuple> = Vec::new();
                for c in classes.iter().copied() {
                    all.extend(candidate_tuples(registry, discovery, c));
                }
                let best = all.into_iter().max_by(|a, b| {
                    rtf(a)
                        .partial_cmp(&rtf(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(b.device_class.rank().cmp(&a.device_class.rank()))
                });
                return match best {
                    Some(t) => {
                        let dev = DeviceRef {
                            class: t.device_class,
                            backend: t.runtime_backend.clone(),
                        };
                        Ok(finish(
                            dev,
                            vec![format!("LowestLatency: best qualified rtf={:.2}", rtf(t))],
                        ))
                    }
                    None => Err(ComputeError::NoQualifiedDevice),
                };
            }
            let mut classes = hierarchy();
            classes.sort_by_key(|c| c.rank());
            for class in classes {
                if let Some(tuples) = try_class(registry, discovery, class, None, &mut chain) {
                    if let Some(t) = pick_best(&tuples, policy) {
                        chain.push(format!(
                            "selected {} ({}) via qualified tuple",
                            backend_label(t.device_class),
                            t.runtime_backend
                        ));
                        return Ok(finish(
                            DeviceRef {
                                class: t.device_class,
                                backend: t.runtime_backend.clone(),
                            },
                            chain,
                        ));
                    }
                }
            }
            Err(ComputeError::NoQualifiedDevice)
        }
    }
}

fn hierarchy() -> [DeviceClass; 4] {
    [
        DeviceClass::Npu,
        DeviceClass::DGpu,
        DeviceClass::IGpu,
        DeviceClass::Cpu,
    ]
}

/// Returns passing tuples when the class is usable; records skip reasons.
fn try_class<'r>(
    registry: &'r QualificationRegistry,
    discovery: &DiscoveryReport,
    class: DeviceClass,
    backend: Option<&str>,
    chain: &mut Vec<String>,
) -> Option<Vec<&'r crate::qualify::QualificationTuple>> {
    if !discovery.class_available(class) {
        chain.push(match class {
            DeviceClass::Npu => format!(
                "NPU enumerated={} runtime_available={} -> skipped (enumeration != capability)",
                discovery.npu.present, discovery.npu.runtime_available
            ),
            other => format!("{:?} not available on this host -> skipped", other),
        });
        return None;
    }
    let tuples = registry.passing_for_class(class, backend);
    if tuples.is_empty() {
        chain.push(format!(
            "{:?} available but no PASSING qualification tuple -> skipped",
            class
        ));
        return None;
    }
    Some(tuples)
}

fn pick_best<'t>(
    tuples: &[&'t crate::qualify::QualificationTuple],
    policy: &Policy,
) -> Option<&'t crate::qualify::QualificationTuple> {
    tuples.iter().copied().max_by(|a, b| match policy {
        Policy::BestAccuracy => accuracy_score(a).total_cmp(&accuracy_score(b)),
        _ => rtf(a)
            .partial_cmp(&rtf(b))
            .unwrap_or(std::cmp::Ordering::Equal),
    })
}

fn accuracy_score(t: &crate::qualify::QualificationTuple) -> f64 {
    let base = f64::from(u8::from(t.metrics.accuracy_fragment_match.unwrap_or(false)));
    // Tiebreak equal accuracy by RTF so faster decode wins among equals.
    base + rtf(t) / 1000.0
}

fn finish(device: DeviceRef, mut reason_chain: Vec<String>) -> ResolvedDecision {
    reason_chain.insert(
        0,
        "Auto: hierarchy NPU->dGPU->iGPU->CPU filtered by availability and passing qualifications"
            .to_string(),
    );
    let class = device.class;
    ResolvedDecision {
        device,
        reason_chain,
        estimated_memory_mb: estimated_memory_mb(class),
        estimated_battery_impact: estimated_battery_impact(class),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{CpuInfo, NpuInfo, VulkanDevice};
    use crate::qualify::{QualificationMetrics, QualificationTuple};
    use DeviceClass::{Cpu as DCpu, DGpu, IGpu, Npu};

    type TestResult = Result<(), String>;

    fn test_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, String> {
        result.map_err(|error| format!("unexpected test failure: {error:?}"))
    }

    fn expect_compute_error<T>(
        result: Result<T, ComputeError>,
        operation: &str,
    ) -> Result<ComputeError, String> {
        match result {
            Err(error) => Ok(error),
            Ok(_) => Err(format!("{operation} unexpectedly succeeded")),
        }
    }

    fn npu_enumerated_no_runtime() -> DiscoveryReport {
        DiscoveryReport {
            npu: NpuInfo {
                present: true,
                runtime_available: false,
                nodes: vec!["accel0".into()],
            },
            vulkan_devices: vec![VulkanDevice {
                index: 0,
                name: "iGPU".into(),
                class: IGpu,
                pci_id: None,
            }],
            cpu: CpuInfo { logical_cores: 8 },
        }
    }

    fn tuple(class: DeviceClass, backend: &str, rtf: f64, pass: bool) -> QualificationTuple {
        QualificationTuple {
            engine: "transcribe-gguf".into(),
            runtime_backend: backend.into(),
            device_class: class,
            model_id: "whisper-tiny.en".into(),
            quant: "Q8_0".into(),
            os: "linux-x86_64".into(),
            narrator_version: "0.0.0".into(),
            pass,
            metrics: QualificationMetrics {
                rtf: Some(rtf),
                ..Default::default()
            },
        }
    }

    fn reg_with(tuples: Vec<QualificationTuple>) -> QualificationRegistry {
        QualificationRegistry {
            schema_version: 1,
            tuples,
        }
    }

    #[test]
    fn npu_enumerated_but_runtime_absent_falls_through() -> TestResult {
        let discovery = npu_enumerated_no_runtime();
        let registry = reg_with(vec![
            tuple(Npu, "openvino", 10.0, true), // qualified on paper...
            tuple(IGpu, "vulkan", 4.0, true),
            tuple(DCpu, "cpu", 1.0, true),
        ]);
        // ...but NPU runtime absent -> must NOT be selected.
        let decision = test_ok(select(&Policy::Balanced, &registry, &discovery))?;
        assert_eq!(
            decision.device,
            DeviceRef {
                class: IGpu,
                backend: "vulkan".into()
            }
        );
        assert!(
            decision
                .reason_chain
                .iter()
                .any(|r| r.contains("enumeration != capability"))
        );
        Ok(())
    }

    #[test]
    fn npu_qualified_on_paper_but_unqualified_tuple_never_selected() -> TestResult {
        let discovery = npu_enumerated_no_runtime();
        let registry = reg_with(vec![
            tuple(Npu, "openvino", 10.0, false), // FAILING tuple
            tuple(DCpu, "cpu", 1.0, true),
        ]);
        let decision = test_ok(select(&Policy::Balanced, &registry, &discovery))?;
        assert_eq!(decision.device.class, DCpu);
        Ok(())
    }

    #[test]
    fn balanced_picks_best_rtf_within_preferred_class() -> TestResult {
        let discovery = npu_enumerated_no_runtime();
        let registry = reg_with(vec![
            tuple(IGpu, "vulkan", 3.5, true),
            tuple(IGpu, "vulkan-slow", 2.0, true),
            tuple(DCpu, "cpu", 1.0, true),
        ]);
        let decision = test_ok(select(&Policy::Balanced, &registry, &discovery))?;
        assert_eq!(decision.device.backend, "vulkan");
        Ok(())
    }

    #[test]
    fn lowest_latency_picks_global_best_rtf() -> TestResult {
        let discovery = npu_enumerated_no_runtime();
        let registry = reg_with(vec![
            tuple(IGpu, "vulkan", 3.5, true),
            tuple(DCpu, "cpu", 6.0, true), // faster despite lower class
        ]);
        let decision = test_ok(select(&Policy::LowestLatency, &registry, &discovery))?;
        assert_eq!(decision.device.class, DCpu);
        Ok(())
    }

    #[test]
    fn locked_unavailable_is_typed_failure_not_silent_fallback() -> TestResult {
        let discovery = npu_enumerated_no_runtime(); // no dGPU at all
        let registry = reg_with(vec![tuple(DCpu, "cpu", 1.0, true)]);
        let err = expect_compute_error(
            select(
                &Policy::Manual(ManualChoice::Locked(DeviceRef {
                    class: DGpu,
                    backend: "cuda".into(),
                })),
                &registry,
                &discovery,
            ),
            "selecting an unavailable locked device",
        )?;
        assert!(matches!(err, ComputeError::LockedDeviceUnavailable { .. }));
        Ok(())
    }

    #[test]
    fn locked_available_but_unqualified_is_typed_failure() -> TestResult {
        let discovery = npu_enumerated_no_runtime();
        let registry = reg_with(vec![
            tuple(IGpu, "vulkan", 4.0, false), // failing tuple only
            tuple(DCpu, "cpu", 1.0, true),
        ]);
        let err = expect_compute_error(
            select(
                &Policy::Manual(ManualChoice::Locked(DeviceRef {
                    class: IGpu,
                    backend: "vulkan".into(),
                })),
                &registry,
                &discovery,
            ),
            "selecting an unqualified locked device",
        )?;
        assert!(matches!(err, ComputeError::LockedDeviceUnavailable { .. }));
        Ok(())
    }

    #[test]
    fn locked_satisfied_selects_without_fallback() -> TestResult {
        let discovery = npu_enumerated_no_runtime();
        let registry = reg_with(vec![tuple(IGpu, "vulkan", 4.0, true)]);
        let decision = test_ok(select(
            &Policy::Manual(ManualChoice::Locked(DeviceRef {
                class: IGpu,
                backend: "vulkan".into(),
            })),
            &registry,
            &discovery,
        ))?;
        assert_eq!(decision.device.backend, "vulkan");
        assert!(decision.summary().starts_with("Auto -> "));
        Ok(())
    }

    #[test]
    fn preferred_unavailable_falls_back_but_locked_does_not() -> TestResult {
        let discovery = npu_enumerated_no_runtime();
        let registry = reg_with(vec![tuple(DCpu, "cpu", 1.0, true)]);
        let preferred = test_ok(select(
            &Policy::Manual(ManualChoice::Preferred(DeviceRef {
                class: Npu,
                backend: "openvino".into(),
            })),
            &registry,
            &discovery,
        ))?;
        assert_eq!(preferred.device.class, DCpu);
        let locked = select(
            &Policy::Manual(ManualChoice::Locked(DeviceRef {
                class: Npu,
                backend: "openvino".into(),
            })),
            &registry,
            &discovery,
        );
        assert!(matches!(
            locked,
            Err(ComputeError::LockedDeviceUnavailable { .. })
        ));
        Ok(())
    }

    #[test]
    fn no_candidates_at_all_is_typed_error() -> TestResult {
        let discovery = npu_enumerated_no_runtime();
        let registry = reg_with(vec![]);
        let err = expect_compute_error(
            select(&Policy::Balanced, &registry, &discovery),
            "selecting from an empty qualification registry",
        )?;
        assert!(matches!(err, ComputeError::NoQualifiedDevice));
        Ok(())
    }
}

#[cfg(test)]
mod planner_disruptor_tests {
    use super::*;
    use crate::discovery::{CpuInfo, NpuInfo};
    use crate::qualify::{QualificationMetrics, QualificationTuple};

    type TestResult = Result<(), String>;

    fn test_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, String> {
        result.map_err(|error| format!("unexpected test failure: {error:?}"))
    }

    fn discovery_with_cpu() -> crate::discovery::DiscoveryReport {
        crate::discovery::DiscoveryReport {
            npu: NpuInfo {
                present: false,
                runtime_available: false,
                nodes: vec![],
            },
            vulkan_devices: vec![],
            cpu: CpuInfo { logical_cores: 4 },
        }
    }

    fn tuple_cpu() -> QualificationTuple {
        QualificationTuple {
            engine: "transcribe-gguf".into(),
            runtime_backend: "cpu".into(),
            device_class: DeviceClass::Cpu,
            model_id: "whisper-tiny.en".into(),
            quant: "Q8_0".into(),
            os: "linux-x86_64".into(),
            narrator_version: "0.0.0".into(),
            pass: true,
            metrics: QualificationMetrics {
                rtf: Some(2.0),
                ..Default::default()
            },
        }
    }

    #[test]
    fn explain_contains_auto_and_memory_and_battery() -> TestResult {
        let discovery = discovery_with_cpu();
        let registry = crate::qualify::QualificationRegistry {
            schema_version: 1,
            tuples: vec![tuple_cpu()],
        };
        let decision = test_ok(select(&Policy::Balanced, &registry, &discovery))?;
        let explain = decision.explain();
        assert!(explain.starts_with("Auto ->"), "explain={explain}");
        assert!(
            explain.contains("0 MB"),
            "explain should contain memory 0 for CPU, got {explain}"
        );
        assert!(
            explain.contains("battery: low"),
            "battery low for CPU, got {explain}"
        );
        // Planner facade same
        let via_planner = Planner::explain(&decision);
        assert_eq!(explain, via_planner);
        Ok(())
    }

    #[test]
    fn explain_for_npu_has_high_battery_and_memory() -> TestResult {
        let npu_disc = crate::discovery::DiscoveryReport {
            npu: NpuInfo {
                present: true,
                runtime_available: true,
                nodes: vec!["accel0".into()],
            },
            vulkan_devices: vec![],
            cpu: CpuInfo { logical_cores: 4 },
        };
        let npu_tuple = QualificationTuple {
            device_class: DeviceClass::Npu,
            runtime_backend: "openvino".into(),
            ..tuple_cpu()
        };
        let reg = crate::qualify::QualificationRegistry {
            schema_version: 1,
            tuples: vec![npu_tuple, tuple_cpu()],
        };
        let decision = test_ok(select(&Policy::Balanced, &reg, &npu_disc))?;
        assert_eq!(decision.device.class, DeviceClass::Npu);
        assert_eq!(decision.estimated_memory_mb, 1500);
        assert_eq!(decision.estimated_battery_impact, "high");
        let explain = decision.explain();
        assert!(explain.contains("1500 MB"));
        assert!(explain.contains("high"));
        Ok(())
    }

    #[test]
    fn frontend_can_render_explain_without_mock() -> TestResult {
        // Simulates P3 frontend's Auto → rendering logic without mocking.
        let discovery = discovery_with_cpu();
        let registry = crate::qualify::QualificationRegistry {
            schema_version: 1,
            tuples: vec![tuple_cpu()],
        };
        let decision = test_ok(select(&Policy::Balanced, &registry, &discovery))?;
        // Frontend does: `Auto → ${value}` compose (see resolved.ts). Our explain already starts with Auto →
        // Verify it is directly usable as a label (non-empty, human readable, contains device).
        let label = decision.explain();
        assert!(label.contains("CPU"), "frontend label should mention CPU");
        // Ensure JSON serialization round-trips for bridge transport
        let json = test_ok(serde_json::to_string(&decision))?;
        let back: ResolvedDecision = test_ok(serde_json::from_str(&json))?;
        assert_eq!(back.explain(), label);
        Ok(())
    }

    #[test]
    fn estimated_helpers_stub_values() {
        assert_eq!(estimated_memory_mb(DeviceClass::Cpu), 0);
        assert_eq!(estimated_memory_mb(DeviceClass::Npu), 1500);
        assert_eq!(estimated_battery_impact(DeviceClass::Cpu), "low");
        assert_eq!(estimated_battery_impact(DeviceClass::Npu), "high");
    }
}
