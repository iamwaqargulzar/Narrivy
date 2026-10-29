//! How busy this computer's GPU and NPU are, from the counters their kernel
//! drivers publish. Content-free: running totals, read only.
//!
//! - GPU: the work each GPU client has done on each engine class (render,
//!   compute, copy, video), from the DRM `fdinfo` of the processes this user
//!   can read (`drm-cycles-<class>` against `drm-total-cycles-<class>`, as
//!   GPU monitors read them). The busiest class is the GPU's use. Work done
//!   for given processes (Narrivy's) is counted apart too. An older counter,
//!   the time the main engines were idle (`gtidle`), is kept for drivers
//!   without cycle counts; it reads high, since the GPU wakes for small work.
//! - NPU: the time Intel's NPU has been busy, from `intel_vpu`
//!   (`/sys/class/accel/accel*/device/npu_busy_time_us`).
//!
//! A share needs two readings over time; the caller keeps the last one.
//! A counter that is not there is `None` (no such device or driver), never
//! a claim that the device is idle.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

const DRM_DIR: &str = "/sys/class/drm";
const ACCEL_DIR: &str = "/sys/class/accel";

/// The running totals at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AcceleratorCounters {
    /// Milliseconds the GPU's main engines have been idle.
    pub gpu_idle_ms: Option<u64>,
    /// Microseconds the NPU has been busy.
    pub npu_busy_us: Option<u64>,
}

/// The GPU work done so far, per engine class (`rcs`, `ccs`, `bcs`, …):
/// cycles busy summed over every client, cycles busy for the given
/// processes, and the class's clock (total cycles) when read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuWork {
    pub busy: BTreeMap<String, u64>,
    pub own: BTreeMap<String, u64>,
    pub total: BTreeMap<String, u64>,
}

/// Read every GPU client's work now; `own` names the processes counted
/// apart. `None` when no client reports cycles (another driver, or none
/// readable).
#[must_use]
pub fn gpu_work(own: &HashSet<u32>) -> Option<GpuWork> {
    gpu_work_under(Path::new("/proc"), own)
}

/// Bounds on what one reading looks at.
const MAX_PROCESSES: usize = 4096;
const MAX_FDS: usize = 512;

fn gpu_work_under(proc_dir: &Path, own: &HashSet<u32>) -> Option<GpuWork> {
    let mut work = GpuWork::default();
    // A client open in several processes (shared on fork) counts once.
    let mut clients: HashSet<(String, String)> = HashSet::new();
    let processes = std::fs::read_dir(proc_dir).ok()?;
    for process in processes.flatten().take(MAX_PROCESSES) {
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten().take(MAX_FDS) {
            // Only GPU device files.
            if !std::fs::read_link(fd.path()).is_ok_and(|target| target.starts_with("/dev/dri")) {
                continue;
            }
            let Ok(info) =
                std::fs::read_to_string(process.path().join("fdinfo").join(fd.file_name()))
            else {
                continue;
            };
            let field = |key: &str| {
                info.lines()
                    .find_map(|line| {
                        line.strip_prefix(key)
                            .and_then(|rest| rest.strip_prefix(':'))
                    })
                    .map(str::trim)
            };
            let Some(client) = field("drm-client-id") else {
                continue;
            };
            let device = field("drm-pdev").unwrap_or_default().to_owned();
            if !clients.insert((device, client.to_owned())) {
                continue;
            }
            for line in info.lines() {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let Ok(value) = value.trim().parse::<u64>() else {
                    continue;
                };
                if let Some(class) = key.strip_prefix("drm-total-cycles-") {
                    let total = work.total.entry(class.to_owned()).or_default();
                    *total = (*total).max(value);
                } else if let Some(class) = key.strip_prefix("drm-cycles-") {
                    *work.busy.entry(class.to_owned()).or_default() += value;
                    if own.contains(&pid) {
                        *work.own.entry(class.to_owned()).or_default() += value;
                    }
                }
            }
        }
    }
    (!work.total.is_empty()).then_some(work)
}

/// How busy the GPU was between two readings, in thousandths: the busiest
/// engine class's share of its clock, over all clients and over the `own`
/// ones. Clients that came or went in between are left out of their class.
#[must_use]
pub fn gpu_work_permille(before: &GpuWork, now: &GpuWork) -> Option<(u32, u32)> {
    let mut all = None::<u32>;
    let mut own = 0_u32;
    for (class, total_now) in &now.total {
        let Some(total_before) = before.total.get(class) else {
            continue;
        };
        let elapsed = total_now.saturating_sub(*total_before);
        if elapsed == 0 {
            continue;
        }
        let share = |map_now: &BTreeMap<String, u64>, map_before: &BTreeMap<String, u64>| {
            let done = map_now
                .get(class)
                .copied()
                .unwrap_or(0)
                .saturating_sub(map_before.get(class).copied().unwrap_or(0));
            u32::try_from((u128::from(done) * 1000 / u128::from(elapsed)).min(1000)).unwrap_or(1000)
        };
        let class_all = share(&now.busy, &before.busy);
        if all.is_none_or(|best| class_all > best) {
            all = Some(class_all);
        }
        own = own.max(share(&now.own, &before.own));
    }
    all.map(|all| (all, own.min(all)))
}

/// Read the counters now.
#[must_use]
pub fn counters() -> AcceleratorCounters {
    counters_under(Path::new(DRM_DIR), Path::new(ACCEL_DIR))
}

fn counters_under(drm: &Path, accel: &Path) -> AcceleratorCounters {
    AcceleratorCounters {
        gpu_idle_ms: first_number(
            &entries(drm, "card")
                .into_iter()
                .flat_map(|card| {
                    [
                        card.join("device/tile0/gt0/gtidle/idle_residency_ms"),
                        card.join("gt/gt0/rc6_residency_ms"),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        npu_busy_us: first_number(
            &entries(accel, "accel")
                .into_iter()
                .map(|device| device.join("device/npu_busy_time_us"))
                .collect::<Vec<_>>(),
        ),
    }
}

/// `card0`, `card1`, … (not `card0-eDP-1`), in order.
fn entries(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|read| {
            read.flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.strip_prefix(prefix))
                        .is_some_and(|rest| {
                            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                        })
                })
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn first_number(paths: &[PathBuf]) -> Option<u64> {
    paths.iter().find_map(|path| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| text.trim().parse().ok())
    })
}

/// The share of `elapsed_ms` a device was busy, in thousandths, from two
/// GPU idle readings.
#[must_use]
pub fn gpu_busy_permille(idle_before_ms: u64, idle_now_ms: u64, elapsed_ms: u64) -> Option<u32> {
    if elapsed_ms == 0 || idle_now_ms < idle_before_ms {
        return None;
    }
    let idle = (idle_now_ms - idle_before_ms).min(elapsed_ms);
    u32::try_from(1000 - idle * 1000 / elapsed_ms).ok()
}

/// The share of `elapsed_ms` the NPU was busy, in thousandths, from two
/// busy readings.
#[must_use]
pub fn npu_busy_permille(busy_before_us: u64, busy_now_us: u64, elapsed_ms: u64) -> Option<u32> {
    if elapsed_ms == 0 || busy_now_us < busy_before_us {
        return None;
    }
    let busy_ms = (busy_now_us - busy_before_us) / 1000;
    u32::try_from((busy_ms * 1000 / elapsed_ms).min(1000)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_read_from_the_drivers_files() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let drm = root.path().join("drm");
        let accel = root.path().join("accel");
        std::fs::create_dir_all(drm.join("card0-eDP-1"))?;
        std::fs::create_dir_all(drm.join("card0/device/tile0/gt0/gtidle"))?;
        std::fs::write(
            drm.join("card0/device/tile0/gt0/gtidle/idle_residency_ms"),
            "197489\n",
        )?;
        std::fs::create_dir_all(accel.join("accel0/device"))?;
        std::fs::write(accel.join("accel0/device/npu_busy_time_us"), "484228827\n")?;
        let read = counters_under(&drm, &accel);
        assert_eq!(read.gpu_idle_ms, Some(197_489));
        assert_eq!(read.npu_busy_us, Some(484_228_827));
        let none = counters_under(&root.path().join("nothing"), &root.path().join("nothing"));
        assert_eq!(none, AcceleratorCounters::default());
        Ok(())
    }

    #[test]
    fn gpu_work_is_summed_per_class_once_per_client() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let dri = root.path().join("dri");
        std::fs::create_dir_all(&dri)?;
        let client = |pid: u32, fd: u32, id: u32, rcs: u64, total: u64| -> std::io::Result<()> {
            let dir = root.path().join("proc").join(pid.to_string());
            std::fs::create_dir_all(dir.join("fd"))?;
            std::fs::create_dir_all(dir.join("fdinfo"))?;
            std::os::unix::fs::symlink("/dev/dri/renderD128", dir.join("fd").join(fd.to_string()))?;
            std::fs::write(
                dir.join("fdinfo").join(fd.to_string()),
                format!(
                    "drm-driver:\txe\ndrm-pdev:\t0000:00:02.0\ndrm-client-id:\t{id}\ndrm-cycles-rcs:\t{rcs}\ndrm-total-cycles-rcs:\t{total}\n"
                ),
            )
        };
        client(10, 3, 1, 100, 1000)?;
        client(10, 4, 1, 100, 1000)?; // the same client again: once
        client(20, 3, 2, 50, 1000)?;
        let own = HashSet::from([20]);
        let work = gpu_work_under(&root.path().join("proc"), &own)
            .ok_or_else(|| std::io::Error::other("none"))?;
        assert_eq!(work.busy.get("rcs"), Some(&150));
        assert_eq!(work.own.get("rcs"), Some(&50));
        let later = GpuWork {
            busy: BTreeMap::from([("rcs".to_owned(), 400)]),
            own: BTreeMap::from([("rcs".to_owned(), 100)]),
            total: BTreeMap::from([("rcs".to_owned(), 2000)]),
        };
        assert_eq!(gpu_work_permille(&work, &later), Some((250, 50)));
        Ok(())
    }

    #[test]
    fn shares_are_bounded() {
        assert_eq!(gpu_busy_permille(1000, 1750, 1000), Some(250));
        assert_eq!(gpu_busy_permille(1000, 3000, 1000), Some(0));
        assert_eq!(gpu_busy_permille(2000, 1000, 1000), None);
        assert_eq!(npu_busy_permille(0, 500_000, 1000), Some(500));
        assert_eq!(npu_busy_permille(0, 5_000_000, 1000), Some(1000));
        assert_eq!(npu_busy_permille(10, 5, 1000), None);
    }
}
