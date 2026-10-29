//! Content-free resource usage of one process tree (a daemon plus the helper
//! and worker processes it spawned), read from `/proc`.
//!
//! The walk is bounded: it scans at most [`MAX_PROC_ENTRIES`] `/proc`
//! entries and counts at most [`MAX_TREE_PROCESSES`] processes, so a busy
//! system cannot make a status request slow. A process that exits during the
//! walk is simply skipped. Nothing here reads command lines, environments or
//! any other content.

use std::collections::HashSet;
use std::fs;

/// Upper bound on `/proc` directory entries examined per sample.
pub const MAX_PROC_ENTRIES: usize = 8192;
/// Upper bound on processes counted in one tree.
pub const MAX_TREE_PROCESSES: usize = 64;

/// One sample of a process tree's resource usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeUsage {
    /// Memory every process in the tree really holds, in bytes: private
    /// resident memory (anonymous plus shared-memory pages). Model files
    /// mapped from disk are left out: the system can drop and re-read them
    /// at any time, like its file cache. Where that split is unavailable,
    /// the full resident size.
    pub rss_bytes: u64,
    /// User plus system CPU time of every process in the tree, in clock ticks.
    pub cpu_ticks: u64,
    /// Processes counted, including the root.
    pub processes: u32,
}

/// The fields of `/proc/<pid>/stat` this module needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StatFields {
    ppid: u32,
    cpu_ticks: u64,
    rss_pages: u64,
}

/// Parse `/proc/<pid>/stat`. The command name (field 2) may contain spaces
/// and parentheses, so parsing starts after its last closing parenthesis.
fn parse_stat(stat: &str) -> Option<StatFields> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `rest` starts at field 3 (state): ppid is field 4, utime 14, stime 15,
    // rss 24, so their indexes here are 1, 11, 12 and 21.
    let ppid = fields.get(1)?.parse().ok()?;
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let rss_pages: i64 = fields.get(21)?.parse().ok()?;
    Some(StatFields {
        ppid,
        cpu_ticks: utime.saturating_add(stime),
        rss_pages: u64::try_from(rss_pages).unwrap_or(0),
    })
}

fn read_stat(pid: u32) -> Option<StatFields> {
    let mut stat = parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
    if let Some(bytes) = fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|status| private_resident_bytes(&status))
    {
        stat.rss_pages = bytes / page_size().max(1);
    }
    Some(stat)
}

/// `RssAnon` plus `RssShmem` from `/proc/<pid>/status`, in bytes.
fn private_resident_bytes(status: &str) -> Option<u64> {
    let field = |name: &str| -> Option<u64> {
        let line = status.lines().find(|l| l.starts_with(name))?;
        let kib: u64 = line[name.len()..]
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .ok()?;
        Some(kib.saturating_mul(1024))
    };
    Some(field("RssAnon:")?.saturating_add(field("RssShmem:").unwrap_or(0)))
}

/// Clock ticks per second (`sysconf(_SC_CLK_TCK)`), for converting CPU ticks.
#[must_use]
pub fn clock_ticks_per_second() -> u64 {
    // SAFETY: sysconf has no pointer arguments and only reads a constant.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(ticks).ok().filter(|t| *t > 0).unwrap_or(100)
}

/// Online logical CPUs (`sysconf(_SC_NPROCESSORS_ONLN)`), at least 1.
#[must_use]
pub fn online_cpus() -> u64 {
    // SAFETY: sysconf has no pointer arguments and only reads a constant.
    let cpus = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    u64::try_from(cpus).ok().filter(|c| *c > 0).unwrap_or(1)
}

fn page_size() -> u64 {
    // SAFETY: sysconf has no pointer arguments and only reads a constant.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    u64::try_from(size).ok().filter(|s| *s > 0).unwrap_or(4096)
}

/// Sum the resource usage of `root` and every descendant process.
///
/// Returns `None` when the root itself cannot be read.
#[must_use]
pub fn process_tree_usage(root: u32) -> Option<TreeUsage> {
    let root_stat = read_stat(root)?;
    let page = page_size();
    let mut entries: Vec<(u32, StatFields)> = Vec::new();
    if let Ok(dir) = fs::read_dir("/proc") {
        for entry in dir.flatten().take(MAX_PROC_ENTRIES) {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == root {
                continue;
            }
            if let Some(stat) = read_stat(pid) {
                entries.push((pid, stat));
            }
        }
    }
    Some(sum_tree(root, root_stat, &entries, page))
}

/// `root` and its descendant processes (bounded like the usage sum).
#[must_use]
pub fn process_tree(root: u32) -> HashSet<u32> {
    let mut parents: Vec<(u32, u32)> = Vec::new();
    if let Ok(dir) = fs::read_dir("/proc") {
        for entry in dir.flatten().take(MAX_PROC_ENTRIES) {
            if let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
                && let Some(stat) = read_stat(pid)
            {
                parents.push((pid, stat.ppid));
            }
        }
    }
    let mut members: HashSet<u32> = HashSet::from([root]);
    loop {
        let before = members.len();
        for (pid, ppid) in &parents {
            if members.len() >= MAX_TREE_PROCESSES {
                return members;
            }
            if members.contains(ppid) {
                members.insert(*pid);
            }
        }
        if members.len() == before {
            return members;
        }
    }
}

/// Walk the parent links breadth-first from `root`, bounded.
fn sum_tree(
    root: u32,
    root_stat: StatFields,
    entries: &[(u32, StatFields)],
    page: u64,
) -> TreeUsage {
    let mut members: HashSet<u32> = HashSet::from([root]);
    let mut usage = TreeUsage {
        rss_bytes: root_stat.rss_pages.saturating_mul(page),
        cpu_ticks: root_stat.cpu_ticks,
        processes: 1,
    };
    loop {
        let mut grew = false;
        for (pid, stat) in entries {
            if members.len() >= MAX_TREE_PROCESSES {
                return usage;
            }
            if !members.contains(pid) && members.contains(&stat.ppid) {
                members.insert(*pid);
                usage.rss_bytes = usage
                    .rss_bytes
                    .saturating_add(stat.rss_pages.saturating_mul(page));
                usage.cpu_ticks = usage.cpu_ticks.saturating_add(stat.cpu_ticks);
                usage.processes += 1;
                grew = true;
            }
        }
        if !grew {
            return usage;
        }
    }
}

#[cfg(test)]
mod private_tests {
    use super::private_resident_bytes;

    #[test]
    fn mapped_model_files_are_not_counted() {
        let status = "VmRSS:\t 2178536 kB\nRssAnon:\t  741516 kB\nRssFile:\t 1435540 kB\nRssShmem:\t    1480 kB\n";
        assert_eq!(
            private_resident_bytes(status),
            Some((741_516 + 1_480) * 1024)
        );
        assert_eq!(private_resident_bytes("VmRSS: 5 kB\n"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat_line(pid: u32, comm: &str, ppid: u32, utime: u64, stime: u64, rss: i64) -> String {
        // 52 fields in total; only the ones parse_stat reads matter.
        let mut fields = vec!["0".to_owned(); 50];
        fields[0] = "S".to_owned(); // field 3
        fields[1] = ppid.to_string(); // field 4
        fields[11] = utime.to_string(); // field 14
        fields[12] = stime.to_string(); // field 15
        fields[21] = rss.to_string(); // field 24
        format!("{pid} ({comm}) {}", fields.join(" "))
    }

    #[test]
    fn parses_stat_even_when_the_command_name_has_spaces_and_parentheses() {
        let line = stat_line(42, "narrivy (worker) x", 7, 30, 12, 256);
        assert_eq!(
            parse_stat(&line),
            Some(StatFields {
                ppid: 7,
                cpu_ticks: 42,
                rss_pages: 256
            })
        );
    }

    #[test]
    fn rejects_truncated_stat() {
        assert_eq!(parse_stat("1 (x) S 0 0"), None);
        assert_eq!(parse_stat("no parenthesis"), None);
    }

    #[test]
    fn sums_descendants_and_ignores_unrelated_processes() {
        let root = StatFields {
            ppid: 1,
            cpu_ticks: 10,
            rss_pages: 100,
        };
        let entries = vec![
            (
                11,
                StatFields {
                    ppid: 10,
                    cpu_ticks: 5,
                    rss_pages: 50,
                },
            ), // child
            (
                12,
                StatFields {
                    ppid: 11,
                    cpu_ticks: 1,
                    rss_pages: 10,
                },
            ), // grandchild
            (
                99,
                StatFields {
                    ppid: 1,
                    cpu_ticks: 500,
                    rss_pages: 5000,
                },
            ), // unrelated
        ];
        let usage = sum_tree(10, root, &entries, 4096);
        assert_eq!(usage.processes, 3);
        assert_eq!(usage.cpu_ticks, 16);
        assert_eq!(usage.rss_bytes, 160 * 4096);
    }

    #[test]
    fn reads_the_current_process() -> Result<(), String> {
        let usage = process_tree_usage(std::process::id())
            .ok_or("the current process has no readable /proc entry")?;
        assert!(usage.processes >= 1);
        assert!(usage.rss_bytes > 0);
        assert!(clock_ticks_per_second() > 0);
        assert!(online_cpus() >= 1);
        Ok(())
    }
}
