//! Native planner selection CLI.
//! Usage: p5_planner <policy> <discovery.json> <qualifications.json>
//!
//! Policy forms:
//!   balanced | best_accuracy | lowest_latency | lowest_power
//!   preferred:<npu|dgpu|igpu|cpu>:<backend>
//!   locked:<npu|dgpu|igpu|cpu>:<backend>
//!
//! Prints {"ok":true,"decision":{...}} or {"ok":false,"error":{...}} as JSON.
use narrivy_compute::discovery::{DeviceClass, DiscoveryReport};
use narrivy_compute::planner::{
    ComputeError, DeviceRef, ManualChoice, Policy, ResolvedDecision, select,
};
use narrivy_compute::qualify::QualificationRegistry;
use std::path::PathBuf;

fn parse_class(s: &str) -> Option<DeviceClass> {
    match s {
        "npu" => Some(DeviceClass::Npu),
        "dgpu" | "d_gpu" => Some(DeviceClass::DGpu),
        "igpu" | "i_gpu" => Some(DeviceClass::IGpu),
        "cpu" => Some(DeviceClass::Cpu),
        _ => None,
    }
}

fn parse_policy(s: &str) -> Option<Policy> {
    match s {
        "balanced" => return Some(Policy::Balanced),
        "best_accuracy" => return Some(Policy::BestAccuracy),
        "lowest_latency" => return Some(Policy::LowestLatency),
        "lowest_power" => return Some(Policy::LowestPower),
        _ => {}
    }
    let (kind, rest) = s.split_once(':')?;
    let (class_s, backend) = rest.split_once(':')?;
    let device = DeviceRef {
        class: parse_class(class_s)?,
        backend: backend.to_string(),
    };
    match kind {
        "preferred" => Some(Policy::Manual(ManualChoice::Preferred(device))),
        "locked" => Some(Policy::Manual(ManualChoice::Locked(device))),
        _ => None,
    }
}

fn error_payload(err: &ComputeError) -> serde_json::Value {
    let kind = match err {
        ComputeError::LockedDeviceUnavailable { .. } => "LockedDeviceUnavailable",
        ComputeError::NoQualifiedDevice => "NoQualifiedDevice",
        ComputeError::UnsupportedCombo(_) => "UnsupportedCombo",
        ComputeError::SidecarSpawn(_) => "SidecarSpawn",
        ComputeError::SidecarTimeout(_) => "SidecarTimeout",
    };
    serde_json::json!({ "kind": kind, "message": err.to_string() })
}

fn decision_payload(d: &ResolvedDecision) -> serde_json::Value {
    serde_json::json!({
        "device": d.device,
        "reason_chain": d.reason_chain,
        "summary": d.summary(),
    })
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let policy_s = args.next().ok_or("missing policy argument")?;
    let discovery_path = PathBuf::from(args.next().ok_or("missing discovery.json argument")?);
    let quals_path = PathBuf::from(args.next().ok_or("missing qualifications.json argument")?);

    let policy =
        parse_policy(&policy_s).ok_or_else(|| format!("unparsable policy '{policy_s}'"))?;
    let discovery_text = std::fs::read_to_string(&discovery_path)
        .map_err(|e| format!("read {}: {e}", discovery_path.display()))?;
    let discovery: DiscoveryReport =
        serde_json::from_str(&discovery_text).map_err(|e| format!("parse discovery: {e}"))?;
    let registry = QualificationRegistry::load(&quals_path)
        .map_err(|e| format!("load qualifications: {e}"))?;

    match select(&policy, &registry, &discovery) {
        Ok(decision) => println!(
            "{}",
            serde_json::json!({ "ok": true, "decision": decision_payload(&decision) })
        ),
        Err(err) => println!(
            "{}",
            serde_json::json!({ "ok": false, "error": error_payload(&err) })
        ),
    }
    Ok(())
}

fn main() {
    if let Err(msg) = run() {
        eprintln!("{msg}");
        std::process::exit(2);
    }
}
