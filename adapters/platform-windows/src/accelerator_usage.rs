//! How busy this computer's GPU is, from Windows' own `GPU Engine`
//! performance counters (the ones Task Manager reads). Content-free: running
//! totals, read only.
//!
//! - GPU: each `\GPU Engine(*)\Running Time` instance is one process's time
//!   on one engine (`pid_<pid>_luid_<adapter>_phys_<n>_eng_<n>_engtype_<type>`),
//!   in 100 ns units. Summed per engine, against the time that passed, the
//!   busiest engine is the GPU's use; work done for given processes
//!   (Narrivy's) is counted apart too. This is the same reading as Linux's
//!   per-client engine cycles, so the portable `GpuWork` shape fits.
//! - NPU: an NPU's engines are in the same counters, under its own adapter
//!   id (DXCore says which adapters are NPUs: hardware with machine
//!   learning and no graphics). Its busiest engine's running time is the
//!   NPU's busy time, and it is left out of the GPU's. No NPU: `no_device`.
//!
//! - Memory: `\GPU Process Memory(*)\Shared Usage` is the system memory
//!   each process's GPU and NPU allocations take, per adapter. It is not in
//!   the process's working set, so [`device_memory_bytes`] adds it for
//!   Narrivy's processes. Dedicated (on-card) memory is not system memory
//!   and is left out.
//!
//! A share needs two readings over time; the caller keeps the last one.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashSet};

/// The running totals at one moment: the NPU's busy time (microseconds),
/// when there is an NPU. The GPU idle-time counter Linux reads has no
/// Windows source (the GPU is read per engine, [`gpu_work`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AcceleratorCounters {
    pub gpu_idle_ms: Option<u64>,
    pub npu_busy_us: Option<u64>,
}

/// GPU work so far per engine: time busy summed over every process, time
/// busy for the given processes, and the clock (time passed) when read, all
/// in 100 ns units.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuWork {
    pub busy: BTreeMap<String, u64>,
    pub own: BTreeMap<String, u64>,
    pub total: BTreeMap<String, u64>,
}

/// The NPU's busy time so far; nothing without an NPU.
#[must_use]
pub fn counters() -> AcceleratorCounters {
    let npus = os::npu_luids();
    if npus.is_empty() {
        return AcceleratorCounters::default();
    }
    // An NPU nothing has used yet has no counter instances: idle so far.
    let samples = os::read_engine_running_time().map(|(_, samples)| samples);
    AcceleratorCounters {
        gpu_idle_ms: None,
        npu_busy_us: Some(npu_busy_us(samples.as_deref().unwrap_or_default(), npus)),
    }
}

/// The adapter id as the counters spell it: `0x<high>_0x<low>`, eight
/// upper-case hex digits each.
#[must_use]
pub fn luid_name(high: u32, low: u32) -> String {
    format!("0x{high:08X}_0x{low:08X}")
}

/// Whether the engine key (`<luid>/<phys>/<eng>/<type>`) is on one of the
/// `npus`.
fn on_npu(engine: &str, npus: &[String]) -> bool {
    engine
        .split('/')
        .next()
        .is_some_and(|luid| npus.iter().any(|npu| npu.eq_ignore_ascii_case(luid)))
}

/// The busiest NPU engine's running time summed over every process, in
/// microseconds.
fn npu_busy_us(samples: &[(String, u64)], npus: &[String]) -> u64 {
    let mut engines = BTreeMap::<String, u64>::new();
    for (name, running) in samples {
        if let Some((_, engine)) = parse_instance(name).filter(|(_, engine)| on_npu(engine, npus)) {
            *engines.entry(engine).or_default() += running;
        }
    }
    engines.into_values().max().unwrap_or(0) / 10
}

/// Read every process's GPU engine time now; `own` names the processes
/// counted apart. `None` when Windows publishes no GPU engine counters (no
/// WDDM 2 GPU, or the counters are off).
#[must_use]
pub fn gpu_work(own: &HashSet<u32>) -> Option<GpuWork> {
    let (stamp, samples) = os::read_engine_running_time()?;
    work_from_samples(stamp, &samples, own, os::npu_luids())
}

/// Build the per-engine totals from `(instance name, running time)` pairs
/// read at `stamp` (100 ns units), leaving out the `npus`' engines.
fn work_from_samples(
    stamp: u64,
    samples: &[(String, u64)],
    own: &HashSet<u32>,
    npus: &[String],
) -> Option<GpuWork> {
    let mut work = GpuWork::default();
    for (name, running) in samples {
        let Some((pid, engine)) = parse_instance(name).filter(|(_, engine)| !on_npu(engine, npus))
        else {
            continue;
        };
        *work.busy.entry(engine.clone()).or_default() += running;
        if own.contains(&pid) {
            *work.own.entry(engine.clone()).or_default() += running;
        }
        work.total.insert(engine, stamp);
    }
    (!work.total.is_empty()).then_some(work)
}

/// `pid_1234_luid_0x0_0xABC_phys_0_eng_3_engtype_Compute` → (1234,
/// `0x0_0xABC/0/3/Compute`). The engine key leaves the process out, so one
/// engine's time sums over every process.
fn parse_instance(name: &str) -> Option<(u32, String)> {
    let rest = name.strip_prefix("pid_")?;
    let (pid, rest) = rest.split_once('_')?;
    let pid = pid.parse::<u32>().ok()?;
    let rest = rest.strip_prefix("luid_")?;
    let (luid, rest) = rest.split_once("_phys_")?;
    let (phys, rest) = rest.split_once("_eng_")?;
    let (eng, kind) = rest.split_once("_engtype_")?;
    // Hex digits in one case, so one adapter is one key however it is spelt.
    let luid = luid.to_ascii_uppercase().replace("0X", "0x");
    Some((pid, format!("{luid}/{phys}/{eng}/{kind}")))
}

/// How busy the GPU was between two readings, in thousandths: the busiest
/// engine's share of the time passed, over all processes and over the `own`
/// ones. Engines that came or went in between are left out.
#[must_use]
pub fn gpu_work_permille(before: &GpuWork, now: &GpuWork) -> Option<(u32, u32)> {
    let mut all = None::<u32>;
    let mut own = 0_u32;
    for (engine, total_now) in &now.total {
        let Some(total_before) = before.total.get(engine) else {
            continue;
        };
        let elapsed = total_now.saturating_sub(*total_before);
        if elapsed == 0 {
            continue;
        }
        let share = |map_now: &BTreeMap<String, u64>, map_before: &BTreeMap<String, u64>| {
            let done = map_now
                .get(engine)
                .copied()
                .unwrap_or(0)
                .saturating_sub(map_before.get(engine).copied().unwrap_or(0));
            u32::try_from((u128::from(done) * 1000 / u128::from(elapsed)).min(1000)).unwrap_or(1000)
        };
        let engine_all = share(&now.busy, &before.busy);
        if all.is_none_or(|best| engine_all > best) {
            all = Some(engine_all);
        }
        own = own.max(share(&now.own, &before.own));
    }
    all.map(|all| (all, own.min(all)))
}

/// The system memory the GPU and NPU drivers hold for the `own` processes
/// (model weights and buffers of a worker on the NPU or GPU), in bytes.
/// `None` when Windows publishes no GPU process memory counters.
#[must_use]
pub fn device_memory_bytes(own: &HashSet<u32>) -> Option<u64> {
    os::read_process_shared_memory().map(|samples| own_memory(&samples, own))
}

/// Sum the `own` processes' instances (`pid_<pid>_luid_…_phys_<n>`), every
/// adapter included.
fn own_memory(samples: &[(String, u64)], own: &HashSet<u32>) -> u64 {
    samples
        .iter()
        .filter(|(name, _)| {
            name.strip_prefix("pid_")
                .and_then(|rest| rest.split_once('_'))
                .and_then(|(pid, _)| pid.parse::<u32>().ok())
                .is_some_and(|pid| own.contains(&pid))
        })
        .fold(0_u64, |sum, (_, bytes)| sum.saturating_add(*bytes))
}

/// No idle-time counter on Windows.
#[must_use]
pub fn gpu_busy_permille(_before: u64, _now: u64, _elapsed_ms: u64) -> Option<u32> {
    None
}

/// How busy the NPU was between two readings of its busy time
/// (microseconds), in thousandths of `elapsed_ms`; as on Linux.
#[must_use]
pub fn npu_busy_permille(busy_before_us: u64, busy_now_us: u64, elapsed_ms: u64) -> Option<u32> {
    if elapsed_ms == 0 || busy_now_us < busy_before_us {
        return None;
    }
    let busy_ms = (busy_now_us - busy_before_us) / 1000;
    u32::try_from((busy_ms * 1000 / elapsed_ms).min(1000)).ok()
}

#[cfg(windows)]
mod os {
    pub(super) use crate::os::gpu_counters::{
        read_engine_running_time, read_process_shared_memory,
    };

    /// The NPUs' adapter ids, looked up once (adapters are fixed while the
    /// service runs).
    pub(super) fn npu_luids() -> &'static [String] {
        static NPUS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        NPUS.get_or_init(crate::os::npu_adapters::npu_luids)
    }
}

#[cfg(not(windows))]
mod os {
    /// Other systems have no GPU Engine counters.
    pub(super) fn read_engine_running_time() -> Option<(u64, Vec<(String, u64)>)> {
        None
    }

    /// Nor GPU process memory counters.
    pub(super) fn read_process_shared_memory() -> Option<Vec<(String, u64)>> {
        None
    }

    /// Nor NPU adapters.
    pub(super) fn npu_luids() -> &'static [String] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_names_split_into_process_and_engine() {
        assert_eq!(
            parse_instance("pid_1234_luid_0x00000000_0x0000D1A3_phys_0_eng_3_engtype_Compute"),
            Some((1234, "0x00000000_0x0000D1A3/0/3/Compute".to_owned()))
        );
        assert_eq!(parse_instance("_Total"), None);
        assert_eq!(parse_instance("pid_x_luid_0_phys_0_eng_0_engtype_3D"), None);
    }

    #[test]
    fn the_busiest_engine_is_the_gpu_share_and_own_work_is_counted_apart() {
        let shares = two_readings().and_then(|(before, now)| gpu_work_permille(&before, &now));
        assert_eq!(shares, Some((500, 300)));
    }

    fn two_readings() -> Option<(GpuWork, GpuWork)> {
        let own = HashSet::from([7]);
        let before = work_from_samples(
            1_000,
            &[
                ("pid_7_luid_a_phys_0_eng_0_engtype_3D".into(), 100),
                ("pid_9_luid_a_phys_0_eng_0_engtype_3D".into(), 100),
                ("pid_7_luid_a_phys_0_eng_1_engtype_Copy".into(), 0),
            ],
            &own,
            &[],
        )?;
        // 1000 units later: the 3D engine did 300 (pid 7) + 200 (pid 9),
        // the copy engine 100.
        let now = work_from_samples(
            2_000,
            &[
                ("pid_7_luid_a_phys_0_eng_0_engtype_3D".into(), 400),
                ("pid_9_luid_a_phys_0_eng_0_engtype_3D".into(), 300),
                ("pid_7_luid_a_phys_0_eng_1_engtype_Copy".into(), 100),
            ],
            &own,
            &[],
        )?;
        Some((before, now))
    }

    #[test]
    fn no_counters_means_no_reading_not_an_idle_gpu() {
        assert_eq!(work_from_samples(5, &[], &HashSet::new(), &[]), None);
        assert_eq!(counters(), AcceleratorCounters::default());
    }

    #[test]
    fn device_memory_counts_only_narrivys_processes_on_every_adapter() {
        let samples = [
            ("pid_7_luid_0x00000000_0x0000BEEF_phys_0".to_owned(), 500),
            ("pid_7_luid_0x00000000_0x0000D1A3_phys_0".to_owned(), 20),
            ("pid_8_luid_0x00000000_0x0000BEEF_phys_0".to_owned(), 5),
            ("pid_9_luid_0x00000000_0x0000BEEF_phys_0".to_owned(), 9000),
            ("_Total".to_owned(), 1_000_000),
        ];
        assert_eq!(own_memory(&samples, &HashSet::from([7, 8])), 525);
        assert_eq!(own_memory(&samples, &HashSet::new()), 0);
    }

    #[test]
    fn npu_engines_count_for_the_npu_and_not_the_gpu() {
        let npus = [luid_name(0, 0xBEEF)];
        assert_eq!(npus[0], "0x00000000_0x0000BEEF");
        let samples = [
            (
                "pid_7_luid_0x00000000_0x0000D1A3_phys_0_eng_0_engtype_3D".to_owned(),
                100,
            ),
            (
                "pid_7_luid_0x00000000_0x0000beef_phys_0_eng_0_engtype_Compute".to_owned(),
                30,
            ),
            (
                "pid_9_luid_0x00000000_0x0000BEEF_phys_0_eng_0_engtype_Compute".to_owned(),
                20,
            ),
            (
                "pid_9_luid_0x00000000_0x0000BEEF_phys_0_eng_1_engtype_Copy".to_owned(),
                10,
            ),
        ];
        // The compute engine: 30 + 20 units of 100 ns = 5 µs.
        assert_eq!(npu_busy_us(&samples, &npus), 5);
        let gpu = work_from_samples(1, &samples, &HashSet::new(), &npus);
        assert_eq!(
            gpu.map(|work| work.busy.into_keys().collect::<Vec<_>>()),
            Some(vec!["0x00000000_0x0000D1A3/0/0/3D".to_owned()])
        );
        assert_eq!(npu_busy_permille(0, 500_000, 1000), Some(500));
        assert_eq!(npu_busy_permille(9, 5, 1000), None);
    }
}
