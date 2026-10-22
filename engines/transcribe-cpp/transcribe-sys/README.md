# transcribe-sys — UNVERIFIED FFI stub for transcribe.cpp

Hand-authored `extern "C"` declarations (hypothesized API) + safe wrappers.
NO upstream code is vendored here. Links only when `TRANSCRIBE_CPP_ROOT` is
set AND the parent crate enables its `runtime` feature. License review must
pass first: see `../BUILD-NOTES.md`.

Lint note: this crate uses a LOCAL lint table (not workspace inheritance)
because it isolates the unavoidable FFI `unsafe` for the whole binding, per
AGENTS.md. All other lint entries mirror the workspace policy.
