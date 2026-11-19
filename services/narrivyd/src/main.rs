// narrivyd entrypoint. The daemon composition (`src/daemon/`) is shared; each
// OS wires it to its own adapters (chosen in `daemon::host`): Linux in
// `src/linux_main.rs`, Windows in `src/windows_main.rs` (PORT-007). Other
// targets get an entrypoint that refuses to start with a typed exit code.
//
// `include!` keeps the wiring at this file's module level, so the
// integration tests that load `src/main.rs` as a module still see it there.

#[cfg(any(target_os = "linux", windows))]
pub mod daemon;

#[cfg(target_os = "linux")]
include!("linux_main.rs");

#[cfg(windows)]
include!("windows_main.rs");

/// Exit status when this build has no daemon composition for the host OS.
#[cfg(not(any(target_os = "linux", windows)))]
const EXIT_UNSUPPORTED_PLATFORM: u8 = 3;

#[cfg(not(any(target_os = "linux", windows)))]
fn main() -> std::process::ExitCode {
    eprintln!("narrivyd: this build has no daemon composition for this operating system yet");
    std::process::ExitCode::from(EXIT_UNSUPPORTED_PLATFORM)
}
