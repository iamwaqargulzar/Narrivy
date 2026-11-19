//! Authenticated Unix-domain transport for Narrivy's portable IPC protocol.
//!
//! [`narrivy_ipc`] owns the stable wire envelope, framing and typed errors.
//! This adapter alone owns Unix sockets, endpoint paths and modes, peer
//! credentials, endpoint locking, connection workers and their shutdown.

#![cfg(unix)]

pub mod client;
pub mod security;
pub mod server;

pub use client::{IpcClient, SequencedEvent};
pub use security::{
    EndpointPermissions, PeerCredentials, SocketPathValidation, current_uid, peer_credentials,
};
pub use server::{IpcHandler, UnixIpcServer};

/// Canonical daemon socket subpath under the user runtime directory.
pub const SOCKET_SUBPATH: &str = "narrivy/narrivyd.sock";

/// Canonical daemon endpoint, rooted in the effective user's runtime area.
#[must_use]
pub fn default_socket_path() -> std::path::PathBuf {
    let runtime_dir = match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
        _ => std::path::PathBuf::from("/run/user").join(current_uid().to_string()),
    };
    runtime_dir.join(SOCKET_SUBPATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_socket_path_follows_xdg_rule() {
        let path = default_socket_path();
        assert!(path.ends_with(std::path::Path::new(SOCKET_SUBPATH)));
        match std::env::var("XDG_RUNTIME_DIR") {
            Ok(dir) if !dir.is_empty() => assert!(path.starts_with(dir)),
            _ => assert_eq!(
                path,
                std::path::PathBuf::from(format!(
                    "/run/user/{}/narrivy/narrivyd.sock",
                    current_uid()
                ))
            ),
        }
    }
}
