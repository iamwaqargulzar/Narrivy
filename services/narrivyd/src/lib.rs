//! narrivyd library surface: deterministic correction + daemon internals.
#![forbid(unsafe_code)]

pub mod breaks;
pub mod bus;
pub mod correct;
#[cfg(target_os = "linux")]
pub mod instance;
pub mod numbers;
