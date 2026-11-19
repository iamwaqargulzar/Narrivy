//! `\GPU Engine(*)\Running Time` and `\GPU Process Memory(*)\Shared Usage`
//! through PDH, for `accelerator_usage`.

use windows::Win32::System::Performance::{
    PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PDH_RAW_COUNTER_ITEM_W, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetRawCounterArrayW, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

/// The most counter instances one reading keeps (a busy desktop has a
/// few hundred).
const MAX_INSTANCES: usize = 8192;

/// A PDH query closed when dropped.
struct Query(PDH_HQUERY);

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful PdhOpenQueryW and is
        // closed exactly once, here.
        unsafe {
            let _ = PdhCloseQuery(self.0);
        }
    }
}

/// One collection of `\GPU Engine(*)\Running Time`: its time stamp and
/// every instance's raw value, both in 100 ns units.
pub(crate) fn read_engine_running_time() -> Option<(u64, Vec<(String, u64)>)> {
    read_raw_array(w!("\\GPU Engine(*)\\Running Time")).filter(|(stamp, _)| *stamp != 0)
}

/// One collection of `\GPU Process Memory(*)\Shared Usage`: every
/// instance's (`pid_<pid>_luid_<adapter>_phys_<n>`) system memory that the
/// GPU or NPU driver holds for that process, in bytes.
pub(crate) fn read_process_shared_memory() -> Option<Vec<(String, u64)>> {
    read_raw_array(w!("\\GPU Process Memory(*)\\Shared Usage")).map(|(_, samples)| samples)
}

/// Every instance's raw first value of one wildcard counter, with the
/// latest time stamp (100 ns units).
fn read_raw_array(path: PCWSTR) -> Option<(u64, Vec<(String, u64)>)> {
    let mut handle = PDH_HQUERY::default();
    // SAFETY: a null data source means the live counters; `handle` is a
    // valid out pointer.
    if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut handle) } != 0 {
        return None;
    }
    let query = Query(handle);
    let mut counter = PDH_HCOUNTER::default();
    // SAFETY: the query is open; the path is a static wide string; the
    // counter handle is owned by the query and freed with it.
    if unsafe { PdhAddEnglishCounterW(query.0, path, 0, &mut counter) } != 0 {
        return None;
    }
    // SAFETY: the query is open.
    if unsafe { PdhCollectQueryData(query.0) } != 0 {
        return None;
    }
    let mut bytes = 0_u32;
    let mut count = 0_u32;
    // SAFETY: a size query (no buffer) writes only the two counts.
    let status = unsafe { PdhGetRawCounterArrayW(counter, &mut bytes, &mut count, None) };
    if status != PDH_MORE_DATA || bytes == 0 {
        return None;
    }
    // The buffer holds the items followed by their names; u64 words keep
    // it aligned for the items.
    let words = (bytes as usize).div_ceil(std::mem::size_of::<u64>());
    let mut buffer = vec![0_u64; words];
    let items = buffer.as_mut_ptr().cast::<PDH_RAW_COUNTER_ITEM_W>();
    // SAFETY: `buffer` is at least `bytes` long and suitably aligned;
    // PDH writes `count` items and their names inside it.
    if unsafe { PdhGetRawCounterArrayW(counter, &mut bytes, &mut count, Some(items)) } != 0 {
        return None;
    }
    let count = (count as usize).min(MAX_INSTANCES);
    let mut stamp = 0_u64;
    let mut samples = Vec::with_capacity(count);
    for index in 0..count {
        // SAFETY: index < count, the number of items PDH wrote.
        let item = unsafe { &*items.add(index) };
        if item.RawValue.CStatus != 0 {
            continue;
        }
        // SAFETY: PDH set szName to a NUL-terminated string inside
        // `buffer`, which is still alive.
        let Ok(name) = (unsafe { item.szName.to_string() }) else {
            continue;
        };
        let time = item.RawValue.TimeStamp;
        stamp = stamp.max((u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime));
        samples.push((name, u64::try_from(item.RawValue.FirstValue).unwrap_or(0)));
    }
    drop(buffer);
    Some((stamp, samples))
}
