//! Content-free resource usage of one process tree (a daemon plus the helper
//! and worker processes it spawned), read through a Toolhelp process
//! snapshot, `GetProcessTimes` and `K32GetProcessMemoryInfo`.
//!
//! The walk is bounded like the Linux `/proc` walk: it examines at most
//! [`MAX_PROC_ENTRIES`] snapshot entries and counts at most
//! [`MAX_TREE_PROCESSES`] processes. A process that exits during the walk is
//! skipped. Windows reuses process ids, so a child is linked to a parent only
//! when it was created after that parent. Nothing here reads command lines,
//! environments or any other content.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
};
use windows_sys::Win32::System::Threading::{
    ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetProcessTimes, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};

/// Upper bound on snapshot entries examined per sample.
pub const MAX_PROC_ENTRIES: usize = 8192;
/// Upper bound on processes counted in one tree.
pub const MAX_TREE_PROCESSES: usize = 64;

/// One sample of a process tree's resource usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeUsage {
    /// Memory every process in the tree really holds, in bytes: the private
    /// working set. Model files mapped from disk are left out, as on Linux.
    /// Where Windows does not report that split, the full working set.
    pub rss_bytes: u64,
    /// User plus kernel CPU time of every process in the tree, in clock
    /// ticks of [`clock_ticks_per_second`].
    pub cpu_ticks: u64,
    /// Processes counted, including the root.
    pub processes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessFacts {
    created: u64,
    cpu_ticks: u64,
    rss_bytes: u64,
}

/// CPU time is reported in FILETIME units of 100 ns.
#[must_use]
pub fn clock_ticks_per_second() -> u64 {
    10_000_000
}

/// Active logical CPUs across all processor groups, at least 1.
#[must_use]
pub fn online_cpus() -> u64 {
    // SAFETY: GetActiveProcessorCount takes a group number and touches no
    // caller memory.
    let cpus = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    u64::from(cpus).max(1)
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned open by the system and is closed
        // exactly once here.
        unsafe { CloseHandle(self.0) };
    }
}

fn filetime(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

fn read_process(pid: u32) -> Option<ProcessFacts> {
    // SAFETY: OpenProcess takes plain values; a null result is handled.
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let process = OwnedHandle(handle);
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the handle is open with query rights; all four out-pointers
    // are valid FILETIMEs.
    if unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) } == 0
    {
        return None;
    }
    Some(ProcessFacts {
        created: filetime(created),
        cpu_ticks: filetime(kernel).saturating_add(filetime(user)),
        rss_bytes: private_working_set(&process).unwrap_or(0),
    })
}

/// The private working set (EX2, Windows 10 22H2 and later), else the full
/// working set.
fn private_working_set(process: &OwnedHandle) -> Option<u64> {
    // SAFETY: an all-zero PROCESS_MEMORY_COUNTERS_EX2 is a valid value of
    // this plain-data struct.
    let mut counters: PROCESS_MEMORY_COUNTERS_EX2 = unsafe { std::mem::zeroed() };
    let size = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX2>()).ok()?;
    counters.cb = size;
    // SAFETY: `counters` is writable for `size` bytes; the EX2 layout starts
    // with PROCESS_MEMORY_COUNTERS, which is what the cast names.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            process.0,
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX2).cast::<PROCESS_MEMORY_COUNTERS>(),
            size,
        )
    };
    if ok != 0 {
        return u64::try_from(counters.PrivateWorkingSetSize).ok();
    }
    // SAFETY: as above, for the base struct.
    let mut basic: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let size = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
    basic.cb = size;
    // SAFETY: `basic` is writable for `size` bytes.
    if unsafe { K32GetProcessMemoryInfo(process.0, &mut basic, size) } == 0 {
        return None;
    }
    u64::try_from(basic.WorkingSetSize).ok()
}

/// `(pid, parent pid)` for at most [`MAX_PROC_ENTRIES`] running processes.
fn parent_links() -> Vec<(u32, u32)> {
    // SAFETY: CreateToolhelp32Snapshot takes plain values; failure is
    // INVALID_HANDLE_VALUE.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let snapshot = OwnedHandle(snapshot);
    let mut links = Vec::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `entry.dwSize` is set as the API requires; the snapshot handle
    // is open.
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while more && links.len() < MAX_PROC_ENTRIES {
        links.push((entry.th32ProcessID, entry.th32ParentProcessID));
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    links
}

/// Sum the resource usage of `root` and every descendant process.
///
/// Returns `None` when the root itself cannot be read.
#[must_use]
pub fn process_tree_usage(root: u32) -> Option<TreeUsage> {
    let root_facts = read_process(root)?;
    let mut usage = TreeUsage {
        rss_bytes: root_facts.rss_bytes,
        cpu_ticks: root_facts.cpu_ticks,
        processes: 1,
    };
    for (pid, facts) in descendants(root, root_facts.created) {
        if pid == root {
            continue;
        }
        usage.rss_bytes = usage.rss_bytes.saturating_add(facts.rss_bytes);
        usage.cpu_ticks = usage.cpu_ticks.saturating_add(facts.cpu_ticks);
        usage.processes += 1;
    }
    Some(usage)
}

/// `root` and its descendant processes (bounded like the usage sum).
#[must_use]
pub fn process_tree(root: u32) -> HashSet<u32> {
    let mut members: HashSet<u32> = HashSet::from([root]);
    if let Some(root_facts) = read_process(root) {
        members.extend(
            descendants(root, root_facts.created)
                .into_iter()
                .map(|(pid, _)| pid),
        );
    }
    members
}

/// Breadth-first walk of the parent links from `root`, bounded, linking a
/// child only when it was created after its recorded parent (process ids are
/// reused on Windows).
fn descendants(root: u32, root_created: u64) -> Vec<(u32, ProcessFacts)> {
    let links = parent_links();
    let mut created: HashMap<u32, u64> = HashMap::from([(root, root_created)]);
    let mut found = Vec::new();
    loop {
        let mut grew = false;
        for (pid, ppid) in &links {
            if created.len() >= MAX_TREE_PROCESSES {
                return found;
            }
            if *pid == *ppid || created.contains_key(pid) {
                continue;
            }
            let Some(parent_created) = created.get(ppid).copied() else {
                continue;
            };
            let Some(facts) = read_process(*pid) else {
                continue;
            };
            if facts.created < parent_created {
                continue;
            }
            created.insert(*pid, facts.created);
            found.push((*pid, facts));
            grew = true;
        }
        if !grew {
            return found;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_is_readable_and_counted() {
        let usage = process_tree_usage(std::process::id());
        assert!(usage.is_some_and(|usage| usage.processes >= 1));
        assert!(process_tree(std::process::id()).contains(&std::process::id()));
        assert!(online_cpus() >= 1);
    }
}
