//! Authenticated per-user named-pipe transport for Narrivy's portable IPC
//! protocol on Windows.
//!
//! [`narrivy_ipc`] owns the stable wire envelope, framing and typed errors.
//! This adapter alone owns named pipes, endpoint names, their security
//! descriptors, peer process identity, connection workers and their
//! shutdown. Its public API mirrors `narrivy-ipc-unix` so the daemon and its
//! clients choose a transport with `cfg` alone (see the crate README).
//!
//! Identity rules ([`security`]) and endpoint naming are portable and tested
//! on every host; the transport itself exists only on Windows. `unsafe` is
//! confined to the private `sys` module.

pub mod security;

#[cfg(windows)]
pub mod client;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
pub mod server;
#[cfg(windows)]
mod sys;

pub use security::{MAX_SID_BYTES, PeerCredentials, UserSid, endpoint_sddl};

#[cfg(windows)]
pub use client::{IpcClient, SequencedEvent};
#[cfg(windows)]
pub use server::{IpcHandler, WindowsIpcServer};
#[cfg(windows)]
pub use sys::current_user_sid;

/// The Windows transport's server type under a transport-neutral name.
#[cfg(windows)]
pub type IpcServer = WindowsIpcServer;

/// Namespace every local named pipe lives in.
pub const PIPE_NAMESPACE: &str = r"\\.\pipe\";

/// Longest pipe path Windows accepts (`\\.\pipe\` plus the name).
pub const MAX_PIPE_PATH_CHARS: usize = 256;

/// Canonical daemon pipe name for one user. Including the SID keeps users
/// from colliding with each other; it is not the security boundary (the
/// owner-only DACL, first-instance creation and peer SID checks are).
#[must_use]
pub fn pipe_name_for_sid(sid: &UserSid) -> String {
    format!("{PIPE_NAMESPACE}narrivy-{sid}-narrivyd")
}

/// Accept only local pipe paths of a length Windows can create. Remote
/// (`\\host\pipe\...`) and non-pipe paths are refused before any OS call.
pub fn validate_pipe_path(path: &std::path::Path) -> Result<&str, narrivy_ipc::IpcError> {
    let text = path
        .to_str()
        .ok_or_else(|| narrivy_ipc::IpcError::bad_request("pipe path is not valid Unicode"))?;
    let name = text.strip_prefix(PIPE_NAMESPACE).ok_or_else(|| {
        narrivy_ipc::IpcError::bad_request("pipe path must start with \\\\.\\pipe\\")
    })?;
    if name.is_empty() || text.chars().count() > MAX_PIPE_PATH_CHARS || text.contains('\0') {
        return Err(narrivy_ipc::IpcError::bad_request(
            "pipe name is empty, too long or contains NUL",
        ));
    }
    Ok(text)
}

/// Canonical daemon endpoint for the current user.
///
/// Infallible like the Unix transport's `default_socket_path`. If the
/// process token's SID cannot be read the returned name contains no SID;
/// that cannot weaken anything, because binding and connecting both need
/// that same SID and fail closed without it.
#[cfg(windows)]
#[must_use]
pub fn default_socket_path() -> std::path::PathBuf {
    let name = match current_user_sid() {
        Ok(sid) => pipe_name_for_sid(&sid),
        Err(_) => format!("{PIPE_NAMESPACE}narrivy-unidentified-narrivyd"),
    };
    std::path::PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn pipe_name_embeds_the_user_sid() -> Result<(), narrivy_ipc::IpcError> {
        let sid = UserSid::parse("S-1-5-21-1-2-3-1001")?;
        let name = pipe_name_for_sid(&sid);
        assert_eq!(name, r"\\.\pipe\narrivy-S-1-5-21-1-2-3-1001-narrivyd");
        assert_eq!(validate_pipe_path(Path::new(&name))?, name);
        Ok(())
    }

    #[test]
    fn longest_sid_still_gives_a_valid_pipe_name() -> Result<(), narrivy_ipc::IpcError> {
        let longest = format!("S-1-0xFFFFFFFFFFFF{}", "-4294967295".repeat(15));
        let name = pipe_name_for_sid(&UserSid::parse(&longest)?);
        validate_pipe_path(Path::new(&name))?;
        Ok(())
    }

    #[test]
    fn non_local_or_malformed_pipe_paths_are_rejected() {
        for path in [
            r"\\.\pipe\",
            r"\\server\pipe\narrivy",
            r"C:\narrivy\narrivyd.sock",
            "/run/user/1000/narrivy/narrivyd.sock",
            "\\\\.\\pipe\\nul\0byte",
        ] {
            assert!(
                validate_pipe_path(Path::new(path)).is_err(),
                "{path:?} must be rejected"
            );
        }
        let too_long = format!(r"\\.\pipe\{}", "a".repeat(MAX_PIPE_PATH_CHARS));
        assert!(validate_pipe_path(Path::new(&too_long)).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn default_endpoint_uses_the_process_token_sid() -> Result<(), narrivy_ipc::IpcError> {
        let sid = current_user_sid()?;
        assert_eq!(
            default_socket_path(),
            std::path::PathBuf::from(pipe_name_for_sid(&sid))
        );
        Ok(())
    }
}
