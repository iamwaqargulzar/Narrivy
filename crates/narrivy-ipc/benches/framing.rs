//! Framing benchmark harness — p50 < 50µs budget.
//!
//! Run with `cargo bench --bench framing` (harness = false).
//! Also covered by unit test `ipc_framing_perf_p50_under_50us` in
//! `src/framing.rs` for CI gating.
//!
//! Zero-copy path documented in `src/framing.rs`: encode allocates once
//! (4 + payload), read validates length before alloc, payload borrowed via
//! `serde_json::from_slice` without extra copy.

use std::hint::black_box;
use std::io::Cursor;
use std::time::Instant;

use narrivy_ipc::framing::{MAX_FRAME_PAYLOAD_BYTES, encode_frame, read_frame};

fn bench_one(
    payload_len: usize,
    iterations: usize,
) -> Result<(u128, u128), Box<dyn std::error::Error>> {
    let payload = vec![b'x'; payload_len];
    let frame = encode_frame(&payload)?;
    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let enc = encode_frame(black_box(&payload))?;
        let mut cur = Cursor::new(enc);
        let dec = read_frame(&mut cur)?;
        black_box(dec);
        timings.push(start.elapsed().as_nanos());
    }
    timings.sort_unstable();
    let p50 = timings[timings.len() / 2];
    let p99 = timings[timings.len() * 99 / 100];
    // Also measure pure decode from cloned frame
    let mut decode_timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let mut cur = Cursor::new(frame.clone());
        let dec = read_frame(&mut cur)?;
        black_box(dec);
        decode_timings.push(start.elapsed().as_nanos());
    }
    decode_timings.sort_unstable();
    let decode_p50 = decode_timings[decode_timings.len() / 2];
    // Print for human inspection; bench harness would parse.
    println!(
        "size={} p50={}ns p99={}ns decode_p50={}ns",
        payload_len, p50, p99, decode_p50
    );
    if payload_len <= 4096 {
        assert!(
            p50 < 50_000,
            "p50 {}ns exceeds 50µs for payload {}",
            p50,
            payload_len
        );
    }
    Ok((p50, decode_p50))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("narrivy-ipc framing bench — budget p50 < 50µs per op");
    println!("zero-copy: encode single-alloc (4+payload), read length-check before alloc");
    let sizes = [32, 256, 1024, 8192, 65536];
    for &size in &sizes {
        if size > MAX_FRAME_PAYLOAD_BYTES {
            continue;
        }
        let (p50, decode_p50) = bench_one(size, 10_000)?;
        println!(
            "  size {:>6}: encode+decode p50 {:>6}ns  decode p50 {:>6}ns",
            size, p50, decode_p50
        );
    }
    // One near-bounds check (1 MiB) with fewer iterations to stay fast — informational,
    // budget applies only to typical control messages (<4k).
    let (p50_big, _) = bench_one(1_048_576 - 1024, 2_000)?;
    println!(
        "  size 1MiB-1k: p50 {}ns (informational, >50µs expected for 1MiB)",
        p50_big
    );
    println!("bench OK — all p50 < 50µs for small control messages (32..4096)");
    Ok(())
}
