//! Peer identity, endpoint permission rules and stale-path safety rules
//! implementing ADR-0011 ("same-user local IPC is a security boundary") and
//! the stale-socket rules of ADR-0012.
//!
//! This module is one of two in the crate allowed to contain `unsafe`
//! (see crate docs). Each FFI site below is cfg-gated, minimal, and carries
//! a SAFETY comment stating why it cannot break memory safety.

use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

use serde::{Deserialize, Serialize};

use narrivy_ipc::error::{IpcError, IpcErrorCode};

/// OS-verified identity of the process at the other end of a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerCredentials {
    pub uid: u32,
    pub gid: u32,
    /// Present only when the platform reports it (Linux `SO_PEERCRED`).
    pub pid: Option<u32>,
}

impl PeerCredentials {
    pub fn is_same_user(&self, expected_uid: u32) -> bool {
        self.uid == expected_uid
    }

    /// Same-user authorization check (ADR-0011). Must pass before any
    /// privileged protocol handling.
    pub fn validate(&self, expected_uid: u32) -> Result<(), IpcError> {
        if self.uid != expected_uid {
            return Err(IpcError::permission_denied(format!(
                "peer uid {} does not match endpoint owner uid {expected_uid}",
                self.uid
            )));
        }
        Ok(())
    }
}

/// Read the kernel-verified credentials of the connected peer.
///
/// Linux uses `getsockopt(SOL_SOCKET, SO_PEERCRED)` which yields uid, gid
/// and pid. Other unixes use `getpeereid()` (uid+gid, no pid). Non-unix
/// targets report an unsupported error — callers must fail closed.
#[cfg(target_os = "linux")]
pub fn peer_credentials(stream: &UnixStream) -> Result<PeerCredentials, IpcError> {
    let fd = stream.as_raw_fd();
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `fd` is a live connected unix-socket descriptor owned by
    // `stream`; `cred` is a valid, correctly aligned `ucred` and `len` its
    // exact size, satisfying the getsockopt contract for SO_PEERCRED. The
    // call only reads kernel state into our buffer and returns -1 on error;
    // it cannot invalidate any Rust reference.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(cred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(IpcError::new(
            IpcErrorCode::PermissionDenied,
            format!("SO_PEERCRED failed: {err}"),
        ));
    }
    Ok(PeerCredentials {
        uid: cred.uid,
        gid: cred.gid,
        pid: Some(cred.pid.max(0) as u32),
    })
}

/// Non-Linux unix fallback via `getpeereid` (uid + gid; no pid reported).
#[cfg(all(unix, not(target_os = "linux")))]
pub fn peer_credentials(stream: &UnixStream) -> Result<PeerCredentials, IpcError> {
    let fd = stream.as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `fd` is a live connected unix-socket descriptor owned by
    // `stream`; both pointers are valid, writable `uid_t`/`gid_t` slots as
    // getpeereid requires. The syscall only writes those two slots.
    let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(IpcError::new(
            IpcErrorCode::PermissionDenied,
            format!("getpeereid failed: {err}"),
        ));
    }
    Ok(PeerCredentials {
        uid,
        gid,
        pid: None,
    })
}

/// Platform note: this function exists only on unix targets (`#[cfg(unix)]`
/// via its two platform bodies). Non-unix transports must fail closed and
/// never claim same-user authorization without kernel-verified identity.
/// Effective uid of this process (`geteuid`). Used to derive the expected
/// endpoint owner without environment trust.
#[cfg(unix)]
pub fn current_uid() -> u32 {
    // SAFETY: geteuid is an async-signal-safe libc query over process state;
    // it takes no pointers and cannot fail or mutate memory.
    unsafe { libc::geteuid() }
}

/// Non-unix placeholder: no meaningful uid exists for socket ownership.
#[cfg(not(unix))]
pub fn current_uid() -> u32 {
    0
}

/// Filesystem permission facts about the IPC endpoint (ADR-0011/0012).
///
/// Secure state: directory mode `0700`, socket mode `0600`, owner equal to
/// the effective user. Insecure state must be rejected, never broadened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointPermissions {
    pub dir_mode: u32,
    pub sock_mode: u32,
    pub owner_uid: u32,
}

impl EndpointPermissions {
    /// True iff dir is 0700 and socket is 0600 (mode bits only).
    pub fn is_secure(&self) -> bool {
        (self.dir_mode & 0o777) == 0o700 && (self.sock_mode & 0o777) == 0o600
    }

    /// Owner match plus secure modes, else [`IpcErrorCode::PermissionDenied`].
    pub fn validate(&self, expected_uid: u32) -> Result<(), IpcError> {
        if self.owner_uid != expected_uid {
            return Err(IpcError::permission_denied(format!(
                "endpoint owner uid {} does not match expected {expected_uid}",
                self.owner_uid
            )));
        }
        if !self.is_secure() {
            return Err(IpcError::permission_denied(format!(
                "insecure endpoint modes: dir {:o} sock {:o} (want 700/600)",
                self.dir_mode & 0o777,
                self.sock_mode & 0o777
            )));
        }
        Ok(())
    }
}

/// Decision inputs for removing one possibly-stale daemon socket file
/// (ADR-0012 "Socket lifecycle").
///
/// All fields come from `lstat`-class metadata: symlinks are never followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketPathValidation {
    /// Canonical path the daemon owns (e.g. `$XDG_RUNTIME_DIR/narrivy/narrivyd.sock`).
    pub expected_path: std::path::PathBuf,
    /// Path actually inspected on disk.
    pub actual_path: std::path::PathBuf,
    pub is_symlink: bool,
    pub is_socket: bool,
    pub owner_uid: u32,
    pub expected_uid: u32,
}

impl SocketPathValidation {
    /// Remove-on-bind decision. Conservative conjunction:
    ///
    /// - exact path identity (`actual == expected`, never a sibling),
    /// - never a symlink,
    /// - must actually be a socket file type,
    /// - owner uid must match the effective uid.
    ///
    /// Anything else refuses removal so cross-user races, path replacement
    /// and symlink attacks cannot make the daemon delete foreign files.
    pub fn should_remove_stale(&self) -> bool {
        self.actual_path == self.expected_path
            && !self.is_symlink
            && self.is_socket
            && self.owner_uid == self.expected_uid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_peer_credentials_same_user_validate_ok() {
        let creds = PeerCredentials {
            uid: 1000,
            gid: 1000,
            pid: Some(4242),
        };
        assert!(creds.is_same_user(1000));
        assert!(creds.validate(1000).is_ok());
        // No pid still validates: pid is informational, uid decides.
        let anonymous = PeerCredentials {
            uid: 1000,
            gid: 1000,
            pid: None,
        };
        assert!(anonymous.validate(1000).is_ok());
    }

    #[test]
    fn ipc_peer_credentials_mismatch_denied() -> Result<(), Box<dyn std::error::Error>> {
        let creds = PeerCredentials {
            uid: 1001,
            gid: 1001,
            pid: Some(7),
        };
        assert!(!creds.is_same_user(1000));
        let err = match creds.validate(1000) {
            Err(err) => err,
            Ok(()) => return Err("foreign uid must be denied".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        assert!(err.to_string().contains("1001"));
        Ok(())
    }

    #[test]
    fn ipc_endpoint_permissions_secure_and_insecure_matrix()
    -> Result<(), Box<dyn std::error::Error>> {
        let secure = EndpointPermissions {
            dir_mode: 0o700,
            sock_mode: 0o600,
            owner_uid: 1000,
        };
        assert!(secure.is_secure());
        assert!(secure.validate(1000).is_ok());

        for (dir_mode, sock_mode) in [(0o755, 0o600), (0o700, 0o644), (0o777, 0o666)] {
            let loose = EndpointPermissions {
                dir_mode,
                sock_mode,
                owner_uid: 1000,
            };
            assert!(
                !loose.is_secure(),
                "{dir_mode:o}/{sock_mode:o} must be insecure"
            );
            assert!(loose.validate(1000).is_err());
        }
        let foreign = EndpointPermissions {
            owner_uid: 1001,
            ..secure.clone()
        };
        let err = match foreign.validate(1000) {
            Err(err) => err,
            Ok(()) => return Err("owner mismatch must be denied".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        Ok(())
    }

    #[test]
    fn ipc_stale_socket_decision_matrix() {
        let base = SocketPathValidation {
            expected_path: std::path::PathBuf::from("/run/user/1000/narrivy/narrivyd.sock"),
            actual_path: std::path::PathBuf::from("/run/user/1000/narrivy/narrivyd.sock"),
            is_symlink: false,
            is_socket: true,
            owner_uid: 1000,
            expected_uid: 1000,
        };
        // Exactly-stale socket owned by us: removable.
        assert!(base.should_remove_stale());

        // Different path than the canonical one: never remove.
        let moved = SocketPathValidation {
            actual_path: std::path::PathBuf::from("/tmp/evil.sock"),
            ..base.clone()
        };
        assert!(!moved.should_remove_stale());

        // Symlink (even pointing at the right place): never follow/remove.
        let symlinked = SocketPathValidation {
            is_symlink: true,
            ..base.clone()
        };
        assert!(!symlinked.should_remove_stale());

        // Regular file squatting on the path: refuse.
        let not_a_socket = SocketPathValidation {
            is_socket: false,
            ..base.clone()
        };
        assert!(!not_a_socket.should_remove_stale());

        // Another user's leftover: refuse.
        let foreign = SocketPathValidation {
            owner_uid: 1001,
            ..base.clone()
        };
        assert!(!foreign.should_remove_stale());

        // Mismatched expected_uid input: refuse.
        let bad_expectation = SocketPathValidation {
            expected_uid: 1001,
            ..base
        };
        assert!(!bad_expectation.should_remove_stale());
    }

    /// Real kernel round-trip of the peer-credential FFI over an actual
    /// unix socket pair (same process ⇒ same user on both ends).
    #[cfg(unix)]
    #[test]
    fn ipc_peer_credentials_real_socket_reports_self_user() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("creds.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        let client_stream = UnixStream::connect(&path)?;
        let (accepted, _) = listener.accept()?;
        let euid = current_uid();

        // What the server sees looking at the client...
        let server_view = peer_credentials(&accepted)?;
        assert!(
            server_view.validate(euid).is_ok(),
            "server must see own user"
        );
        // ...and what a client sees looking at the server.
        let client_view = peer_credentials(&client_stream)?;
        assert!(client_view.is_same_user(euid));
        Ok(())
    }
}
