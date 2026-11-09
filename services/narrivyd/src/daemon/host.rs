//! Host adapter selection (PORT-007).
//!
//! The daemon composes one set of OS adapters per target: on Linux the Unix
//! socket, Unix journal, Linux process control and the Linux platform
//! (PipeWire, portal, wtype); on Windows the per-user named pipe, Windows
//! journal, job-object process control and the Windows platform (WASAPI,
//! `SendInput`). Daemon modules that run on both name the adapters through
//! here, so the OS choice is made once. Features that exist on one OS only
//! (the portal, PipeWire echo cancellation, desktop shortcut files, the
//! evdev hold key, OpenVINO) keep naming their Linux adapter and have a
//! Windows stand-in module that reports them unavailable.

#[cfg(target_os = "linux")]
pub use narrivy_ipc_unix::{
    IpcClient, PeerCredentials, UnixIpcServer as IpcServer, default_socket_path,
};
#[cfg(windows)]
pub use narrivy_ipc_windows::{
    IpcClient, PeerCredentials, WindowsIpcServer as IpcServer, default_socket_path,
};

#[cfg(target_os = "linux")]
pub use narrivy_journal_unix::{
    RetainedUnixJournal as RetainedHostJournal, UnixJournal as HostJournal,
};
#[cfg(windows)]
pub use narrivy_journal_windows::{
    RetainedWindowsJournal as RetainedHostJournal, WindowsJournal as HostJournal,
};

#[cfg(target_os = "linux")]
pub use narrivy_process_linux as process;
#[cfg(windows)]
pub use narrivy_process_windows as process;

#[cfg(windows)]
pub use super::linux_features_unavailable as linux_features;
/// Linux-only platform features (portals, PipeWire and wtype helpers, echo
/// cancellation, shortcut files, hold key, Secret Service, sysfs counters).
/// On Windows a stand-in with the same shapes reports each one unavailable.
#[cfg(target_os = "linux")]
pub use narrivy_platform_linux as linux_features;

#[cfg(target_os = "linux")]
pub use narrivy_platform_linux::instance::{StateDirError, runtime_dir, state_dir};
#[cfg(windows)]
pub use narrivy_platform_windows::instance::{StateDirError, runtime_dir, state_dir};

/// The user this daemon serves: the only principal allowed on its endpoint
/// (ADR-0011). A uid on Unix, the token user SID on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserIdentity(Option<Principal>);

#[cfg(target_os = "linux")]
type Principal = u32;
#[cfg(windows)]
type Principal = narrivy_ipc_windows::UserSid;

impl UserIdentity {
    /// The identity of the user running this process. On Windows the token
    /// query can fail; the identity then matches nobody, so every request is
    /// denied (fail closed) rather than served unchecked.
    #[must_use]
    pub fn current() -> Self {
        #[cfg(target_os = "linux")]
        return Self(Some(narrivy_ipc_unix::current_uid()));
        #[cfg(windows)]
        return Self(narrivy_ipc_windows::current_user_sid().ok());
    }

    /// A fixed uid, for policy tests.
    #[cfg(all(test, target_os = "linux"))]
    #[must_use]
    pub const fn from_uid(uid: u32) -> Self {
        Self(Some(uid))
    }

    /// Whether `credentials` (as the transport verified them) are this user.
    #[must_use]
    pub fn matches(&self, credentials: &PeerCredentials) -> bool {
        #[cfg(target_os = "linux")]
        return self.0.is_some_and(|uid| credentials.is_same_user(uid));
        #[cfg(windows)]
        return self
            .0
            .as_ref()
            .is_some_and(|sid| credentials.is_same_user(sid));
    }

    /// Credentials for this user, for in-process callers and tests.
    #[must_use]
    pub fn credentials(&self, pid: Option<u32>) -> Option<PeerCredentials> {
        #[cfg(target_os = "linux")]
        return self.0.map(|uid| PeerCredentials { uid, gid: uid, pid });
        #[cfg(windows)]
        return self.0.map(|sid| PeerCredentials { sid, pid });
    }
}

/// Owner-only file modes. On Unix these set and check permission bits. On
/// Windows there are none to set: every daemon path lives under
/// `%LOCALAPPDATA%\Narrivy`, whose inherited ACL already limits it to the
/// user (plus SYSTEM and administrators), so the setters do nothing and the
/// checks accept what the ACL already guarantees.
pub mod fs {
    use std::fs::{DirBuilder, Metadata, OpenOptions};
    use std::io;
    use std::path::Path;

    /// Set `path`'s Unix mode; nothing on Windows.
    pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        }
        #[cfg(windows)]
        {
            let _ = (path, mode);
            Ok(())
        }
    }

    /// No group or other access (`mode & 0o077 == 0`); always on Windows.
    #[must_use]
    pub fn is_owner_only(metadata: &Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            metadata.permissions().mode() & 0o077 == 0
        }
        #[cfg(windows)]
        {
            let _ = metadata;
            true
        }
    }

    /// Any execute bit set; on Windows every regular file can be started.
    #[must_use]
    pub fn is_executable(metadata: &Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(windows)]
        {
            metadata.is_file()
        }
    }

    /// A recursive directory builder that creates with `mode` on Unix.
    #[must_use]
    pub fn dir_builder(mode: u32) -> DirBuilder {
        let mut builder = DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(mode);
        }
        #[cfg(windows)]
        let _ = mode;
        builder
    }

    /// Apply a creation mode to `options` on Unix.
    pub fn create_mode(options: &mut OpenOptions, mode: u32) -> &mut OpenOptions {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(mode)
        }
        #[cfg(windows)]
        {
            let _ = mode;
            options
        }
    }
}

/// Start `command` in a process group of its own, so a timeout can stop the
/// child and everything it started: `setpgid` on Unix,
/// `CREATE_NEW_PROCESS_GROUP` on Windows.
pub fn own_process_group(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP)
    }
}

/// Enforce the Inbox's retention (D42): files older than 15 minutes go, and
/// on Linux the oldest beyond its file bound.
pub fn prune_inbox(dir: std::path::PathBuf, now: std::time::SystemTime) -> usize {
    #[cfg(target_os = "linux")]
    {
        use narrivy_platform_linux::delivery::{INBOX_MAX_FILES, INBOX_RETENTION, InboxStore};
        InboxStore::new(dir).prune(INBOX_RETENTION, INBOX_MAX_FILES, now)
    }
    #[cfg(windows)]
    {
        narrivy_platform_windows::delivery::FileInbox::new(dir)
            .prune_older_than(std::time::Duration::from_secs(15 * 60), now)
    }
}

/// Record `seconds` from the microphone `endpoint` (else the default one)
/// as 16 kHz mono samples, through the same WASAPI capture as dictation.
/// For the microphone check and voice training; Linux records these with
/// `pw-record` in `voice.rs`.
///
/// # Errors
/// `voice_recorder_failed` when the device cannot be opened or delivers no
/// audio within the capture's bounds.
#[cfg(windows)]
pub fn record_microphone(endpoint: Option<&str>, seconds: u16) -> Result<Vec<f32>, String> {
    use narrivy_platform_api::AudioCapturePort as _;

    let failed = |_| "voice_recorder_failed".to_owned();
    let wanted = usize::from(seconds) * narrivy_types::CAPTURE_SAMPLE_RATE_HZ as usize;
    // One second over what is wanted, so the recording ends on the request
    // below rather than on the capture's bound. The echo-cancelled target
    // records the processed microphone, as dictation does.
    let mut capture = linux_features::echo_cancel::capture_for(f32::from(seconds) + 1.0, endpoint)
        .map_err(failed)?;
    let mut session = capture.start().map_err(failed)?;
    let control = narrivy_types::SessionControl::new();
    let captured = session
        .collect_observed(&control, &mut |samples| {
            if samples.len() >= wanted {
                control.request_finalization();
            }
        })
        .map_err(failed)?;
    let mut samples = captured.audio.samples;
    samples.truncate(wanted);
    Ok(samples)
}
