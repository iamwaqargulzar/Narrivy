//! Daemon transport seam (PORT-001, PORT-007).
//!
//! Unix builds use the authenticated Unix-socket adapter; Windows builds use
//! the authenticated per-user named-pipe adapter. Both expose the same client
//! API, so callers import from here and never name an OS transport.

#[cfg(unix)]
pub use narrivy_ipc_unix::{IpcClient, default_socket_path};

#[cfg(windows)]
pub use narrivy_ipc_windows::{IpcClient, default_socket_path};
