//! Windows-only ownership for trusted system helpers.
//!
//! The Windows counterpart of `narrivy-process-linux`, with the same public
//! API. It does not know about audio, text delivery, ASR, or daemon policy.
//! It validates a system helper in the Windows system directory (owned and
//! writable only by TrustedInstaller, SYSTEM or Administrators), clears the
//! child environment, and contains every child in its own Job Object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: children start suspended, join the
//! job, and only then run, so no descendant can start outside it. Closing
//! the job, including by the daemon exiting, kills the tree; that replaces
//! the Linux process group and parent-death signal. A job is not a
//! hostile-code sandbox, so callers must admit only trusted helpers.
//!
//! Unsafe policy: FFI is confined to small wrappers in this crate, each with
//! a `SAFETY` note.

#![cfg(windows)]

mod security;
pub mod usage;

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use narrivy_types::SessionCancellation;
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, READ_CONTROL,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    GetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::{PIPE_NOWAIT, PeekNamedPipe, SetNamedPipeHandleState};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

use security::{ObjectFacts, Principals, REPLACE_RIGHTS, WRITE_RIGHTS};

const POLL: Duration = Duration::from_millis(2);
const MAX_CONTROLLED_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CONTROLLED_DATA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Every helper keeps `SystemRoot`: Windows networking and cryptography fail
/// without it. It names the OS directory and carries no user content.
const BASE_ENVIRONMENT: [&str; 1] = ["SystemRoot"];
/// The job exit code used when the adapter kills a contained tree.
const KILLED_EXIT_CODE: u32 = 1;

/// Environment required by one reviewed helper role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperEnvironment {
    /// PipeWire same-user session routing only; code, plugin, and config
    /// loading override knobs are intentionally omitted.
    PipeWireCapture,
    /// Native Wayland connection only.
    WaylandTextInput,
    /// Resident ASR worker. It requires no inherited environment variables.
    ResidentAsr,
    /// The system download tool fetching a model the user asked for. It
    /// inherits nothing: no proxy, credential or configuration variable.
    HttpsDownload,
}

impl HelperEnvironment {
    /// Variables a role inherits beyond [`BASE_ENVIRONMENT`]. The Linux
    /// session-routing roles (PipeWire, Wayland) have no Windows variables.
    fn inherited_keys(self) -> &'static [&'static str] {
        match self {
            Self::PipeWireCapture
            | Self::WaylandTextInput
            | Self::ResidentAsr
            | Self::HttpsDownload => &[],
        }
    }
}

/// Content-free helper resolution failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    Missing,
    NonExecutable,
    Untrusted,
    IdentityChanged,
    UnsupportedPlatform,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "trusted helper is missing",
            Self::NonExecutable => "trusted helper is not executable",
            Self::Untrusted => "helper ownership or path is untrusted",
            Self::IdentityChanged => "helper identity changed after resolution",
            Self::UnsupportedPlatform => "required Windows helper isolation is unavailable",
        })
    }
}

impl std::error::Error for ResolveError {}

/// Distinguishes identity/trust drift from an operating-system spawn failure.
#[derive(Debug)]
pub enum LaunchError {
    Resolve(ResolveError),
    Spawn(io::Error),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolve(error) => error.fmt(formatter),
            Self::Spawn(_) => formatter.write_str("trusted helper spawn failed"),
        }
    }
}

impl std::error::Error for LaunchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Resolve(error) => Some(error),
            Self::Spawn(error) => Some(error),
        }
    }
}

/// Content-free process-group lifecycle failure.
#[derive(Debug)]
pub enum ProcessError {
    Wait(io::Error),
    Signal(io::Error),
    UnexpectedDescendant,
    TerminationTimeout,
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wait(_) => formatter.write_str("trusted helper wait failed"),
            Self::Signal(_) => formatter.write_str("trusted helper group signal failed"),
            Self::UnexpectedDescendant => {
                formatter.write_str("trusted helper left an unexpected descendant")
            }
            Self::TerminationTimeout => {
                formatter.write_str("trusted helper group termination timed out")
            }
        }
    }
}

impl std::error::Error for ProcessError {}

/// Provenance of a digest-bound executable artifact.
///
/// `ControlledBuild` is useful for deterministic recovery work but never
/// proves package installation or release provenance. `TrustAnchorDigestBound`
/// means that the source and every path component were observed under one
/// caller-supplied ownership/mode trust anchor before immutable snapshotting
/// (on Windows: installed for all users, under operating-system ownership).
/// It does not, by itself, prove that a manifest, package, or release owns the
/// artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactProvenance {
    ControlledBuild,
    TrustAnchorDigestBound,
}

/// Content-free failure while binding an executable artifact to immutable
/// bytes. This policy is deliberately distinct from [`ResolveError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactError {
    Cancelled,
    TimedOut,
    Missing,
    InvalidDigest,
    InvalidSize,
    Untrusted,
    NonExecutable,
    /// Not a native executable image for this host. The name is kept from
    /// the Linux adapter so both expose the same API.
    NotNativeElf,
    SizeMismatch,
    DigestMismatch,
    IdentityChanged,
    SnapshotUnavailable,
    UnsupportedPlatform,
}

/// Content-free failure while binding a caller-selected data artifact to
/// immutable bytes. A successful binding is a controlled-process fact, not
/// installed-package or release provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataArtifactError {
    Cancelled,
    TimedOut,
    Missing,
    InvalidDigest,
    InvalidSize,
    Untrusted,
    SizeMismatch,
    DigestMismatch,
    IdentityChanged,
    SnapshotUnavailable,
    UnsupportedPlatform,
}

impl std::fmt::Display for DataArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "controlled data snapshot was cancelled",
            Self::TimedOut => "controlled data snapshot timed out",
            Self::Missing => "controlled data artifact is missing",
            Self::InvalidDigest => "controlled data digest is invalid",
            Self::InvalidSize => "controlled data size is invalid",
            Self::Untrusted => "controlled data path or mode is unsafe",
            Self::SizeMismatch => "controlled data size does not match",
            Self::DigestMismatch => "controlled data digest does not match",
            Self::IdentityChanged => "controlled data changed while snapshotting",
            Self::SnapshotUnavailable => "immutable data snapshot is unavailable",
            Self::UnsupportedPlatform => "data artifact isolation is unavailable",
        })
    }
}

impl std::error::Error for DataArtifactError {}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "controlled executable operation was cancelled",
            Self::TimedOut => "controlled executable operation timed out",
            Self::Missing => "controlled executable artifact is missing",
            Self::InvalidDigest => "controlled executable digest is invalid",
            Self::InvalidSize => "controlled executable size is invalid",
            Self::Untrusted => "controlled executable path or mode is unsafe",
            Self::NonExecutable => "controlled executable artifact is not executable",
            Self::NotNativeElf => "controlled executable artifact is not a native Windows image",
            Self::SizeMismatch => "controlled executable size does not match",
            Self::DigestMismatch => "controlled executable digest does not match",
            Self::IdentityChanged => "controlled executable changed while snapshotting",
            Self::SnapshotUnavailable => "immutable executable snapshot is unavailable",
            Self::UnsupportedPlatform => "artifact execution isolation is unavailable",
        })
    }
}

impl std::error::Error for ArtifactError {}

/// Spawn failure for a digest-bound artifact command.
#[derive(Debug)]
pub enum ArtifactLaunchError {
    Artifact(ArtifactError),
    Data(DataArtifactError),
    Spawn(io::Error),
}

impl std::fmt::Display for ArtifactLaunchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Artifact(error) => error.fmt(formatter),
            Self::Data(error) => error.fmt(formatter),
            Self::Spawn(_) => formatter.write_str("digest-bound artifact spawn failed"),
        }
    }
}

impl std::error::Error for ArtifactLaunchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Artifact(error) => Some(error),
            Self::Data(error) => Some(error),
            Self::Spawn(error) => Some(error),
        }
    }
}

#[derive(Debug, Clone)]
struct TrustPolicy {
    /// Highest directory whose ancestry is checked (the volume root for the
    /// system directory).
    anchor: PathBuf,
}

/// A system helper whose path, identity, owner and DACL are revalidated
/// before every command is created.
#[derive(Clone)]
pub struct TrustedExecutable {
    launch_path: PathBuf,
    identity: ObjectFacts,
    policy: TrustPolicy,
}

/// A narrowly configurable command whose trust-critical program,
/// environment, working directory and job containment cannot be changed by a
/// payload adapter.
pub struct TrustedCommand {
    executable: TrustedExecutable,
    environment: HelperEnvironment,
    arguments: Vec<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

/// A private, immutable copy of artifact bytes. The file sits alone in a
/// fresh private directory (so no foreign DLL can sit beside an executable
/// snapshot) and stays open with a share mode that denies every writer and
/// every rename or delete for as long as any clone lives.
struct Snapshot {
    file: Option<File>,
    path: PathBuf,
    identity: ObjectFacts,
    _directory: tempfile::TempDir,
}

impl Snapshot {
    fn file(&self) -> Result<&File, Failure> {
        self.file.as_ref().ok_or(Failure::SnapshotUnavailable)
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        // Close the deny-delete handle first so the directory can be removed.
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Immutable, digest-bound snapshot of a caller-selected native executable.
///
/// The source pathname is used only while constructing the snapshot. Every
/// generation executes the same private snapshot file, so later rename or
/// in-place source changes cannot change the worker bytes.
#[derive(Clone)]
pub struct VerifiedArtifactExecutable {
    snapshot: Arc<Snapshot>,
    expected_size: u64,
    expected_sha256: [u8; 32],
    provenance: ArtifactProvenance,
}

/// Immutable, digest-bound snapshot of caller-selected read-only data.
///
/// Children receive the path of a private snapshot that nobody can write
/// while this value lives.
#[derive(Clone)]
pub struct VerifiedArtifactData {
    snapshot: Arc<Snapshot>,
    expected_size: u64,
    expected_sha256: [u8; 32],
    provenance: ArtifactProvenance,
}

/// Narrow command builder for one immutable artifact snapshot.
pub struct ArtifactCommand {
    executable: VerifiedArtifactExecutable,
    arguments: Vec<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
    inherited_data: Option<VerifiedArtifactData>,
}

impl std::fmt::Debug for VerifiedArtifactExecutable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedArtifactExecutable")
            .field("size", &self.expected_size)
            .field("provenance", &self.provenance)
            .field("binding", &"sha256-deny-write-snapshot")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ArtifactCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArtifactCommand")
            .field("executable", &self.executable)
            .field("argument_count", &self.arguments.len())
            .field("has_inherited_data", &self.inherited_data.is_some())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for VerifiedArtifactData {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedArtifactData")
            .field("size", &self.expected_size)
            .field("provenance", &self.provenance)
            .field("binding", &"sha256-deny-write-snapshot")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for TrustedCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrustedCommand")
            .field("executable", &self.executable)
            .field("environment", &self.environment)
            .field("argument_count", &self.arguments.len())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for TrustedExecutable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrustedExecutable")
            .field("launch_path", &self.launch_path)
            .field("trust", &"system-owned-revalidated")
            .finish_non_exhaustive()
    }
}

impl TrustedCommand {
    pub fn arg(&mut self, argument: impl AsRef<OsStr>) -> &mut Self {
        self.arguments.push(argument.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_os_string()),
        );
        self
    }

    pub fn stdin(&mut self, configuration: Stdio) -> &mut Self {
        self.stdin = Some(configuration);
        self
    }

    pub fn stdout(&mut self, configuration: Stdio) -> &mut Self {
        self.stdout = Some(configuration);
        self
    }

    pub fn stderr(&mut self, configuration: Stdio) -> &mut Self {
        self.stderr = Some(configuration);
        self
    }

    /// Revalidate at the final API boundary, then build, spawn and contain
    /// the child. Payload adapters cannot add environment variables or
    /// replace the executable between this check and the spawn.
    pub fn spawn(self) -> Result<Child, LaunchError> {
        self.executable.revalidate().map_err(LaunchError::Resolve)?;
        let mut command = base_command(&self.executable.launch_path, self.environment)
            .map_err(LaunchError::Spawn)?;
        command.args(&self.arguments);
        command.stdin(self.stdin.unwrap_or_else(Stdio::null));
        command.stdout(self.stdout.unwrap_or_else(Stdio::null));
        command.stderr(self.stderr.unwrap_or_else(Stdio::null));
        spawn_contained(&mut command).map_err(LaunchError::Spawn)
    }
}

impl TrustedExecutable {
    /// Resolve `name` from the Windows system directory, without consulting
    /// the daemon's inherited `PATH`. A name without an extension gets `.exe`.
    pub fn resolve_system(name: &OsStr) -> Result<Self, ResolveError> {
        let system = system_directory().map_err(|_| ResolveError::UnsupportedPlatform)?;
        Self::resolve_system_in(name, [system])
    }

    /// Resolve from injected absolute system directories. This supports
    /// deterministic trust-policy tests without consulting ambient `PATH`.
    pub fn resolve_system_in(
        name: &OsStr,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, ResolveError> {
        if Path::new(name).file_name() != Some(name) {
            return Err(ResolveError::Untrusted);
        }
        let name = if Path::new(name).extension().is_some() {
            name.to_os_string()
        } else {
            let mut with_extension = name.to_os_string();
            with_extension.push(".exe");
            with_extension
        };
        let principals =
            Principals::operating_system().map_err(|_| ResolveError::UnsupportedPlatform)?;
        let mut best = ResolveError::Missing;
        for directory in paths {
            if !directory.is_absolute() {
                best = ResolveError::Untrusted;
                continue;
            }
            let Some(anchor) = directory.ancestors().last().map(Path::to_path_buf) else {
                best = ResolveError::Untrusted;
                continue;
            };
            let policy = TrustPolicy { anchor };
            match Self::open_candidate(directory.join(&name), policy, &principals) {
                Ok(executable) => return Ok(executable),
                Err(ResolveError::Missing) => {}
                Err(ResolveError::NonExecutable) if best != ResolveError::Untrusted => {
                    best = ResolveError::NonExecutable;
                }
                Err(_) => best = ResolveError::Untrusted,
            }
        }
        Err(best)
    }

    fn open_candidate(
        launch_path: PathBuf,
        policy: TrustPolicy,
        principals: &Principals,
    ) -> Result<Self, ResolveError> {
        let identity = validate_helper(&launch_path, &policy, principals)?;
        Ok(Self {
            launch_path,
            identity,
            policy,
        })
    }

    /// Absolute helper path.
    #[must_use]
    pub fn launch_path(&self) -> &Path {
        &self.launch_path
    }

    /// Recheck identity, owner, DACL and path trust before spawn.
    pub fn revalidate(&self) -> Result<(), ResolveError> {
        let principals =
            Principals::operating_system().map_err(|_| ResolveError::IdentityChanged)?;
        let identity = validate_helper(&self.launch_path, &self.policy, &principals)
            .map_err(|_| ResolveError::IdentityChanged)?;
        if identity != self.identity {
            return Err(ResolveError::IdentityChanged);
        }
        Ok(())
    }

    /// Construct a scrubbed, contained command after revalidation. Callers
    /// still own argv and stdio because those are payload-specific contracts.
    pub fn command(&self, environment: HelperEnvironment) -> Result<TrustedCommand, ResolveError> {
        self.revalidate()?;
        Ok(TrustedCommand {
            executable: self.clone(),
            environment,
            arguments: Vec::new(),
            stdin: None,
            stdout: None,
            stderr: None,
        })
    }
}

/// Validate a helper file and its ancestry; return its identity.
fn validate_helper(
    path: &Path,
    policy: &TrustPolicy,
    principals: &Principals,
) -> Result<ObjectFacts, ResolveError> {
    let file = match security::open_object(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(ResolveError::Missing),
        Err(_) => return Err(ResolveError::Untrusted),
    };
    let facts = security::facts(&file).map_err(|_| ResolveError::Untrusted)?;
    if facts.is_reparse_point || facts.is_directory {
        return Err(ResolveError::Untrusted);
    }
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(ResolveError::NonExecutable);
    }
    if !security::owned_by(&file, principals).map_err(|_| ResolveError::Untrusted)?
        || !security::only_trusted_hold(&file, principals, WRITE_RIGHTS)
            .map_err(|_| ResolveError::Untrusted)?
    {
        return Err(ResolveError::Untrusted);
    }
    validate_ancestry(path, policy, principals)?;
    Ok(facts)
}

/// The helper's own directory may be written by nobody untrusted (a planted
/// DLL beside it would load first); higher ancestors may not let anyone
/// untrusted replace the path.
fn validate_ancestry(
    path: &Path,
    policy: &TrustPolicy,
    principals: &Principals,
) -> Result<(), ResolveError> {
    let parent = path.parent().ok_or(ResolveError::Untrusted)?;
    if !parent.starts_with(&policy.anchor) {
        return Err(ResolveError::Untrusted);
    }
    let mut reached_anchor = false;
    for (depth, component) in parent.ancestors().enumerate() {
        let file = security::open_object(component).map_err(|_| ResolveError::Untrusted)?;
        let facts = security::facts(&file).map_err(|_| ResolveError::Untrusted)?;
        if facts.is_reparse_point || !facts.is_directory {
            return Err(ResolveError::Untrusted);
        }
        let rights = if depth == 0 {
            WRITE_RIGHTS
        } else {
            REPLACE_RIGHTS
        };
        if !security::only_trusted_hold(&file, principals, rights)
            .map_err(|_| ResolveError::Untrusted)?
        {
            return Err(ResolveError::Untrusted);
        }
        if component == policy.anchor {
            reached_anchor = true;
            break;
        }
    }
    if !reached_anchor {
        return Err(ResolveError::Untrusted);
    }
    Ok(())
}

fn system_directory() -> io::Result<PathBuf> {
    let mut buffer = [0_u16; 512];
    // SAFETY: `buffer` is writable for its full length, which is passed.
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    let length = usize::try_from(length).map_err(|_| io::Error::other("system directory"))?;
    if length == 0 || length >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(<OsString as OsStringExt>::from_wide(
        &buffer[..length],
    )))
}

/// A helper command with a cleared environment (plus [`BASE_ENVIRONMENT`]
/// and the role's keys) running from the system directory.
fn base_command(program: &Path, environment: HelperEnvironment) -> io::Result<Command> {
    let mut command = Command::new(program);
    command.env_clear().current_dir(system_directory()?);
    for key in BASE_ENVIRONMENT.iter().chain(environment.inherited_keys()) {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    Ok(command)
}

// ---------------------------------------------------------------------------
// Job Object containment.

/// One kill-on-close job holding exactly one spawned tree.
struct Job(OwnedHandle);

impl Job {
    fn create() -> io::Result<Self> {
        // SAFETY: null attributes and name create an anonymous job; a null
        // result is handled.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is a fresh handle owned by nothing else.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw as RawHandle) });
        // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a valid
        // value of this plain-data struct.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the job handle is open and `limits` is a valid value of the
        // class's struct, passed with its exact size.
        let ok = unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0.as_raw_handle().cast()
    }

    fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are open; the child's handle belongs to the
        // `Child`, which outlives this call.
        if unsafe { AssignProcessToJobObject(self.raw(), child.as_raw_handle().cast()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn terminate(&self) -> io::Result<()> {
        // SAFETY: the job handle is open.
        if unsafe { TerminateJobObject(self.raw(), KILLED_EXIT_CODE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn active_processes(&self) -> io::Result<u32> {
        // SAFETY: an all-zero accounting struct is a valid plain-data value.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is writable for the size passed; the return-length
        // pointer may be null.
        let ok = unsafe {
            QueryInformationJobObject(
                self.raw(),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info.ActiveProcesses)
    }
}

/// Jobs of contained children by process id. An entry lives until the child
/// is reaped through [`terminate_group_and_reap`] or [`try_wait_contained`];
/// dropping the entry closes the job, which kills anything still in it.
fn jobs() -> &'static Mutex<HashMap<u32, Arc<Job>>> {
    static JOBS: OnceLock<Mutex<HashMap<u32, Arc<Job>>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn job_for(pid: u32) -> Option<Arc<Job>> {
    jobs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&pid)
        .cloned()
}

fn forget_job(pid: u32) {
    jobs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&pid);
}

/// Spawn suspended, join a fresh kill-on-close job, then resume. Any failure
/// kills the suspended child before returning.
fn spawn_contained(command: &mut Command) -> io::Result<Child> {
    command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
    let mut child = command.spawn()?;
    let contained = Job::create().and_then(|job| {
        job.assign(&child)?;
        resume_only_thread(child.id())?;
        Ok(job)
    });
    match contained {
        Ok(job) => {
            jobs()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(child.id(), Arc::new(job));
            Ok(child)
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

/// Resume the single thread of a process created with `CREATE_SUSPENDED`.
/// `std::process::Child` keeps no thread handle, so the thread is found
/// through a Toolhelp snapshot; a suspended new process has exactly one.
fn resume_only_thread(pid: u32) -> io::Result<()> {
    // SAFETY: plain-value arguments; failure is INVALID_HANDLE_VALUE.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `snapshot` is a fresh handle owned by nothing else.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot as RawHandle) };
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut threads = Vec::new();
    // SAFETY: `entry.dwSize` is set as the API requires; the snapshot is open.
    let mut more = unsafe { Thread32First(snapshot.as_raw_handle().cast(), &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            threads.push(entry.th32ThreadID);
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot.as_raw_handle().cast(), &mut entry) } != 0;
    }
    let [thread_id] = threads[..] else {
        return Err(io::Error::other(
            "suspended child does not have exactly one thread",
        ));
    };
    // SAFETY: plain-value arguments; a null result is handled.
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `thread` is a fresh handle owned by nothing else.
    let thread = unsafe { OwnedHandle::from_raw_handle(thread as RawHandle) };
    // SAFETY: the thread handle is open with THREAD_SUSPEND_RESUME.
    if unsafe { ResumeThread(thread.as_raw_handle().cast()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Windows cannot attach a job to a `Command` before `spawn` returns without
/// starting the child suspended, which a caller-built command would not
/// expect. This therefore only hides the console window; a caller that builds
/// its own command contains the child with [`adopt_spawned`] right after
/// spawning. Production launches go through [`ArtifactCommand::spawn`] and
/// [`TrustedCommand::spawn`], which start suspended and have no such window.
pub fn install_resident_worker_lifetime_hardening(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Put an already running child into its own kill-on-close job. Unlike the
/// adapter's own spawns there is a short window in which the child could
/// start a process outside the job, so prefer [`ArtifactCommand::spawn`].
pub fn adopt_spawned(child: &Child) -> Result<(), ProcessError> {
    let job = Job::create().map_err(ProcessError::Signal)?;
    job.assign(child).map_err(ProcessError::Signal)?;
    jobs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(child.id(), Arc::new(job));
    Ok(())
}

/// Poll a contained child. If it exited, any residual descendant in its job
/// is killed before the exit status is returned.
pub fn try_wait_contained(
    child: &mut Child,
    deadline: Instant,
) -> Result<Option<ExitStatus>, ProcessError> {
    let Some(status) = child.try_wait().map_err(ProcessError::Wait)? else {
        return Ok(None);
    };
    let Some(job) = job_for(child.id()) else {
        return Ok(Some(status));
    };
    if job.active_processes().map_err(ProcessError::Wait)? == 0 {
        forget_job(child.id());
        return Ok(Some(status));
    }
    job.terminate().map_err(ProcessError::Signal)?;
    wait_until_empty(&job, deadline)?;
    forget_job(child.id());
    Err(ProcessError::UnexpectedDescendant)
}

/// Kill the complete contained tree and boundedly reap its direct child.
pub fn terminate_group_and_reap(child: &mut Child, deadline: Instant) -> Result<(), ProcessError> {
    let mut reaped = child.try_wait().map_err(ProcessError::Wait)?.is_some();
    let job = job_for(child.id());
    match &job {
        Some(job) => job.terminate().map_err(ProcessError::Signal)?,
        None if !reaped => match child.kill() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
            Err(error) => return Err(ProcessError::Signal(error)),
        },
        None => {}
    }
    loop {
        if !reaped {
            reaped = child.try_wait().map_err(ProcessError::Wait)?.is_some();
        }
        let empty = match &job {
            Some(job) => job.active_processes().map_err(ProcessError::Wait)? == 0,
            None => true,
        };
        if reaped && empty {
            forget_job(child.id());
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(ProcessError::TerminationTimeout);
        }
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn wait_until_empty(job: &Job, deadline: Instant) -> Result<(), ProcessError> {
    loop {
        if job.active_processes().map_err(ProcessError::Wait)? == 0 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(ProcessError::TerminationTimeout);
        }
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

// ---------------------------------------------------------------------------
// Bounded child pipe I/O.
//
// Windows anonymous pipes have no `O_NONBLOCK`. A parent that must honour a
// deadline and cancellation while a child stalls therefore reads only what
// `PeekNamedPipe` reports as waiting, and writes through a `PIPE_NOWAIT`
// handle that accepts only what fits in the pipe buffer.

/// Make writes to a child's stdin return at once with what fits in the pipe
/// buffer, possibly nothing, instead of blocking on a stalled reader. Use
/// [`write_child_pipe`] afterwards.
pub fn set_child_stdin_nonblocking(stdin: &ChildStdin) -> io::Result<()> {
    let mode = PIPE_NOWAIT;
    // SAFETY: the handle is a live pipe handle owned by `stdin`; `mode` is a
    // valid pointer for the call and the two optional pointers are null.
    let ok = unsafe {
        SetNamedPipeHandleState(
            stdin.as_raw_handle().cast(),
            &mode,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Write without blocking to a stdin prepared by
/// [`set_child_stdin_nonblocking`]. A full pipe is `WouldBlock`; a closed
/// reader is an error.
pub fn write_child_pipe(stdin: &mut ChildStdin, bytes: &[u8]) -> io::Result<usize> {
    if bytes.is_empty() {
        return Ok(0);
    }
    match stdin.write(bytes)? {
        0 => Err(io::ErrorKind::WouldBlock.into()),
        count => Ok(count),
    }
}

/// Read without blocking from a child's stdout: only the bytes already
/// waiting are read. Nothing waiting is `WouldBlock`; a child that closed
/// its end is `Ok(0)` once everything it wrote has been read.
pub fn read_child_pipe(stdout: &mut ChildStdout, bytes: &mut [u8]) -> io::Result<usize> {
    if bytes.is_empty() {
        return Ok(0);
    }
    let mut available = 0_u32;
    // SAFETY: the handle is a live pipe handle owned by `stdout`; no data is
    // copied (null buffer, zero length) and `available` is a valid pointer.
    let ok = unsafe {
        PeekNamedPipe(
            stdout.as_raw_handle().cast(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let error = io::Error::last_os_error();
        // ERROR_BROKEN_PIPE: the writer closed and the pipe is drained.
        return if error.raw_os_error() == Some(109) {
            Ok(0)
        } else {
            Err(error)
        };
    }
    if available == 0 {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    let wanted = bytes
        .len()
        .min(usize::try_from(available).unwrap_or(usize::MAX));
    stdout.read(&mut bytes[..wanted])
}

/// The closest per-process counterpart of Linux `PR_SET_DUMPABLE` 0: this
/// process and its children no longer invoke Windows Error Reporting on a
/// crash, so no report or dump is collected for them by default. A
/// machine-wide WER `LocalDumps` policy set by an administrator is outside
/// the process's control.
///
/// # Errors
/// None today; the signature matches the Linux adapter.
pub fn forbid_core_dumps() -> io::Result<()> {
    // SAFETY: both calls only read or update this process's error-mode flags.
    unsafe {
        SetErrorMode(GetErrorMode() | SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Digest-bound artifact snapshots.

/// Shared failure of the snapshot machinery, mapped onto each public error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    Cancelled,
    TimedOut,
    InvalidSize,
    SizeMismatch,
    DigestMismatch,
    IdentityChanged,
    SnapshotUnavailable,
}

impl From<Failure> for ArtifactError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Cancelled => Self::Cancelled,
            Failure::TimedOut => Self::TimedOut,
            Failure::InvalidSize => Self::InvalidSize,
            Failure::SizeMismatch => Self::SizeMismatch,
            Failure::DigestMismatch => Self::DigestMismatch,
            Failure::IdentityChanged => Self::IdentityChanged,
            Failure::SnapshotUnavailable => Self::SnapshotUnavailable,
        }
    }
}

impl From<Failure> for DataArtifactError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Cancelled => Self::Cancelled,
            Failure::TimedOut => Self::TimedOut,
            Failure::InvalidSize => Self::InvalidSize,
            Failure::SizeMismatch => Self::SizeMismatch,
            Failure::DigestMismatch => Self::DigestMismatch,
            Failure::IdentityChanged => Self::IdentityChanged,
            Failure::SnapshotUnavailable => Self::SnapshotUnavailable,
        }
    }
}

/// One cooperative cancellation signal and absolute deadline. Cancellation
/// wins when both are observed together.
struct Control<'a> {
    cancellation: &'a SessionCancellation,
    deadline: Instant,
}

fn check(control: Option<&Control<'_>>) -> Result<(), Failure> {
    let Some(control) = control else {
        return Ok(());
    };
    if control.cancellation.is_cancelled() {
        return Err(Failure::Cancelled);
    }
    if Instant::now() >= control.deadline {
        return Err(Failure::TimedOut);
    }
    Ok(())
}

fn parse_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).ok()?;
        digest[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(digest)
}

/// Why a caller-selected source failed its checks, before any copying.
enum SourceRejection {
    Missing,
    Untrusted,
    SizeMismatch,
}

/// Open a caller-selected file without following a reparse point and check
/// that the current user owns it and nobody untrusted may write it.
fn open_controlled_source(
    path: &Path,
    expected_size: u64,
) -> Result<(File, ObjectFacts), SourceRejection> {
    if !path.is_absolute() {
        return Err(SourceRejection::Untrusted);
    }
    let file = OpenOptions::new()
        .access_mode(GENERIC_READ | READ_CONTROL)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                SourceRejection::Missing
            } else {
                SourceRejection::Untrusted
            }
        })?;
    let facts = security::facts(&file).map_err(|_| SourceRejection::Untrusted)?;
    if facts.is_reparse_point || facts.is_directory {
        return Err(SourceRejection::Untrusted);
    }
    let principals = Principals::current_user().map_err(|_| SourceRejection::Untrusted)?;
    if !security::owned_by(&file, &principals).map_err(|_| SourceRejection::Untrusted)?
        || !security::only_trusted_hold(&file, &principals, WRITE_RIGHTS)
            .map_err(|_| SourceRejection::Untrusted)?
    {
        return Err(SourceRejection::Untrusted);
    }
    if facts.size != expected_size {
        return Err(SourceRejection::SizeMismatch);
    }
    Ok((file, facts))
}

/// Open a file installed for all users (Program Files): owned by, and
/// writable only by, TrustedInstaller, SYSTEM or Administrators, inside
/// `anchor`, in a folder nobody untrusted can write and on a path nobody
/// untrusted can replace up to the volume root. The Windows form of a
/// root-owned file under a root-owned tree.
fn open_installed_source(
    path: &Path,
    anchor: &Path,
    expected_size: Option<u64>,
) -> Result<(File, ObjectFacts), SourceRejection> {
    if !path.is_absolute() || !anchor.is_absolute() || !path.starts_with(anchor) {
        return Err(SourceRejection::Untrusted);
    }
    let file = OpenOptions::new()
        .access_mode(GENERIC_READ | READ_CONTROL)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                SourceRejection::Missing
            } else {
                SourceRejection::Untrusted
            }
        })?;
    let facts = security::facts(&file).map_err(|_| SourceRejection::Untrusted)?;
    if facts.is_reparse_point || facts.is_directory {
        return Err(SourceRejection::Untrusted);
    }
    let principals = Principals::operating_system().map_err(|_| SourceRejection::Untrusted)?;
    if !security::owned_by(&file, &principals).map_err(|_| SourceRejection::Untrusted)?
        || !security::only_trusted_hold(&file, &principals, WRITE_RIGHTS)
            .map_err(|_| SourceRejection::Untrusted)?
    {
        return Err(SourceRejection::Untrusted);
    }
    let Some(volume) = path.ancestors().last().map(Path::to_path_buf) else {
        return Err(SourceRejection::Untrusted);
    };
    validate_ancestry(path, &TrustPolicy { anchor: volume }, &principals)
        .map_err(|_| SourceRejection::Untrusted)?;
    if expected_size.is_some_and(|size| facts.size != size) {
        return Err(SourceRejection::SizeMismatch);
    }
    Ok((file, facts))
}

/// Read a small file installed for all users (such as the installed-runtime
/// manifest), after the checks of [`open_installed_source`], through the
/// handle that was checked. Writers are shut out while it is read.
///
/// # Errors
/// [`DataArtifactError::Missing`], [`DataArtifactError::Untrusted`] or
/// [`DataArtifactError::InvalidSize`] when it is larger than `max_bytes`.
pub fn read_installed_file(
    path: &Path,
    anchor: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, DataArtifactError> {
    let (mut file, facts) =
        open_installed_source(path, anchor, None).map_err(|rejection| match rejection {
            SourceRejection::Missing => DataArtifactError::Missing,
            SourceRejection::Untrusted | SourceRejection::SizeMismatch => {
                DataArtifactError::Untrusted
            }
        })?;
    if facts.size > max_bytes {
        return Err(DataArtifactError::InvalidSize);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(facts.size).unwrap_or(0));
    (&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| DataArtifactError::Untrusted)?;
    if bytes.len() as u64 != facts.size {
        return Err(DataArtifactError::Untrusted);
    }
    Ok(bytes)
}

/// Copy exactly `expected_size` bytes into a fresh private snapshot, check
/// the digest, then reopen the snapshot with a share mode that denies every
/// writer and re-hash it through that handle, so bytes changed in between
/// are caught and none can change afterwards.
fn snapshot_source(
    mut source: File,
    opened: ObjectFacts,
    expected_sha256: [u8; 32],
    expected_size: u64,
    suffix: &str,
    control: Option<&Control<'_>>,
) -> Result<Snapshot, Failure> {
    check(control)?;
    let directory = tempfile::Builder::new()
        .prefix("narrivy-artifact-")
        .tempdir()
        .map_err(|_| Failure::SnapshotUnavailable)?;
    let path = directory.path().join(format!("snapshot{suffix}"));
    let mut copy = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&path)
        .map_err(|_| Failure::SnapshotUnavailable)?;
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        check(control)?;
        let count = source
            .read(&mut buffer)
            .map_err(|_| Failure::IdentityChanged)?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(u64::try_from(count).map_err(|_| Failure::InvalidSize)?)
            .ok_or(Failure::InvalidSize)?;
        if copied > expected_size {
            return Err(Failure::SizeMismatch);
        }
        hasher.update(&buffer[..count]);
        copy.write_all(&buffer[..count])
            .map_err(|_| Failure::SnapshotUnavailable)?;
    }
    check(control)?;
    if copied != expected_size {
        return Err(Failure::SizeMismatch);
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if actual != expected_sha256 {
        return Err(Failure::DigestMismatch);
    }
    if security::facts(&source).map_err(|_| Failure::IdentityChanged)? != opened {
        return Err(Failure::IdentityChanged);
    }
    copy.sync_all().map_err(|_| Failure::SnapshotUnavailable)?;
    drop(copy);
    let file = OpenOptions::new()
        .access_mode(GENERIC_READ | READ_CONTROL)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .map_err(|_| Failure::SnapshotUnavailable)?;
    let identity = security::facts(&file).map_err(|_| Failure::SnapshotUnavailable)?;
    let snapshot = Snapshot {
        file: Some(file),
        path,
        identity,
        _directory: directory,
    };
    verify_snapshot(&snapshot, expected_size, expected_sha256, control)?;
    Ok(snapshot)
}

/// Structure and full digest of a held snapshot.
fn verify_snapshot(
    snapshot: &Snapshot,
    expected_size: u64,
    expected_sha256: [u8; 32],
    control: Option<&Control<'_>>,
) -> Result<(), Failure> {
    verify_structure(snapshot, expected_size, control)?;
    let file = snapshot.file()?;
    let mut hasher = Sha256::new();
    let mut observed = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    while observed < expected_size {
        check(control)?;
        let remaining = expected_size - observed;
        let requested = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| Failure::InvalidSize)?;
        let count = file
            .seek_read(&mut buffer[..requested], observed)
            .map_err(|_| Failure::SnapshotUnavailable)?;
        if count == 0 {
            break;
        }
        observed = observed
            .checked_add(u64::try_from(count).map_err(|_| Failure::InvalidSize)?)
            .ok_or(Failure::InvalidSize)?;
        hasher.update(&buffer[..count]);
    }
    check(control)?;
    let digest: [u8; 32] = hasher.finalize().into();
    if observed != expected_size || digest != expected_sha256 {
        return Err(Failure::IdentityChanged);
    }
    Ok(())
}

fn verify_structure(
    snapshot: &Snapshot,
    expected_size: u64,
    control: Option<&Control<'_>>,
) -> Result<(), Failure> {
    check(control)?;
    let facts = security::facts(snapshot.file()?).map_err(|_| Failure::SnapshotUnavailable)?;
    if facts != snapshot.identity || facts.size != expected_size || facts.is_reparse_point {
        return Err(Failure::IdentityChanged);
    }
    Ok(())
}

/// A native executable image for this host: an `MZ` stub, a `PE\0\0`
/// header, this host's machine type, executable and not a DLL.
fn validate_native_image(file: &File, size: u64) -> Result<(), ArtifactError> {
    const IMAGE_FILE_EXECUTABLE_IMAGE: u16 = 0x0002;
    const IMAGE_FILE_DLL: u16 = 0x2000;
    let host_machine: u16 = if cfg!(target_arch = "x86_64") {
        0x8664
    } else if cfg!(target_arch = "aarch64") {
        0xAA64
    } else {
        return Err(ArtifactError::UnsupportedPlatform);
    };
    let mut stub = [0_u8; 64];
    read_exact_at(file, &mut stub, 0)?;
    if &stub[..2] != b"MZ" {
        return Err(ArtifactError::NotNativeElf);
    }
    let header_offset = u64::from(u32::from_le_bytes([stub[60], stub[61], stub[62], stub[63]]));
    if header_offset < 64 || header_offset.saturating_add(24) > size || header_offset > 4096 {
        return Err(ArtifactError::NotNativeElf);
    }
    let mut header = [0_u8; 24];
    read_exact_at(file, &mut header, header_offset)?;
    let machine = u16::from_le_bytes([header[4], header[5]]);
    let characteristics = u16::from_le_bytes([header[22], header[23]]);
    if &header[..4] != b"PE\0\0"
        || machine != host_machine
        || characteristics & IMAGE_FILE_EXECUTABLE_IMAGE == 0
        || characteristics & IMAGE_FILE_DLL != 0
    {
        return Err(ArtifactError::NotNativeElf);
    }
    Ok(())
}

fn read_exact_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> Result<(), ArtifactError> {
    while !bytes.is_empty() {
        let count = file
            .seek_read(bytes, offset)
            .map_err(|_| ArtifactError::SnapshotUnavailable)?;
        if count == 0 {
            return Err(ArtifactError::NotNativeElf);
        }
        offset += count as u64;
        bytes = &mut bytes[count..];
    }
    Ok(())
}

impl VerifiedArtifactExecutable {
    /// Bind a user/build-produced native executable to an immutable digest and
    /// exact size. This never produces installed-package provenance.
    pub fn bind_controlled(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<Self, ArtifactError> {
        Self::bind_controlled_inner(path, expected_sha256, expected_size, None)
    }

    fn bind_controlled_inner(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        control: Option<&Control<'_>>,
    ) -> Result<Self, ArtifactError> {
        if !path.is_absolute() {
            return Err(ArtifactError::Untrusted);
        }
        if expected_size == 0 || expected_size > MAX_CONTROLLED_EXECUTABLE_BYTES {
            return Err(ArtifactError::InvalidSize);
        }
        let expected_sha256 = parse_digest(expected_sha256).ok_or(ArtifactError::InvalidDigest)?;
        let (source, opened) =
            open_controlled_source(path, expected_size).map_err(|rejection| match rejection {
                SourceRejection::Missing => ArtifactError::Missing,
                SourceRejection::Untrusted => ArtifactError::Untrusted,
                SourceRejection::SizeMismatch => ArtifactError::SizeMismatch,
            })?;
        let snapshot = snapshot_source(
            source,
            opened,
            expected_sha256,
            expected_size,
            ".exe",
            control,
        )?;
        validate_native_image(snapshot.file()?, expected_size)?;
        Ok(Self {
            snapshot: Arc::new(snapshot),
            expected_size,
            expected_sha256,
            provenance: ArtifactProvenance::ControlledBuild,
        })
    }

    /// Bind an executable installed for all users (Program Files) under
    /// `anchor` to an immutable digest and exact size. Windows anchors trust
    /// on the operating-system principals (TrustedInstaller, SYSTEM and
    /// Administrators own it and alone may change it or replace its path),
    /// so the Unix `owner` is not used here.
    pub fn bind_trust_anchored(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
    ) -> Result<Self, ArtifactError> {
        let _ = owner;
        Self::bind_trust_anchored_inner(path, expected_sha256, expected_size, anchor, None)
    }

    /// [`Self::bind_trust_anchored`] within a cancellation and deadline
    /// budget.
    pub fn bind_trust_anchored_with_control(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<Self, ArtifactError> {
        let _ = owner;
        let control = Control {
            cancellation,
            deadline,
        };
        check(Some(&control))?;
        Self::bind_trust_anchored_inner(
            path,
            expected_sha256,
            expected_size,
            anchor,
            Some(&control),
        )
    }

    fn bind_trust_anchored_inner(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        control: Option<&Control<'_>>,
    ) -> Result<Self, ArtifactError> {
        if expected_size == 0 || expected_size > MAX_CONTROLLED_EXECUTABLE_BYTES {
            return Err(ArtifactError::InvalidSize);
        }
        let expected_sha256 = parse_digest(expected_sha256).ok_or(ArtifactError::InvalidDigest)?;
        let (source, opened) =
            open_installed_source(path, anchor, Some(expected_size)).map_err(|rejection| {
                match rejection {
                    SourceRejection::Missing => ArtifactError::Missing,
                    SourceRejection::Untrusted => ArtifactError::Untrusted,
                    SourceRejection::SizeMismatch => ArtifactError::SizeMismatch,
                }
            })?;
        let snapshot = snapshot_source(
            source,
            opened,
            expected_sha256,
            expected_size,
            ".exe",
            control,
        )?;
        validate_native_image(snapshot.file()?, expected_size)?;
        Ok(Self {
            snapshot: Arc::new(snapshot),
            expected_size,
            expected_sha256,
            provenance: ArtifactProvenance::TrustAnchorDigestBound,
        })
    }

    #[must_use]
    pub fn provenance(&self) -> ArtifactProvenance {
        self.provenance
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.expected_size
    }

    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        self.expected_sha256
    }

    /// Revalidate the immutable snapshot itself. The original pathname is no
    /// longer a launch input and therefore is deliberately not re-opened.
    pub fn revalidate(&self) -> Result<(), ArtifactError> {
        self.revalidate_inner(None)
    }

    /// Revalidate the complete immutable executable digest under one
    /// cooperative cancellation signal and absolute deadline.
    pub fn revalidate_with_control(
        &self,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<(), ArtifactError> {
        self.revalidate_inner(Some(&Control {
            cancellation,
            deadline,
        }))
    }

    fn revalidate_inner(&self, control: Option<&Control<'_>>) -> Result<(), ArtifactError> {
        verify_snapshot(
            &self.snapshot,
            self.expected_size,
            self.expected_sha256,
            control,
        )
        .map_err(ArtifactError::from)
    }

    pub fn command(
        &self,
        environment: HelperEnvironment,
    ) -> Result<ArtifactCommand, ArtifactError> {
        self.command_inner(environment, None, true)
    }

    /// Build an artifact command under one control budget. This validates the
    /// snapshot structure; [`ArtifactCommand::spawn_with_control`] performs
    /// the single full digest revalidation at the final launch boundary.
    pub fn command_with_control(
        &self,
        environment: HelperEnvironment,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<ArtifactCommand, ArtifactError> {
        self.command_inner(
            environment,
            Some(&Control {
                cancellation,
                deadline,
            }),
            false,
        )
    }

    fn command_inner(
        &self,
        environment: HelperEnvironment,
        control: Option<&Control<'_>>,
        revalidate_digest: bool,
    ) -> Result<ArtifactCommand, ArtifactError> {
        check(control)?;
        if environment != HelperEnvironment::ResidentAsr {
            return Err(ArtifactError::Untrusted);
        }
        if revalidate_digest {
            self.revalidate_inner(control)?;
        } else {
            verify_structure(&self.snapshot, self.expected_size, control)?;
        }
        check(control)?;
        Ok(ArtifactCommand {
            executable: self.clone(),
            arguments: Vec::new(),
            stdin: None,
            stdout: None,
            stderr: None,
            inherited_data: None,
        })
    }
}

impl VerifiedArtifactData {
    /// Copy exact caller-selected bytes into one immutable read-only snapshot.
    pub fn bind_controlled(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<Self, DataArtifactError> {
        if !path.is_absolute() {
            return Err(DataArtifactError::Untrusted);
        }
        if expected_size == 0 || expected_size > MAX_CONTROLLED_DATA_BYTES {
            return Err(DataArtifactError::InvalidSize);
        }
        let expected_sha256 =
            parse_digest(expected_sha256).ok_or(DataArtifactError::InvalidDigest)?;
        let (source, opened) =
            open_controlled_source(path, expected_size).map_err(|rejection| match rejection {
                SourceRejection::Missing => DataArtifactError::Missing,
                SourceRejection::Untrusted => DataArtifactError::Untrusted,
                SourceRejection::SizeMismatch => DataArtifactError::SizeMismatch,
            })?;
        let snapshot =
            snapshot_source(source, opened, expected_sha256, expected_size, ".bin", None)?;
        Ok(Self {
            snapshot: Arc::new(snapshot),
            expected_size,
            expected_sha256,
            provenance: ArtifactProvenance::ControlledBuild,
        })
    }

    /// Bind data installed for all users under `anchor`; see
    /// [`VerifiedArtifactExecutable::bind_trust_anchored`].
    pub fn bind_trust_anchored(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
    ) -> Result<Self, DataArtifactError> {
        let _ = owner;
        Self::bind_trust_anchored_inner(path, expected_sha256, expected_size, anchor, None)
    }

    /// [`Self::bind_trust_anchored`] within a cancellation and deadline
    /// budget.
    pub fn bind_trust_anchored_with_control(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<Self, DataArtifactError> {
        let _ = owner;
        let control = Control {
            cancellation,
            deadline,
        };
        check(Some(&control))?;
        Self::bind_trust_anchored_inner(
            path,
            expected_sha256,
            expected_size,
            anchor,
            Some(&control),
        )
    }

    fn bind_trust_anchored_inner(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        control: Option<&Control<'_>>,
    ) -> Result<Self, DataArtifactError> {
        if expected_size == 0 || expected_size > MAX_CONTROLLED_DATA_BYTES {
            return Err(DataArtifactError::InvalidSize);
        }
        let expected_sha256 =
            parse_digest(expected_sha256).ok_or(DataArtifactError::InvalidDigest)?;
        let (source, opened) =
            open_installed_source(path, anchor, Some(expected_size)).map_err(|rejection| {
                match rejection {
                    SourceRejection::Missing => DataArtifactError::Missing,
                    SourceRejection::Untrusted => DataArtifactError::Untrusted,
                    SourceRejection::SizeMismatch => DataArtifactError::SizeMismatch,
                }
            })?;
        let snapshot = snapshot_source(
            source,
            opened,
            expected_sha256,
            expected_size,
            ".bin",
            control,
        )?;
        Ok(Self {
            snapshot: Arc::new(snapshot),
            expected_size,
            expected_sha256,
            provenance: ArtifactProvenance::TrustAnchorDigestBound,
        })
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.expected_size
    }

    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        self.expected_sha256
    }

    #[must_use]
    pub fn provenance(&self) -> ArtifactProvenance {
        self.provenance
    }

    pub fn revalidate(&self) -> Result<(), DataArtifactError> {
        verify_snapshot(
            &self.snapshot,
            self.expected_size,
            self.expected_sha256,
            None,
        )
        .map_err(DataArtifactError::from)
    }
}

impl ArtifactCommand {
    pub fn arg(&mut self, argument: impl AsRef<OsStr>) -> &mut Self {
        self.arguments.push(argument.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_os_string()),
        );
        self
    }

    pub fn stdin(&mut self, configuration: Stdio) -> &mut Self {
        self.stdin = Some(configuration);
        self
    }

    pub fn stdout(&mut self, configuration: Stdio) -> &mut Self {
        self.stdout = Some(configuration);
        self
    }

    pub fn stderr(&mut self, configuration: Stdio) -> &mut Self {
        self.stderr = Some(configuration);
        self
    }

    /// Append one argument naming an immutable read-only data snapshot. Only
    /// one such artifact is admitted by this narrow builder. The child opens
    /// the snapshot path for reading; nobody can write it meanwhile.
    pub fn inherited_read_only_arg(
        &mut self,
        data: &VerifiedArtifactData,
    ) -> Result<&mut Self, DataArtifactError> {
        data.revalidate()?;
        if self.inherited_data.is_some() {
            return Err(DataArtifactError::Untrusted);
        }
        self.inherited_data = Some(data.clone());
        Ok(self)
    }

    pub fn spawn(self) -> Result<Child, ArtifactLaunchError> {
        self.spawn_inner(None)
    }

    /// Revalidate and spawn under one cooperative cancellation signal and
    /// absolute deadline. A final control check immediately precedes spawn.
    pub fn spawn_with_control(
        self,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<Child, ArtifactLaunchError> {
        self.spawn_inner(Some(&Control {
            cancellation,
            deadline,
        }))
    }

    fn spawn_inner(self, control: Option<&Control<'_>>) -> Result<Child, ArtifactLaunchError> {
        let artifact = |failure: Failure| ArtifactLaunchError::Artifact(failure.into());
        check(control).map_err(artifact)?;
        self.executable
            .revalidate_inner(control)
            .map_err(ArtifactLaunchError::Artifact)?;
        let mut arguments = self.arguments;
        if let Some(data) = self.inherited_data.as_ref() {
            check(control).map_err(artifact)?;
            data.revalidate().map_err(ArtifactLaunchError::Data)?;
            arguments.push(data.snapshot.path.clone().into_os_string());
        }
        let mut command = base_command(
            &self.executable.snapshot.path,
            HelperEnvironment::ResidentAsr,
        )
        .map_err(ArtifactLaunchError::Spawn)?;
        command
            .args(&arguments)
            .stdin(self.stdin.unwrap_or_else(Stdio::null))
            .stdout(self.stdout.unwrap_or_else(Stdio::null))
            .stderr(self.stderr.unwrap_or_else(Stdio::null));
        check(control).map_err(artifact)?;
        spawn_contained(&mut command).map_err(ArtifactLaunchError::Spawn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn system_helper_resolves_only_from_the_system_directory() {
        let cmd = TrustedExecutable::resolve_system(OsStr::new("cmd"));
        assert!(cmd.is_ok(), "{cmd:?}");
        assert_eq!(
            TrustedExecutable::resolve_system(OsStr::new("..\\cmd.exe")).err(),
            Some(ResolveError::Untrusted)
        );
        assert_eq!(
            TrustedExecutable::resolve_system(OsStr::new("narrivy-missing-helper")).err(),
            Some(ResolveError::Missing)
        );
    }

    #[test]
    fn user_writable_directory_is_not_a_system_helper_source() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        std::fs::copy(
            system_directory()?.join("cmd.exe"),
            directory.path().join("cmd.exe"),
        )?;
        assert_eq!(
            TrustedExecutable::resolve_system_in(
                OsStr::new("cmd"),
                [directory.path().to_path_buf()]
            )
            .err(),
            Some(ResolveError::Untrusted)
        );
        Ok(())
    }

    #[test]
    fn contained_helper_runs_and_the_tree_is_reaped() -> Result<(), Box<dyn std::error::Error>> {
        let cmd = TrustedExecutable::resolve_system(OsStr::new("cmd"))?;
        let mut command = cmd.command(HelperEnvironment::HttpsDownload)?;
        command.args(["/c", "ping -n 30 127.0.0.1 >NUL"]);
        let mut child = command.spawn()?;
        terminate_group_and_reap(&mut child, Instant::now() + Duration::from_secs(10))?;
        assert!(job_for(child.id()).is_none());
        Ok(())
    }

    #[test]
    fn data_snapshot_survives_source_changes() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("model.bin");
        std::fs::write(&source, b"exact model bytes")?;
        let data =
            VerifiedArtifactData::bind_controlled(&source, &sha256_hex(b"exact model bytes"), 17)?;
        std::fs::write(&source, b"changed afterwards")?;
        data.revalidate()?;
        assert!(
            OpenOptions::new()
                .write(true)
                .open(&data.snapshot.path)
                .is_err(),
            "nobody may write a held snapshot"
        );
        Ok(())
    }

    #[test]
    fn non_image_executable_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("worker.exe");
        std::fs::write(&source, b"#!/bin/sh\necho not a PE image\n")?;
        let digest = sha256_hex(b"#!/bin/sh\necho not a PE image\n");
        assert_eq!(
            VerifiedArtifactExecutable::bind_controlled(&source, &digest, 30).err(),
            Some(ArtifactError::NotNativeElf)
        );
        Ok(())
    }

    /// Installed (trust-anchored) bindings accept only files the operating
    /// system owns: a user's own file, a path outside the anchor and a
    /// manifest read from a user folder are all refused.
    #[test]
    fn trust_anchored_binding_refuses_user_files() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("worker.exe");
        let bytes = b"MZ not really";
        std::fs::write(&source, bytes)?;
        let digest = sha256_hex(bytes);
        let size = bytes.len() as u64;
        assert_eq!(
            VerifiedArtifactExecutable::bind_trust_anchored(
                &source,
                &digest,
                size,
                directory.path(),
                0
            )
            .err(),
            Some(ArtifactError::Untrusted)
        );
        assert_eq!(
            VerifiedArtifactData::bind_trust_anchored(&source, &digest, size, directory.path(), 0)
                .err(),
            Some(DataArtifactError::Untrusted)
        );
        assert_eq!(
            VerifiedArtifactData::bind_trust_anchored(
                &source,
                &digest,
                size,
                Path::new("C:\\elsewhere"),
                0
            )
            .err(),
            Some(DataArtifactError::Untrusted)
        );
        assert_eq!(
            read_installed_file(&source, directory.path(), 1024).err(),
            Some(DataArtifactError::Untrusted)
        );
        Ok(())
    }

    /// A file in the system directory passes the installed checks.
    #[test]
    fn system_files_read_as_installed() -> io::Result<()> {
        let system = system_directory()?;
        let path = system.join("drivers").join("etc").join("hosts");
        let bytes = read_installed_file(&path, &system, 1024 * 1024)
            .map_err(|_| io::Error::other("hosts refused"))?;
        assert!(!bytes.is_empty());
        Ok(())
    }

    /// A child that never reads its stdin: writes fill the pipe and then
    /// report `WouldBlock` instead of blocking; a silent stdout reports
    /// `WouldBlock`; once the child is gone and stdout drained, `Ok(0)`.
    #[test]
    fn child_pipes_never_block_on_a_stalled_child() -> io::Result<()> {
        let program = system_directory()?.join("PING.EXE");
        let mut child = Command::new(program)
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        let mut stdin = child.stdin.take().ok_or(io::ErrorKind::Other)?;
        let mut stdout = child.stdout.take().ok_or(io::ErrorKind::Other)?;
        set_child_stdin_nonblocking(&stdin)?;

        let chunk = vec![0_u8; 64 * 1024];
        let started = Instant::now();
        let mut written = 0_usize;
        let blocked = loop {
            match write_child_pipe(&mut stdin, &chunk) {
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break true,
                Err(error) => return Err(error),
            }
            if written > 64 * 1024 * 1024 {
                break false;
            }
        };
        assert!(blocked, "a stalled reader must surface WouldBlock");
        assert!(started.elapsed() < Duration::from_secs(5));

        child.kill()?;
        child.wait()?;
        let mut buffer = [0_u8; 4096];
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match read_child_pipe(&mut stdout, &mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "closed pipe never drained");
                    std::thread::sleep(POLL);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    #[test]
    fn silent_child_stdout_reports_would_block() -> io::Result<()> {
        let program = system_directory()?.join("PING.EXE");
        let mut child = Command::new(program)
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        let mut stdout = child.stdout.take().ok_or(io::ErrorKind::Other)?;
        // Drain the banner ping prints at once, then expect silence.
        std::thread::sleep(Duration::from_millis(300));
        let mut buffer = [0_u8; 4096];
        while read_child_pipe(&mut stdout, &mut buffer).is_ok() {}
        let result = read_child_pipe(&mut stdout, &mut buffer);
        child.kill()?;
        child.wait()?;
        assert_eq!(
            result.err().map(|error| error.kind()),
            Some(io::ErrorKind::WouldBlock)
        );
        Ok(())
    }
}
