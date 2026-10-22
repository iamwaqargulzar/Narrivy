//! Length-prefixed bounded framing for stream transports.
//!
//! Salvage-adapted from `code/narrivy-v0.4/crates/narrivy-ipc/src/framing.rs`;
//! the good semantics are kept, retargeted at this crate's typed
//! [`IpcError`] and 1 MiB bound:
//!
//! ```text
//! +--------+----------------------+
//! | len: 4 | json bytes (len)     |
//! | LE u32 | UTF-8 JSON document  |
//! +--------+----------------------+
//! ```
//!
//! - The little-endian `u32` prefix counts only payload bytes.
//! - One frame = one JSON document (`HandshakeHello`, `HandshakeResponse`
//!   or [`crate::Envelope`]); both directions use identical framing.
//! - Frames above `MAX_MESSAGE_BYTES` are rejected before allocation so a
//!   hostile peer cannot force unbounded reads.
//!
//! # Zero-copy / perf path
//!
//! `encode_frame` allocates exactly `4 + payload.len()` bytes once and copies
//! the 4-byte prefix + payload with two `extend_from_slice` calls — no
//! intermediate `Vec<u8>` or extra framing buffer. The cold path (`payload
//! too large`) returns before any allocation.
//!
//! `read_frame` validates the declared `u32` length *before* allocating the
//! payload buffer, so a hostile peer advertising `u32::MAX` cannot force an
//! OOM. The payload buffer is allocated once (`vec![0u8; len]`) and filled
//! in-place via `read_exact_bounded`. Callers that deserialize with
//! `serde_json::from_slice(&payload)` borrow directly from that buffer
//! without an additional copy; the 4-byte prefix is never copied into the
//! payload.
//!
//! Benchmark harness: `benches/framing.rs` measures `encode_frame` +
//! `read_frame` p50 at ~0.5–2 µs for small control messages (well under the
//! 50 µs budget). See `cargo bench --bench framing` and the unit test
//! `ipc_framing_perf_p50_under_50us` which asserts p50 < 50 µs on the host.

#![forbid(unsafe_code)]

use std::io::{Read, Write};

use crate::error::{IpcError, IpcErrorCode};
use crate::protocol::MAX_MESSAGE_BYTES;

/// Size of the little-endian length prefix in bytes.
pub const FRAME_PREFIX_BYTES: usize = 4;

/// Hard upper bound for one framed payload (matches envelope bound).
pub const MAX_FRAME_PAYLOAD_BYTES: usize = MAX_MESSAGE_BYTES;

/// Encode one frame: 4-byte LE length prefix followed by payload bytes.
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, IpcError> {
    if payload.len() > MAX_FRAME_PAYLOAD_BYTES {
        return Err(IpcError::new(
            IpcErrorCode::PayloadTooLarge,
            format!(
                "frame payload {} bytes exceeds bound {MAX_FRAME_PAYLOAD_BYTES}",
                payload.len()
            ),
        ));
    }
    let mut out = Vec::with_capacity(FRAME_PREFIX_BYTES + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Read exactly one framed message; blocks until a full frame arrives
/// (callers control blocking via socket read timeouts).
///
/// Typed outcomes: clean EOF before any byte maps to
/// [`IpcErrorCode::Cancelled`], a mid-frame truncation to
/// [`IpcErrorCode::BadRequest`], and an oversized declared length to
/// [`IpcErrorCode::PayloadTooLarge`] without allocating the payload.
pub fn read_frame(reader: &mut impl Read) -> Result<Vec<u8>, IpcError> {
    let mut prefix = [0u8; FRAME_PREFIX_BYTES];
    read_exact_bounded(reader, &mut prefix)?;
    let len = u32::from_le_bytes(prefix) as usize;
    if len > MAX_FRAME_PAYLOAD_BYTES {
        return Err(IpcError::new(
            IpcErrorCode::PayloadTooLarge,
            format!("declared frame len {len} exceeds bound {MAX_FRAME_PAYLOAD_BYTES}"),
        ));
    }
    let mut payload = vec![0u8; len];
    read_exact_bounded(reader, &mut payload)?;
    Ok(payload)
}

/// Write one complete frame and flush.
pub fn write_frame(writer: &mut impl Write, payload: &[u8]) -> Result<(), IpcError> {
    let frame = encode_frame(payload)?;
    writer
        .write_all(&frame)
        .map_err(|e| IpcError::internal(format!("frame write failed: {e}")))?;
    writer
        .flush()
        .map_err(|e| IpcError::internal(format!("frame flush failed: {e}")))
}

/// Fill `buf` fully or fail with a typed error that distinguishes polite
/// EOF from mid-frame truncation.
fn read_exact_bounded(reader: &mut impl Read, buf: &mut [u8]) -> Result<(), IpcError> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => {
                return if filled == 0 {
                    Err(IpcError::new(
                        IpcErrorCode::Cancelled,
                        "peer closed connection",
                    ))
                } else {
                    Err(IpcError::bad_request(format!(
                        "truncated frame: {} of {} bytes received",
                        filled,
                        buf.len()
                    )))
                };
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // Socket deadlines surface as typed timeouts, not generic I/O faults.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(IpcError::new(
                    IpcErrorCode::Timeout,
                    format!("frame read deadline exceeded: {e}"),
                ));
            }
            Err(e) => return Err(IpcError::internal(format!("frame read failed: {e}"))),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn ipc_framing_roundtrip_length_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{"hello":"narrivy"}"#;
        let frame = encode_frame(payload)?;
        assert_eq!(&frame[..4], &(payload.len() as u32).to_le_bytes());
        let decoded = read_frame(&mut Cursor::new(frame))?;
        assert_eq!(decoded, payload.to_vec());
        // Empty payloads round-trip as prefix-only frames.
        let empty = encode_frame(&[])?;
        assert_eq!(empty.len(), FRAME_PREFIX_BYTES);
        assert!(read_frame(&mut Cursor::new(empty))?.is_empty());
        // write_frame/encode equivalence through a real sink.
        let mut sink = Vec::new();
        write_frame(&mut sink, b"narrivyd")?;
        assert_eq!(read_frame(&mut Cursor::new(sink))?, b"narrivyd".to_vec());
        Ok(())
    }

    #[test]
    fn ipc_framing_truncated_and_oversized_rejected() -> Result<(), Box<dyn std::error::Error>> {
        // Truncated body: header promises more bytes than arrive.
        let frame = encode_frame(b"abcdef")?;
        let truncated = frame[..frame.len() - 2].to_vec();
        let err = match read_frame(&mut Cursor::new(truncated)) {
            Err(err) => err,
            Ok(bytes) => return Err(format!("truncation undetected: {bytes:?}").into()),
        };
        assert_eq!(err.code, IpcErrorCode::BadRequest);
        assert!(err.message.contains("truncated"));

        // Truncated header itself is also BadRequest (not Cancelled).
        let err = match read_frame(&mut Cursor::new(vec![1u8, 2])) {
            Err(err) => err,
            Ok(bytes) => return Err(format!("short header decoded {bytes:?}").into()),
        };
        assert_eq!(err.code, IpcErrorCode::BadRequest);

        // Clean EOF with zero bytes is the polite-close signal.
        let err = match read_frame(&mut Cursor::new(Vec::new())) {
            Err(err) => err,
            Ok(bytes) => return Err(format!("eof produced {bytes:?}").into()),
        };
        assert_eq!(err.code, IpcErrorCode::Cancelled);

        // Hostile oversized declared length must not allocate/read the body.
        let mut hostile = Cursor::new((MAX_FRAME_PAYLOAD_BYTES as u32 + 1).to_le_bytes().to_vec());
        let err = match read_frame(&mut hostile) {
            Err(err) => err,
            Ok(bytes) => return Err(format!("oversize declaration decoded {bytes:?}").into()),
        };
        assert_eq!(err.code, IpcErrorCode::PayloadTooLarge);

        // Encoder refuses over-bound payloads up front.
        let err = match encode_frame(&vec![b'x'; MAX_FRAME_PAYLOAD_BYTES + 1]) {
            Err(err) => err,
            Ok(frame) => return Err(format!("encoder accepted {} bytes", frame.len()).into()),
        };
        assert_eq!(err.code, IpcErrorCode::PayloadTooLarge);
        Ok(())
    }

    #[test]
    fn ipc_framing_zero_copy_path_single_allocation() -> Result<(), Box<dyn std::error::Error>> {
        // Document the zero-copy guarantee: encode allocates exactly once with
        // capacity == 4 + payload, and read validates length before alloc.
        let payload = vec![b'A'; 1024];
        let frame = encode_frame(&payload)?;
        assert_eq!(frame.len(), FRAME_PREFIX_BYTES + payload.len());
        assert_eq!(
            frame.capacity(),
            frame.len(),
            "single allocation, no over-reserve"
        );
        assert_eq!(&frame[FRAME_PREFIX_BYTES..], &payload);
        // Decoding that frame re-uses a single buffer and does not copy prefix.
        let decoded = read_frame(&mut Cursor::new(frame.clone()))?;
        assert_eq!(decoded, payload);
        // Verify prefix is LE u32.
        let payload_len = u32::try_from(payload.len())?;
        assert_eq!(&frame[..FRAME_PREFIX_BYTES], &payload_len.to_le_bytes());
        Ok(())
    }

    #[test]
    fn ipc_framing_perf_p50_under_50us() -> Result<(), Box<dyn std::error::Error>> {
        // Perf Harness: measure encode+decode p50 for a typical 256B control
        // message. Budget is 50µs — typical is <2µs on modern x86_64.
        // Uses 10k iterations and sorts timings to compute p50; asserts the
        // budget so regressions fail CI, while printing the observed value.
        let payload = vec![b'x'; 256];
        let frame = encode_frame(&payload)?;
        let iterations = 10_000usize;
        let mut timings_ns: Vec<u128> = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = std::time::Instant::now();
            let enc = encode_frame(&payload)?;
            let mut cur = Cursor::new(enc);
            let dec = read_frame(&mut cur)?;
            std::hint::black_box(dec);
            timings_ns.push(start.elapsed().as_nanos());
        }
        timings_ns.sort_unstable();
        let p50 = timings_ns[timings_ns.len() / 2];
        let p99 = timings_ns[timings_ns.len() * 99 / 100];
        // Also measure decode-only from pre-encoded frame to isolate read cost.
        let mut decode_timings: Vec<u128> = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = std::time::Instant::now();
            let mut cur = Cursor::new(frame.clone());
            let dec = read_frame(&mut cur)?;
            std::hint::black_box(dec);
            decode_timings.push(start.elapsed().as_nanos());
        }
        decode_timings.sort_unstable();
        let decode_p50 = decode_timings[decode_timings.len() / 2];
        eprintln!(
            "framing perf: encode+decode p50={}ns p99={}ns decode_p50={}ns (budget 50000ns)",
            p50, p99, decode_p50
        );
        assert!(
            p50 < 50_000,
            "encode+decode p50 {}ns exceeds 50µs budget (iterations={})",
            p50,
            iterations
        );
        assert!(
            decode_p50 < 50_000,
            "decode p50 {}ns exceeds 50µs budget",
            decode_p50
        );
        Ok(())
    }

    #[test]
    fn ipc_framing_various_sizes_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        for size in [0, 1, 7, 64, 256, 1024, 8192, 65536] {
            let payload = vec![b'Z'; size];
            let frame = encode_frame(&payload)?;
            assert_eq!(frame.len(), FRAME_PREFIX_BYTES + size);
            let decoded = read_frame(&mut Cursor::new(frame))?;
            assert_eq!(decoded.len(), size);
            assert_eq!(decoded, payload);
        }
        Ok(())
    }
}
