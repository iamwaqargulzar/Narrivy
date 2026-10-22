//! Raw + safe FFI skeleton for the transcribe.cpp public C API.
//!
//! # LICENSE: UNVERIFIED — READ BEFORE ANY REAL USE
//!
//! The upstream project (handy-computer org, announced 2026-06-30) has NOT
//! passed our license review. Until it does:
//!
//! - NO upstream source, header, or binary may be vendored, copied, or
//!   committed into this repository.
//! - This crate contains ONLY hand-authored `extern "C"` declarations for a
//!   MINIMAL HYPOTHESIZED slice of the public C API. Every declaration is
//!   **UNVERIFIED-UNTIL-LICENSE-REVIEW and UNVERIFIED-UNTIL-UPSTREAM-API-
//!   CONFIRMATION**; names, signatures, return-code conventions (0 == success)
//!   and lifetime rules below are plausible guesses, not observed facts.
//! - Nothing links unless the parent crate's `runtime` feature is enabled AND
//!   `TRANSCRIBE_CPP_ROOT` points at an operator-provided built library
//!   (`include/` + `lib/libtranscribecpp.{a,so}`). See ../BUILD-NOTES.md.
//!
//! # Safety architecture
//!
//! This crate is the ONLY place in the binding allowed to contain `unsafe`
//! (FFI isolation per AGENTS.md). It exposes a small SAFE surface
//! ([`TcppModel`], [`TcppContext`], [`TcppStatus`]); the parent crate never
//! writes `unsafe` itself.

use std::ffi::{CStr, CString, c_char, c_int};
use std::fmt;
use std::path::Path;
use std::ptr::NonNull;

/// Hypothesized opaque model handle; layout owned entirely by upstream.
#[repr(C)]
pub struct tcpp_model {
    _private: [u8; 0],
}

/// Hypothesized per-stream decode context; layout owned entirely by upstream.
#[repr(C)]
pub struct tcpp_context {
    _private: [u8; 0],
}

// --- HYPOTHESIZED PUBLIC C API SURFACE (UNVERIFIED) -------------------------
//
// Conventions assumed until upstream confirmation:
// - `c_int` returns: 0 == success, non-zero == failure.
// - Out-parameters are written only on success.
// - `tcpp_last_error_message()` returns a NUL-terminated message valid until
//   the next call on the same thread (typical errno-style contract).
unsafe extern "C" {
    /// Hypothesis: load a GGUF model file into an opaque handle.
    pub(crate) fn tcpp_model_load_gguf(
        path: *const c_char,
        out_model: *mut *mut tcpp_model,
    ) -> c_int;

    /// Hypothesis: free a model handle (null tolerated).
    pub(crate) fn tcpp_model_free(model: *mut tcpp_model);

    /// Hypothesis: create a decode context bound to a loaded model.
    pub(crate) fn tcpp_context_create(
        model: *const tcpp_model,
        out_ctx: *mut *mut tcpp_context,
    ) -> c_int;

    /// Hypothesis: free a decode context (null tolerated).
    pub(crate) fn tcpp_context_free(ctx: *mut tcpp_context);

    /// Hypothesis: queue 16 kHz mono f32 samples in [-1, 1] for decoding.
    pub(crate) fn tcpp_context_set_samples_f32(
        ctx: *mut tcpp_context,
        samples: *const f32,
        n_samples: usize,
    ) -> c_int;

    /// Hypothesis: run (blocking) transcription over the queued samples.
    pub(crate) fn tcpp_context_run_decode(ctx: *mut tcpp_context) -> c_int;

    /// Hypothesis: number of text segments produced by the last decode.
    pub(crate) fn tcpp_context_segment_count(ctx: *const tcpp_context) -> c_int;

    /// Hypothesis: borrow the UTF-8 text of segment `index` from the last
    /// decode; valid until the context is freed or re-decoded.
    pub(crate) fn tcpp_context_segment_text(
        ctx: *const tcpp_context,
        index: c_int,
    ) -> *const c_char;

    /// Hypothesis: thread-local message for the most recent failure.
    pub(crate) fn tcpp_last_error_message() -> *const c_char;
}
// --- END HYPOTHESIZED SURFACE ----------------------------------------------

/// Failure of a hypothesized upstream call, with the best available message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcppStatus {
    pub code: c_int,
    pub message: String,
}

impl TcppStatus {
    fn from_call(call_site: &'static str, code: c_int) -> Self {
        let message = unsafe {
            let ptr = tcpp_last_error_message();
            if ptr.is_null() {
                format!("{call_site} failed with code {code} (no upstream error message)")
            } else {
                let text = CStr::from_ptr(ptr).to_string_lossy().into_owned();
                if text.is_empty() {
                    format!("{call_site} failed with code {code}")
                } else {
                    format!("{call_site} failed with code {code}: {text}")
                }
            }
        };
        Self { code, message }
    }
}

impl fmt::Display for TcppStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TcppStatus {}

fn path_cstring(path: &Path) -> Result<CString, TcppStatus> {
    CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| TcppStatus {
        code: -1,
        message: format!(
            "model path contains an interior NUL byte: {}",
            path.display()
        ),
    })
}

/// Owned handle to a loaded GGUF model. Freed exactly once on drop.
///
/// # Invariants (documented, upstream-unconfirmed)
///
/// - The pointer originates from `tcpp_model_load_gguf` and is freed by
///   `tcpp_model_free` exactly once; the type is neither `Clone` nor `Copy`.
/// - Assumed immutable after load. `Send`/`Sync` are declared so one loaded
///   model can back several streams; concurrent context creation against one
///   model is ASSUMED safe and must be revisited at P5 once the upstream
///   threading contract is confirmed (see ../BUILD-NOTES.md).
#[derive(Debug)]
pub struct TcppModel(NonNull<tcpp_model>);

// SAFETY: see type-level invariant documentation above; both impls are
// assumptions about upstream behavior flagged for P5 confirmation.
unsafe impl Send for TcppModel {}
unsafe impl Sync for TcppModel {}

impl TcppModel {
    /// Loads a GGUF model from `path` via the hypothesized `tcpp_model_load_gguf`.
    pub fn load_gguf(path: &Path) -> Result<Self, TcppStatus> {
        let cpath = path_cstring(path)?;
        let mut ptr: *mut tcpp_model = std::ptr::null_mut();
        // SAFETY: `cpath` outlives the call; `ptr` is a valid out-slot. Upstream
        // owns all pointee semantics; we only wrap a non-null success result.
        let code = unsafe { tcpp_model_load_gguf(cpath.as_ptr(), &mut ptr) };
        if code != 0 || ptr.is_null() {
            return Err(TcppStatus::from_call(
                "tcpp_model_load_gguf",
                if code == 0 { -1 } else { code },
            ));
        }
        // SAFETY: non-null guaranteed by the check above.
        Ok(Self(unsafe { NonNull::new_unchecked(ptr) }))
    }

    fn as_ptr(&self) -> *const tcpp_model {
        self.0.as_ptr()
    }
}

impl Drop for TcppModel {
    fn drop(&mut self) {
        // SAFETY: pointer came from `tcpp_model_load_gguf` and is dropped
        // exactly once here; null was excluded at construction.
        unsafe { tcpp_model_free(self.0.as_ptr()) };
    }
}

/// Owned decode context bound to one [`TcppModel`]. Not `Sync`: decode state
/// is treated as mutable and single-owner until upstream says otherwise.
#[derive(Debug)]
pub struct TcppContext(NonNull<tcpp_context>);

// SAFETY: ownership transfers wholesale with the value; no shared mutable
// access is possible because the type is not `Sync`.
unsafe impl Send for TcppContext {}

impl TcppContext {
    /// Creates a decode context via the hypothesized `tcpp_context_create`.
    pub fn new(model: &TcppModel) -> Result<Self, TcppStatus> {
        let mut ptr: *mut tcpp_context = std::ptr::null_mut();
        // SAFETY: `model.as_ptr()` is a live upstream handle; `ptr` is a valid
        // out-slot within this call.
        let code = unsafe { tcpp_context_create(model.as_ptr(), &mut ptr) };
        if code != 0 || ptr.is_null() {
            return Err(TcppStatus::from_call(
                "tcpp_context_create",
                if code == 0 { -1 } else { code },
            ));
        }
        // SAFETY: non-null guaranteed by the check above.
        Ok(Self(unsafe { NonNull::new_unchecked(ptr) }))
    }

    /// Queues 16 kHz mono f32 samples. An empty slice is accepted as a no-op so
    /// callers do not need a special case before `run_decode`.
    pub fn set_samples_f32(&mut self, samples: &[f32]) -> Result<(), TcppStatus> {
        if samples.is_empty() {
            return Ok(());
        }
        // SAFETY: `samples` points to `samples.len()` initialized f32 values
        // for the duration of the call.
        let code = unsafe {
            tcpp_context_set_samples_f32(self.0.as_ptr(), samples.as_ptr(), samples.len())
        };
        if code == 0 {
            Ok(())
        } else {
            Err(TcppStatus::from_call("tcpp_context_set_samples_f32", code))
        }
    }

    /// Runs blocking transcription over the queued samples.
    pub fn run_decode(&mut self) -> Result<(), TcppStatus> {
        // SAFETY: `self.0` is a live upstream context handle.
        let code = unsafe { tcpp_context_run_decode(self.0.as_ptr()) };
        if code == 0 {
            Ok(())
        } else {
            Err(TcppStatus::from_call("tcpp_context_run_decode", code))
        }
    }

    /// Copies out the text segments produced by the last successful decode.
    pub fn segments(&self) -> Vec<String> {
        // SAFETY: `self.0` is a live handle; each returned char pointer is
        // borrowed only for the duration of its own `CStr` conversion.
        let count = unsafe { tcpp_context_segment_count(self.0.as_ptr()) };
        if count <= 0 {
            return Vec::new();
        }
        (0..count)
            .map(|i| {
                // SAFETY: index is within the count reported by upstream for
                // this same context; pointer is converted before any further
                // upstream call can invalidate it.
                let text_ptr = unsafe { tcpp_context_segment_text(self.0.as_ptr(), i) };
                if text_ptr.is_null() {
                    String::new()
                } else {
                    // SAFETY: hypothesized contract: NUL-terminated UTF-8.
                    unsafe { CStr::from_ptr(text_ptr) }
                        .to_string_lossy()
                        .into_owned()
                }
            })
            .collect()
    }
}

impl Drop for TcppContext {
    fn drop(&mut self) {
        // SAFETY: pointer came from `tcpp_context_create`, dropped exactly once.
        unsafe { tcpp_context_free(self.0.as_ptr()) };
    }
}
