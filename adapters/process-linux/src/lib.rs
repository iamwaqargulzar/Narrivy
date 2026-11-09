//! Linux-only ownership for trusted system helpers.
//!
//! This crate deliberately does not know about audio, text delivery, ASR, or
//! daemon policy. It validates a root-owned system-helper launch alias and
//! canonical executable, scrubs the child environment, creates a dedicated process
//! group, installs a parent-death contract, and provides bounded group cleanup.
//! Process groups are not a hostile-code sandbox: an executable which calls
//! `setsid` can escape them, so callers must admit only trusted helpers.

#![cfg(target_os = "linux")]

pub mod usage;

use std::ffi::OsStr;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use narrivy_types::SessionCancellation;
use sha2::{Digest, Sha256};

const POLL: Duration = Duration::from_millis(2);
const SYSTEM_HELPER_DIRS: [&str; 2] = ["/usr/bin", "/bin"];
const MAX_CONTROLLED_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CONTROLLED_DATA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const INHERITED_DATA_FD_MIN: i32 = 256;
const ELF64_HEADER_BYTES: usize = 64;
const ELF32_HEADER_BYTES: usize = 52;
const MAX_ELF_PROGRAM_HEADERS: u16 = 128;

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
    fn inherited_keys(self) -> &'static [&'static str] {
        match self {
            Self::PipeWireCapture => {
                &["XDG_RUNTIME_DIR", "PIPEWIRE_RUNTIME_DIR", "PIPEWIRE_REMOTE"]
            }
            Self::WaylandTextInput => &["XDG_RUNTIME_DIR", "WAYLAND_DISPLAY"],
            Self::ResidentAsr | Self::HttpsDownload => &[],
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
            Self::UnsupportedPlatform => "required Linux helper isolation is unavailable",
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    owner: u32,
    group: u32,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl FileIdentity {
    fn from(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            owner: metadata.uid(),
            group: metadata.gid(),
            size: metadata.size(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

#[derive(Debug, Clone)]
struct TrustPolicy {
    owner: u32,
    anchor: PathBuf,
}

/// A root/package-owned system helper whose launch alias and canonical target
/// are revalidated before every command is created.
#[derive(Clone)]
pub struct TrustedExecutable {
    launch_path: PathBuf,
    canonical_target: PathBuf,
    launch_identity: FileIdentity,
    target_identity: FileIdentity,
    policy: TrustPolicy,
}

/// A narrowly configurable command whose trust-critical program, environment,
/// working directory, process group, and pre-exec contract cannot be changed
/// by a payload adapter.
pub struct TrustedCommand {
    executable: TrustedExecutable,
    environment: HelperEnvironment,
    arguments: Vec<std::ffi::OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

/// Provenance of a digest-bound executable artifact.
///
/// `ControlledBuild` is useful for deterministic recovery work but never
/// proves package installation or release provenance. `TrustAnchorDigestBound`
/// means that the source and every path component were observed under one
/// caller-supplied ownership/mode trust anchor before immutable snapshotting.
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
            Self::NotNativeElf => "controlled executable artifact is not a native ELF",
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

/// Immutable, digest-bound snapshot of a caller-selected native executable.
///
/// The source pathname is used only while constructing the snapshot. Every
/// generation executes the same sealed memfd, so later rename or in-place
/// source changes cannot change the worker bytes.
#[derive(Clone)]
pub struct VerifiedArtifactExecutable {
    snapshot: Arc<File>,
    expected_size: u64,
    expected_sha256: [u8; 32],
    provenance: ArtifactProvenance,
}

/// Immutable, digest-bound snapshot of caller-selected read-only data.
///
/// The source pathname is used only during construction. Children receive a
/// sealed, read-only memfd, so later source rename or mutation cannot alter the
/// bytes they consume.
#[derive(Clone)]
pub struct VerifiedArtifactData {
    snapshot: Arc<File>,
    expected_size: u64,
    expected_sha256: [u8; 32],
    provenance: ArtifactProvenance,
}

/// Narrow command builder for one immutable artifact snapshot.
pub struct ArtifactCommand {
    executable: VerifiedArtifactExecutable,
    arguments: Vec<std::ffi::OsString>,
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
            .field("binding", &"sha256-sealed-memfd")
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
            .field("binding", &"sha256-sealed-memfd")
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

    /// Revalidate at the final API boundary, then build and spawn the child.
    /// Payload adapters cannot add environment variables or replace the
    /// executable between this check and `Command::spawn`.
    pub fn spawn(self) -> Result<Child, LaunchError> {
        self.executable.revalidate().map_err(LaunchError::Resolve)?;
        let mut command = self.executable.base_command(self.environment);
        command.args(&self.arguments);
        command.stdin(self.stdin.unwrap_or_else(Stdio::null));
        command.stdout(self.stdout.unwrap_or_else(Stdio::null));
        command.stderr(self.stderr.unwrap_or_else(Stdio::null));
        command.spawn().map_err(LaunchError::Spawn)
    }
}

impl VerifiedArtifactExecutable {
    /// Bind a user/build-produced native executable to an immutable digest and
    /// exact size. This never produces installed-package provenance.
    pub fn bind_controlled(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<Self, ArtifactError> {
        if !path.is_absolute() {
            return Err(ArtifactError::Untrusted);
        }
        Self::bind_with_policy(
            path,
            expected_sha256,
            expected_size,
            ArtifactProvenance::ControlledBuild,
            |metadata| validate_controlled_artifact(metadata, expected_size),
        )
    }

    /// Bind one executable beneath an explicit ownership/mode trust anchor.
    ///
    /// This is a generic filesystem observation. Callers remain responsible
    /// for establishing any manifest, package, or release relationship and
    /// must not infer one from the returned provenance alone.
    pub fn bind_trust_anchored(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
    ) -> Result<Self, ArtifactError> {
        Self::bind_trust_anchored_inner(
            path,
            expected_sha256,
            expected_size,
            anchor,
            owner,
            None,
            &mut |_| {},
        )
    }

    /// Bind a trust-anchored executable under one cooperative cancellation
    /// signal and absolute deadline.
    ///
    /// Snapshot copy and digest validation use fixed-size chunks with control
    /// checks between them. Individual filesystem syscalls can still block in
    /// the kernel, so the bound is cooperative. Cancellation wins when it and
    /// deadline expiry are observed together.
    pub fn bind_trust_anchored_with_control(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<Self, ArtifactError> {
        let control = ArtifactControl {
            cancellation,
            deadline,
            now: &Instant::now,
        };
        Self::bind_trust_anchored_inner(
            path,
            expected_sha256,
            expected_size,
            anchor,
            owner,
            Some(&control),
            &mut |_| {},
        )
    }

    fn bind_trust_anchored_inner(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
        control: Option<&ArtifactControl<'_>>,
        checkpoint: &mut impl FnMut(ArtifactCheckpoint),
    ) -> Result<Self, ArtifactError> {
        check_artifact_control(control)?;
        validate_nofollow_ancestry_with_control(path, anchor, owner, control)?;
        if expected_size == 0 {
            return Err(ArtifactError::InvalidSize);
        }
        let expected_sha256 = parse_sha256(expected_sha256)?;
        check_artifact_control(control)?;
        ensure_close_range_support().map_err(|_| ArtifactError::UnsupportedPlatform)?;
        let anchor_path_metadata =
            std::fs::symlink_metadata(anchor).map_err(|_| ArtifactError::Untrusted)?;
        let anchor_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(anchor)
            .map_err(|_| ArtifactError::Untrusted)?;
        let anchor_metadata = anchor_file
            .metadata()
            .map_err(|_| ArtifactError::Untrusted)?;
        if FileIdentity::from(&anchor_metadata) != FileIdentity::from(&anchor_path_metadata) {
            return Err(ArtifactError::IdentityChanged);
        }
        check_artifact_control(control)?;
        let relative = path
            .strip_prefix(anchor)
            .map_err(|_| ArtifactError::Untrusted)?;
        let source = rustix::fs::openat2(
            &anchor_file,
            relative,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_MAGICLINKS,
        )
        .map(File::from)
        .map_err(|error| match error {
            rustix::io::Errno::NOENT => ArtifactError::Missing,
            rustix::io::Errno::NOSYS => ArtifactError::UnsupportedPlatform,
            _ => ArtifactError::Untrusted,
        })?;
        let opened_metadata = source.metadata().map_err(|_| ArtifactError::Untrusted)?;
        validate_trust_anchored_artifact(&opened_metadata, expected_size, owner)?;
        check_artifact_control(control)?;
        Self::snapshot_opened_with_control(
            source,
            opened_metadata,
            expected_sha256,
            expected_size,
            ArtifactProvenance::TrustAnchorDigestBound,
            control,
            checkpoint,
        )
    }

    fn bind_with_policy(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        provenance: ArtifactProvenance,
        validate: impl Fn(&Metadata) -> Result<(), ArtifactError>,
    ) -> Result<Self, ArtifactError> {
        if !path.is_absolute() || expected_size == 0 {
            return Err(if expected_size == 0 {
                ArtifactError::InvalidSize
            } else {
                ArtifactError::Untrusted
            });
        }
        let expected_sha256 = parse_sha256(expected_sha256)?;
        ensure_close_range_support().map_err(|_| ArtifactError::UnsupportedPlatform)?;

        let path_metadata = std::fs::symlink_metadata(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                ArtifactError::Missing
            } else {
                ArtifactError::Untrusted
            }
        })?;
        validate(&path_metadata)?;

        let source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    ArtifactError::Missing
                } else {
                    ArtifactError::Untrusted
                }
            })?;
        let opened_metadata = source.metadata().map_err(|_| ArtifactError::Untrusted)?;
        if FileIdentity::from(&opened_metadata) != FileIdentity::from(&path_metadata) {
            return Err(ArtifactError::IdentityChanged);
        }

        Self::snapshot_opened(
            source,
            opened_metadata,
            expected_sha256,
            expected_size,
            provenance,
        )
    }

    fn snapshot_opened(
        source: File,
        opened_metadata: Metadata,
        expected_sha256: [u8; 32],
        expected_size: u64,
        provenance: ArtifactProvenance,
    ) -> Result<Self, ArtifactError> {
        Self::snapshot_opened_with_control(
            source,
            opened_metadata,
            expected_sha256,
            expected_size,
            provenance,
            None,
            &mut |_| {},
        )
    }

    fn snapshot_opened_with_control(
        mut source: File,
        opened_metadata: Metadata,
        expected_sha256: [u8; 32],
        expected_size: u64,
        provenance: ArtifactProvenance,
        control: Option<&ArtifactControl<'_>>,
        checkpoint: &mut impl FnMut(ArtifactCheckpoint),
    ) -> Result<Self, ArtifactError> {
        check_artifact_control(control)?;
        let mut snapshot = create_sealable_memfd()?;
        let mut hasher = Sha256::new();
        let mut copied = 0_u64;
        let mut elf_prefix = [0_u8; ELF64_HEADER_BYTES];
        let mut prefix_len = 0_usize;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            check_artifact_control(control)?;
            let count = source
                .read(&mut buffer)
                .map_err(|_| ArtifactError::IdentityChanged)?;
            if count == 0 {
                break;
            }
            copied = copied
                .checked_add(u64::try_from(count).map_err(|_| ArtifactError::InvalidSize)?)
                .ok_or(ArtifactError::InvalidSize)?;
            if copied > expected_size {
                return Err(ArtifactError::SizeMismatch);
            }
            if prefix_len < elf_prefix.len() {
                let take = (elf_prefix.len() - prefix_len).min(count);
                elf_prefix[prefix_len..prefix_len + take].copy_from_slice(&buffer[..take]);
                prefix_len += take;
            }
            hasher.update(&buffer[..count]);
            let mut written = 0;
            while written < count {
                check_artifact_control(control)?;
                let count_written = snapshot
                    .write(&buffer[written..count])
                    .map_err(|_| ArtifactError::SnapshotUnavailable)?;
                if count_written == 0 {
                    return Err(ArtifactError::SnapshotUnavailable);
                }
                written += count_written;
            }
            checkpoint(ArtifactCheckpoint::SnapshotChunk(copied));
            check_artifact_control(control)?;
        }
        checkpoint(ArtifactCheckpoint::SnapshotComplete);
        check_artifact_control(control)?;
        if copied != expected_size {
            return Err(ArtifactError::SizeMismatch);
        }
        validate_native_elf(&snapshot, &elf_prefix[..prefix_len], expected_size)?;
        let actual: [u8; 32] = hasher.finalize().into();
        if actual != expected_sha256 {
            return Err(ArtifactError::DigestMismatch);
        }
        let final_source_metadata = source
            .metadata()
            .map_err(|_| ArtifactError::IdentityChanged)?;
        if FileIdentity::from(&final_source_metadata) != FileIdentity::from(&opened_metadata) {
            return Err(ArtifactError::IdentityChanged);
        }

        check_artifact_control(control)?;
        if unsafe { libc::fchmod(snapshot.as_raw_fd(), 0o500) } != 0 {
            return Err(ArtifactError::SnapshotUnavailable);
        }
        check_artifact_control(control)?;
        seal_memfd(&snapshot)?;
        let executable = Self {
            snapshot: Arc::new(snapshot),
            expected_size,
            expected_sha256,
            provenance,
        };
        executable.validate_snapshot_structure(control)?;
        check_artifact_control(control)?;
        Ok(executable)
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
        self.revalidate_inner(None, &mut |_| {})
    }

    /// Revalidate the complete immutable executable digest under one
    /// cooperative cancellation signal and absolute deadline.
    pub fn revalidate_with_control(
        &self,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<(), ArtifactError> {
        let control = ArtifactControl {
            cancellation,
            deadline,
            now: &Instant::now,
        };
        self.revalidate_inner(Some(&control), &mut |_| {})
    }

    fn revalidate_inner(
        &self,
        control: Option<&ArtifactControl<'_>>,
        checkpoint: &mut impl FnMut(ArtifactCheckpoint),
    ) -> Result<(), ArtifactError> {
        self.validate_snapshot_structure(control)?;
        check_artifact_control(control)?;
        let mut hasher = Sha256::new();
        let mut observed_size = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        while observed_size < self.expected_size {
            check_artifact_control(control)?;
            let remaining = self.expected_size - observed_size;
            let requested = usize::try_from(remaining.min(buffer.len() as u64))
                .map_err(|_| ArtifactError::InvalidSize)?;
            let count = self
                .snapshot
                .read_at(&mut buffer[..requested], observed_size)
                .map_err(|_| ArtifactError::SnapshotUnavailable)?;
            if count == 0 {
                break;
            }
            observed_size = observed_size
                .checked_add(u64::try_from(count).map_err(|_| ArtifactError::InvalidSize)?)
                .ok_or(ArtifactError::InvalidSize)?;
            hasher.update(&buffer[..count]);
            checkpoint(ArtifactCheckpoint::RevalidatedChunk(observed_size));
            check_artifact_control(control)?;
        }
        checkpoint(ArtifactCheckpoint::RevalidationComplete);
        check_artifact_control(control)?;
        let digest: [u8; 32] = hasher.finalize().into();
        if observed_size != self.expected_size || digest != self.expected_sha256 {
            return Err(ArtifactError::IdentityChanged);
        }
        Ok(())
    }

    fn validate_snapshot_structure(
        &self,
        control: Option<&ArtifactControl<'_>>,
    ) -> Result<(), ArtifactError> {
        check_artifact_control(control)?;
        let metadata = self
            .snapshot
            .metadata()
            .map_err(|_| ArtifactError::SnapshotUnavailable)?;
        if !metadata.is_file()
            || metadata.len() != self.expected_size
            || metadata.permissions().mode() & 0o7777 != 0o500
        {
            return Err(ArtifactError::IdentityChanged);
        }
        check_artifact_control(control)?;
        let seals = rustix::fs::fcntl_get_seals(&self.snapshot)
            .map_err(|_| ArtifactError::SnapshotUnavailable)?;
        let required = artifact_seals();
        if seals != required {
            return Err(ArtifactError::IdentityChanged);
        }
        check_artifact_control(control)?;
        let proc_metadata = std::fs::metadata(proc_fd_path(self.snapshot.as_raw_fd()))
            .map_err(|_| ArtifactError::UnsupportedPlatform)?;
        if FileIdentity::from(&proc_metadata) != FileIdentity::from(&metadata) {
            return Err(ArtifactError::IdentityChanged);
        }
        check_artifact_control(control)?;
        Ok(())
    }

    pub fn command(
        &self,
        environment: HelperEnvironment,
    ) -> Result<ArtifactCommand, ArtifactError> {
        self.command_inner(environment, None, true, &mut |_| {})
    }

    /// Build an artifact command under one control budget. This validates the
    /// sealed snapshot structure; [`ArtifactCommand::spawn_with_control`]
    /// performs the single full digest revalidation at the final launch
    /// boundary under the same caller-owned budget.
    pub fn command_with_control(
        &self,
        environment: HelperEnvironment,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<ArtifactCommand, ArtifactError> {
        let control = ArtifactControl {
            cancellation,
            deadline,
            now: &Instant::now,
        };
        self.command_inner(environment, Some(&control), false, &mut |_| {})
    }

    fn command_inner(
        &self,
        environment: HelperEnvironment,
        control: Option<&ArtifactControl<'_>>,
        revalidate_digest: bool,
        checkpoint: &mut impl FnMut(ArtifactCheckpoint),
    ) -> Result<ArtifactCommand, ArtifactError> {
        check_artifact_control(control)?;
        if environment != HelperEnvironment::ResidentAsr {
            return Err(ArtifactError::Untrusted);
        }
        if revalidate_digest {
            self.revalidate_inner(control, checkpoint)?;
        } else {
            self.validate_snapshot_structure(control)?;
        }
        check_artifact_control(control)?;
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
        let expected_sha256 = parse_data_sha256(expected_sha256)?;
        ensure_close_range_support().map_err(|_| DataArtifactError::UnsupportedPlatform)?;

        let path_metadata = std::fs::symlink_metadata(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                DataArtifactError::Missing
            } else {
                DataArtifactError::Untrusted
            }
        })?;
        validate_controlled_data(&path_metadata, expected_size)?;
        let source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    DataArtifactError::Missing
                } else {
                    DataArtifactError::Untrusted
                }
            })?;
        let opened_metadata = source
            .metadata()
            .map_err(|_| DataArtifactError::Untrusted)?;
        if FileIdentity::from(&opened_metadata) != FileIdentity::from(&path_metadata) {
            return Err(DataArtifactError::IdentityChanged);
        }

        Self::snapshot_opened(
            source,
            opened_metadata,
            expected_sha256,
            expected_size,
            ArtifactProvenance::ControlledBuild,
        )
    }

    /// Bind exact data bytes beneath an explicit ownership/mode trust anchor.
    /// The descriptor-relative open rejects traversal and every symlink. The
    /// returned provenance still does not imply manifest, package, or release
    /// membership.
    pub fn bind_trust_anchored(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
    ) -> Result<Self, DataArtifactError> {
        Self::bind_trust_anchored_inner(
            path,
            expected_sha256,
            expected_size,
            anchor,
            owner,
            None,
            |_| {},
        )
    }

    /// Bind exact trust-anchored data under one cooperative cancellation and
    /// absolute-deadline budget.
    ///
    /// Reads and writes are split into fixed-size chunks and control is checked
    /// between them. Individual filesystem syscalls can still block in the
    /// kernel, so this is a cooperative bound rather than a hard wall-clock
    /// guarantee. Cancellation has priority when cancellation and expiry are
    /// observed together.
    pub fn bind_trust_anchored_with_control(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<Self, DataArtifactError> {
        let control = DataSnapshotControl {
            cancellation,
            deadline,
            now: &Instant::now,
        };
        Self::bind_trust_anchored_inner(
            path,
            expected_sha256,
            expected_size,
            anchor,
            owner,
            Some(&control),
            |_| {},
        )
    }

    fn bind_trust_anchored_inner(
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
        anchor: &Path,
        owner: u32,
        control: Option<&DataSnapshotControl<'_>>,
        mut checkpoint: impl FnMut(DataSnapshotCheckpoint),
    ) -> Result<Self, DataArtifactError> {
        check_data_snapshot_control(control)?;
        if expected_size == 0 || expected_size > MAX_CONTROLLED_DATA_BYTES {
            return Err(DataArtifactError::InvalidSize);
        }
        validate_nofollow_data_ancestry_with_control(
            path,
            anchor,
            owner,
            control,
            &mut checkpoint,
        )?;
        let expected_sha256 = parse_data_sha256(expected_sha256)?;
        check_data_snapshot_control(control)?;
        ensure_close_range_support().map_err(|_| DataArtifactError::UnsupportedPlatform)?;
        let anchor_path_metadata =
            std::fs::symlink_metadata(anchor).map_err(|_| DataArtifactError::Untrusted)?;
        let anchor_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(anchor)
            .map_err(|_| DataArtifactError::Untrusted)?;
        let anchor_metadata = anchor_file
            .metadata()
            .map_err(|_| DataArtifactError::Untrusted)?;
        if FileIdentity::from(&anchor_metadata) != FileIdentity::from(&anchor_path_metadata) {
            return Err(DataArtifactError::IdentityChanged);
        }
        check_data_snapshot_control(control)?;
        let relative = path
            .strip_prefix(anchor)
            .map_err(|_| DataArtifactError::Untrusted)?;
        let source = rustix::fs::openat2(
            &anchor_file,
            relative,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_MAGICLINKS,
        )
        .map(File::from)
        .map_err(|error| match error {
            rustix::io::Errno::NOENT => DataArtifactError::Missing,
            rustix::io::Errno::NOSYS => DataArtifactError::UnsupportedPlatform,
            _ => DataArtifactError::Untrusted,
        })?;
        let opened_metadata = source
            .metadata()
            .map_err(|_| DataArtifactError::Untrusted)?;
        validate_trust_anchored_data(&opened_metadata, expected_size, owner)?;
        check_data_snapshot_control(control)?;
        Self::snapshot_opened_with_control(
            source,
            opened_metadata,
            expected_sha256,
            expected_size,
            ArtifactProvenance::TrustAnchorDigestBound,
            control,
            &mut checkpoint,
        )
    }

    fn snapshot_opened(
        source: File,
        opened_metadata: Metadata,
        expected_sha256: [u8; 32],
        expected_size: u64,
        provenance: ArtifactProvenance,
    ) -> Result<Self, DataArtifactError> {
        Self::snapshot_opened_with_control(
            source,
            opened_metadata,
            expected_sha256,
            expected_size,
            provenance,
            None,
            &mut |_| {},
        )
    }

    fn snapshot_opened_with_control(
        mut source: File,
        opened_metadata: Metadata,
        expected_sha256: [u8; 32],
        expected_size: u64,
        provenance: ArtifactProvenance,
        control: Option<&DataSnapshotControl<'_>>,
        checkpoint: &mut impl FnMut(DataSnapshotCheckpoint),
    ) -> Result<Self, DataArtifactError> {
        check_data_snapshot_control(control)?;
        let mut snapshot = create_sealable_data_memfd()?;
        let mut hasher = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            check_data_snapshot_control(control)?;
            let count = source
                .read(&mut buffer)
                .map_err(|_| DataArtifactError::IdentityChanged)?;
            if count == 0 {
                break;
            }
            copied = copied
                .checked_add(u64::try_from(count).map_err(|_| DataArtifactError::InvalidSize)?)
                .ok_or(DataArtifactError::InvalidSize)?;
            if copied > expected_size {
                return Err(DataArtifactError::SizeMismatch);
            }
            hasher.update(&buffer[..count]);
            let mut written = 0;
            while written < count {
                check_data_snapshot_control(control)?;
                let count_written = snapshot
                    .write(&buffer[written..count])
                    .map_err(|_| DataArtifactError::SnapshotUnavailable)?;
                if count_written == 0 {
                    return Err(DataArtifactError::SnapshotUnavailable);
                }
                written += count_written;
            }
            checkpoint(DataSnapshotCheckpoint::CopiedChunk(copied));
            check_data_snapshot_control(control)?;
        }
        checkpoint(DataSnapshotCheckpoint::CopyComplete);
        check_data_snapshot_control(control)?;
        if copied != expected_size {
            return Err(DataArtifactError::SizeMismatch);
        }
        let actual: [u8; 32] = hasher.finalize().into();
        if actual != expected_sha256 {
            return Err(DataArtifactError::DigestMismatch);
        }
        let final_source_metadata = source
            .metadata()
            .map_err(|_| DataArtifactError::IdentityChanged)?;
        if FileIdentity::from(&final_source_metadata) != FileIdentity::from(&opened_metadata) {
            return Err(DataArtifactError::IdentityChanged);
        }
        check_data_snapshot_control(control)?;
        if unsafe { libc::fchmod(snapshot.as_raw_fd(), 0o400) } != 0 {
            return Err(DataArtifactError::SnapshotUnavailable);
        }
        check_data_snapshot_control(control)?;
        seal_data_memfd(&snapshot)?;
        let data = Self {
            snapshot: Arc::new(snapshot),
            expected_size,
            expected_sha256,
            provenance,
        };
        data.revalidate()?;
        check_data_snapshot_control(control)?;
        Ok(data)
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
        let metadata = self
            .snapshot
            .metadata()
            .map_err(|_| DataArtifactError::SnapshotUnavailable)?;
        if !metadata.is_file()
            || metadata.len() != self.expected_size
            || metadata.permissions().mode() & 0o7777 != 0o400
        {
            return Err(DataArtifactError::IdentityChanged);
        }
        let seals = rustix::fs::fcntl_get_seals(&self.snapshot)
            .map_err(|_| DataArtifactError::SnapshotUnavailable)?;
        if seals != data_seals() {
            return Err(DataArtifactError::IdentityChanged);
        }
        Ok(())
    }
}

struct ArtifactControl<'a> {
    cancellation: &'a SessionCancellation,
    deadline: Instant,
    now: &'a dyn Fn() -> Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactCheckpoint {
    SnapshotChunk(u64),
    SnapshotComplete,
    RevalidatedChunk(u64),
    RevalidationComplete,
    BeforeSpawn,
}

fn check_artifact_control(control: Option<&ArtifactControl<'_>>) -> Result<(), ArtifactError> {
    let Some(control) = control else {
        return Ok(());
    };
    if control.cancellation.is_cancelled() {
        return Err(ArtifactError::Cancelled);
    }
    if (control.now)() >= control.deadline {
        return Err(ArtifactError::TimedOut);
    }
    Ok(())
}

struct DataSnapshotControl<'a> {
    cancellation: &'a SessionCancellation,
    deadline: Instant,
    now: &'a dyn Fn() -> Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataSnapshotCheckpoint {
    AncestryComponent(usize),
    CopiedChunk(u64),
    CopyComplete,
}

fn check_data_snapshot_control(
    control: Option<&DataSnapshotControl<'_>>,
) -> Result<(), DataArtifactError> {
    let Some(control) = control else {
        return Ok(());
    };
    if control.cancellation.is_cancelled() {
        return Err(DataArtifactError::Cancelled);
    }
    if (control.now)() >= control.deadline {
        return Err(DataArtifactError::TimedOut);
    }
    Ok(())
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

    /// Append one argument naming an inherited immutable read-only data
    /// descriptor. Only one such artifact is admitted by this narrow builder.
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
        self.spawn_inner(None, &mut |_| {})
    }

    /// Revalidate and spawn under one cooperative cancellation signal and
    /// absolute deadline. The executable digest is checked in fixed-size
    /// chunks, and a final control check immediately precedes the spawn call.
    pub fn spawn_with_control(
        self,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<Child, ArtifactLaunchError> {
        let control = ArtifactControl {
            cancellation,
            deadline,
            now: &Instant::now,
        };
        self.spawn_inner(Some(&control), &mut |_| {})
    }

    fn spawn_inner(
        self,
        control: Option<&ArtifactControl<'_>>,
        checkpoint: &mut impl FnMut(ArtifactCheckpoint),
    ) -> Result<Child, ArtifactLaunchError> {
        check_artifact_control(control).map_err(ArtifactLaunchError::Artifact)?;
        self.executable
            .revalidate_inner(control, checkpoint)
            .map_err(ArtifactLaunchError::Artifact)?;
        check_artifact_control(control).map_err(ArtifactLaunchError::Artifact)?;
        let inherited_fd = self
            .inherited_data
            .as_ref()
            .map(|data| {
                check_artifact_control(control).map_err(ArtifactLaunchError::Artifact)?;
                data.revalidate().map_err(ArtifactLaunchError::Data)?;
                check_artifact_control(control).map_err(ArtifactLaunchError::Artifact)?;
                rustix::io::fcntl_dupfd_cloexec(&data.snapshot, INHERITED_DATA_FD_MIN)
                    .map(File::from)
                    .map_err(|error| ArtifactLaunchError::Spawn(io::Error::from(error)))
            })
            .transpose()?;
        let mut arguments = self.arguments;
        if let Some(file) = inherited_fd.as_ref() {
            arguments.push(proc_fd_path(file.as_raw_fd()).into_os_string());
        }
        let mut command = Command::new(proc_fd_path(self.executable.snapshot.as_raw_fd()));
        command
            .env_clear()
            .current_dir("/")
            .process_group(0)
            .args(&arguments)
            .stdin(self.stdin.unwrap_or_else(Stdio::null))
            .stdout(self.stdout.unwrap_or_else(Stdio::null))
            .stderr(self.stderr.unwrap_or_else(Stdio::null));
        install_child_hardening(
            &mut command,
            true,
            inherited_fd.as_ref().map(AsRawFd::as_raw_fd),
        );
        checkpoint(ArtifactCheckpoint::BeforeSpawn);
        check_artifact_control(control).map_err(ArtifactLaunchError::Artifact)?;
        command.spawn().map_err(ArtifactLaunchError::Spawn)
    }
}

fn parse_data_sha256(value: &str) -> Result<[u8; 32], DataArtifactError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DataArtifactError::InvalidDigest);
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).map_err(|_| DataArtifactError::InvalidDigest)?;
        digest[index] =
            u8::from_str_radix(text, 16).map_err(|_| DataArtifactError::InvalidDigest)?;
    }
    Ok(digest)
}

fn validate_controlled_data(
    metadata: &Metadata,
    expected_size: u64,
) -> Result<(), DataArtifactError> {
    if !metadata.file_type().is_file() {
        return Err(DataArtifactError::Untrusted);
    }
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid || metadata.permissions().mode() & 0o6022 != 0 {
        return Err(DataArtifactError::Untrusted);
    }
    if metadata.len() != expected_size {
        return Err(DataArtifactError::SizeMismatch);
    }
    Ok(())
}

fn validate_trust_anchored_data(
    metadata: &Metadata,
    expected_size: u64,
    owner: u32,
) -> Result<(), DataArtifactError> {
    if !metadata.file_type().is_file()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o6022 != 0
    {
        return Err(DataArtifactError::Untrusted);
    }
    if metadata.len() != expected_size {
        return Err(DataArtifactError::SizeMismatch);
    }
    Ok(())
}

fn create_sealable_data_memfd() -> Result<File, DataArtifactError> {
    rustix::fs::memfd_create(
        "narrivy-controlled-data",
        rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
    )
    .map(File::from)
    .map_err(|_| DataArtifactError::SnapshotUnavailable)
}

fn seal_data_memfd(file: &File) -> Result<(), DataArtifactError> {
    rustix::fs::fcntl_add_seals(file, data_seals())
        .map_err(|_| DataArtifactError::SnapshotUnavailable)
}

fn data_seals() -> rustix::fs::SealFlags {
    rustix::fs::SealFlags::WRITE
        | rustix::fs::SealFlags::GROW
        | rustix::fs::SealFlags::SHRINK
        | rustix::fs::SealFlags::FUTURE_WRITE
        | rustix::fs::SealFlags::EXEC
        | rustix::fs::SealFlags::SEAL
}

fn parse_sha256(value: &str) -> Result<[u8; 32], ArtifactError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ArtifactError::InvalidDigest);
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).map_err(|_| ArtifactError::InvalidDigest)?;
        digest[index] = u8::from_str_radix(text, 16).map_err(|_| ArtifactError::InvalidDigest)?;
    }
    Ok(digest)
}

fn validate_controlled_artifact(
    metadata: &Metadata,
    expected_size: u64,
) -> Result<(), ArtifactError> {
    if !metadata.file_type().is_file() {
        return Err(ArtifactError::Untrusted);
    }
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid || metadata.permissions().mode() & 0o6022 != 0 {
        return Err(ArtifactError::Untrusted);
    }
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(ArtifactError::NonExecutable);
    }
    if expected_size > MAX_CONTROLLED_EXECUTABLE_BYTES {
        return Err(ArtifactError::InvalidSize);
    }
    if metadata.len() != expected_size {
        return Err(ArtifactError::SizeMismatch);
    }
    Ok(())
}

fn validate_trust_anchored_artifact(
    metadata: &Metadata,
    expected_size: u64,
    owner: u32,
) -> Result<(), ArtifactError> {
    if !metadata.file_type().is_file()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o7777 != 0o755
    {
        return Err(ArtifactError::Untrusted);
    }
    if expected_size > MAX_CONTROLLED_EXECUTABLE_BYTES {
        return Err(ArtifactError::InvalidSize);
    }
    if metadata.len() != expected_size {
        return Err(ArtifactError::SizeMismatch);
    }
    Ok(())
}

fn validate_nofollow_ancestry_with_control(
    path: &Path,
    anchor: &Path,
    owner: u32,
    control: Option<&ArtifactControl<'_>>,
) -> Result<(), ArtifactError> {
    check_artifact_control(control)?;
    if !path.is_absolute() || !anchor.is_absolute() || !path.starts_with(anchor) {
        return Err(ArtifactError::Untrusted);
    }
    let mut current = path.parent().ok_or(ArtifactError::Untrusted)?;
    loop {
        check_artifact_control(control)?;
        let metadata = std::fs::symlink_metadata(current).map_err(|_| ArtifactError::Untrusted)?;
        check_artifact_control(control)?;
        if !metadata.file_type().is_dir()
            || metadata.uid() != owner
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(ArtifactError::Untrusted);
        }
        if current == anchor {
            return Ok(());
        }
        current = current.parent().ok_or(ArtifactError::Untrusted)?;
    }
}

fn validate_nofollow_data_ancestry_with_control(
    path: &Path,
    anchor: &Path,
    owner: u32,
    control: Option<&DataSnapshotControl<'_>>,
    checkpoint: &mut impl FnMut(DataSnapshotCheckpoint),
) -> Result<(), DataArtifactError> {
    check_data_snapshot_control(control)?;
    if !path.is_absolute() || !anchor.is_absolute() || !path.starts_with(anchor) {
        return Err(DataArtifactError::Untrusted);
    }
    let mut current = path.parent().ok_or(DataArtifactError::Untrusted)?;
    let mut component_count = 0_usize;
    loop {
        check_data_snapshot_control(control)?;
        let metadata =
            std::fs::symlink_metadata(current).map_err(|_| DataArtifactError::Untrusted)?;
        check_data_snapshot_control(control)?;
        if !metadata.file_type().is_dir()
            || metadata.uid() != owner
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(DataArtifactError::Untrusted);
        }
        component_count = component_count
            .checked_add(1)
            .ok_or(DataArtifactError::Untrusted)?;
        checkpoint(DataSnapshotCheckpoint::AncestryComponent(component_count));
        check_data_snapshot_control(control)?;
        if current == anchor {
            return Ok(());
        }
        current = current.parent().ok_or(DataArtifactError::Untrusted)?;
    }
}

fn create_sealable_memfd() -> Result<File, ArtifactError> {
    rustix::fs::memfd_create(
        "narrivy-controlled-worker",
        rustix::fs::MemfdFlags::CLOEXEC
            | rustix::fs::MemfdFlags::ALLOW_SEALING
            | rustix::fs::MemfdFlags::EXEC,
    )
    .map(File::from)
    .map_err(|_| ArtifactError::SnapshotUnavailable)
}

fn seal_memfd(file: &File) -> Result<(), ArtifactError> {
    rustix::fs::fcntl_add_seals(file, artifact_seals())
        .map_err(|_| ArtifactError::SnapshotUnavailable)
}

fn artifact_seals() -> rustix::fs::SealFlags {
    rustix::fs::SealFlags::WRITE
        | rustix::fs::SealFlags::GROW
        | rustix::fs::SealFlags::SHRINK
        | rustix::fs::SealFlags::FUTURE_WRITE
        | rustix::fs::SealFlags::EXEC
        | rustix::fs::SealFlags::SEAL
}

fn validate_native_elf(file: &File, header: &[u8], size: u64) -> Result<(), ArtifactError> {
    if header.len() < ELF32_HEADER_BYTES || &header[..4] != b"\x7fELF" || header[6] != 1 {
        return Err(ArtifactError::NotNativeElf);
    }
    let expected_class = if size_of::<usize>() == 8 { 2 } else { 1 };
    let expected_endian = if cfg!(target_endian = "little") { 1 } else { 2 };
    if header[4] != expected_class || header[5] != expected_endian {
        return Err(ArtifactError::NotNativeElf);
    }
    let read_u16 = |bytes: [u8; 2]| {
        if cfg!(target_endian = "little") {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }
    };
    let read_u32 = |bytes: [u8; 4]| {
        if cfg!(target_endian = "little") {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        }
    };
    let read_u64 = |bytes: [u8; 8]| {
        if cfg!(target_endian = "little") {
            u64::from_le_bytes(bytes)
        } else {
            u64::from_be_bytes(bytes)
        }
    };
    let elf_type = read_u16([header[16], header[17]]);
    let machine = read_u16([header[18], header[19]]);
    if !matches!(elf_type, 2 | 3)
        || machine != host_elf_machine()
        || read_u32(
            header[20..24]
                .try_into()
                .map_err(|_| ArtifactError::NotNativeElf)?,
        ) != 1
    {
        return Err(ArtifactError::NotNativeElf);
    }
    let (header_size, program_offset, program_entry_size, program_count) = if expected_class == 2 {
        if header.len() < ELF64_HEADER_BYTES {
            return Err(ArtifactError::NotNativeElf);
        }
        (
            ELF64_HEADER_BYTES as u16,
            read_u64(
                header[32..40]
                    .try_into()
                    .map_err(|_| ArtifactError::NotNativeElf)?,
            ),
            read_u16(
                header[54..56]
                    .try_into()
                    .map_err(|_| ArtifactError::NotNativeElf)?,
            ),
            read_u16(
                header[56..58]
                    .try_into()
                    .map_err(|_| ArtifactError::NotNativeElf)?,
            ),
        )
    } else {
        (
            ELF32_HEADER_BYTES as u16,
            u64::from(read_u32(
                header[28..32]
                    .try_into()
                    .map_err(|_| ArtifactError::NotNativeElf)?,
            )),
            read_u16(
                header[42..44]
                    .try_into()
                    .map_err(|_| ArtifactError::NotNativeElf)?,
            ),
            read_u16(
                header[44..46]
                    .try_into()
                    .map_err(|_| ArtifactError::NotNativeElf)?,
            ),
        )
    };
    let encoded_header_size = if expected_class == 2 {
        read_u16(
            header[52..54]
                .try_into()
                .map_err(|_| ArtifactError::NotNativeElf)?,
        )
    } else {
        read_u16(
            header[40..42]
                .try_into()
                .map_err(|_| ArtifactError::NotNativeElf)?,
        )
    };
    let expected_program_size = if expected_class == 2 { 56 } else { 32 };
    if encoded_header_size != header_size
        || program_entry_size != expected_program_size
        || program_count == 0
        || program_count > MAX_ELF_PROGRAM_HEADERS
        || program_offset < u64::from(header_size)
    {
        return Err(ArtifactError::NotNativeElf);
    }
    let table_size = u64::from(program_entry_size)
        .checked_mul(u64::from(program_count))
        .ok_or(ArtifactError::NotNativeElf)?;
    if program_offset
        .checked_add(table_size)
        .filter(|end| *end <= size)
        .is_none()
    {
        return Err(ArtifactError::NotNativeElf);
    }
    let mut has_executable_load = false;
    let mut entry = [0_u8; 56];
    for index in 0..program_count {
        let offset = program_offset + u64::from(index) * u64::from(program_entry_size);
        read_exact_at(file, &mut entry[..usize::from(program_entry_size)], offset)?;
        let segment_type = read_u32(
            entry[..4]
                .try_into()
                .map_err(|_| ArtifactError::NotNativeElf)?,
        );
        if segment_type != 1 {
            continue;
        }
        let (flags, file_offset, file_size, memory_size) = if expected_class == 2 {
            (
                read_u32(
                    entry[4..8]
                        .try_into()
                        .map_err(|_| ArtifactError::NotNativeElf)?,
                ),
                read_u64(
                    entry[8..16]
                        .try_into()
                        .map_err(|_| ArtifactError::NotNativeElf)?,
                ),
                read_u64(
                    entry[32..40]
                        .try_into()
                        .map_err(|_| ArtifactError::NotNativeElf)?,
                ),
                read_u64(
                    entry[40..48]
                        .try_into()
                        .map_err(|_| ArtifactError::NotNativeElf)?,
                ),
            )
        } else {
            (
                read_u32(
                    entry[24..28]
                        .try_into()
                        .map_err(|_| ArtifactError::NotNativeElf)?,
                ),
                u64::from(read_u32(
                    entry[4..8]
                        .try_into()
                        .map_err(|_| ArtifactError::NotNativeElf)?,
                )),
                u64::from(read_u32(
                    entry[16..20]
                        .try_into()
                        .map_err(|_| ArtifactError::NotNativeElf)?,
                )),
                u64::from(read_u32(
                    entry[20..24]
                        .try_into()
                        .map_err(|_| ArtifactError::NotNativeElf)?,
                )),
            )
        };
        if file_size > memory_size
            || file_offset
                .checked_add(file_size)
                .filter(|end| *end <= size)
                .is_none()
        {
            return Err(ArtifactError::NotNativeElf);
        }
        if flags & 1 != 0 && file_size != 0 {
            has_executable_load = true;
        }
    }
    if !has_executable_load {
        return Err(ArtifactError::NotNativeElf);
    }
    Ok(())
}

fn read_exact_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> Result<(), ArtifactError> {
    while !bytes.is_empty() {
        let count = file
            .read_at(bytes, offset)
            .map_err(|_| ArtifactError::NotNativeElf)?;
        if count == 0 {
            return Err(ArtifactError::NotNativeElf);
        }
        offset = offset
            .checked_add(u64::try_from(count).map_err(|_| ArtifactError::NotNativeElf)?)
            .ok_or(ArtifactError::NotNativeElf)?;
        bytes = &mut bytes[count..];
    }
    Ok(())
}

const fn host_elf_machine() -> u16 {
    #[cfg(target_arch = "x86")]
    {
        3
    }
    #[cfg(target_arch = "x86_64")]
    {
        62
    }
    #[cfg(target_arch = "arm")]
    {
        40
    }
    #[cfg(target_arch = "aarch64")]
    {
        183
    }
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        243
    }
    #[cfg(not(any(
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "riscv32",
        target_arch = "riscv64"
    )))]
    {
        0
    }
}

fn proc_fd_path(fd: i32) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{fd}"))
}

impl std::fmt::Debug for TrustedExecutable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrustedExecutable")
            .field("launch_path", &self.launch_path)
            .field("canonical_target", &self.canonical_target)
            .field("trust", &"system-owned-revalidated")
            .finish_non_exhaustive()
    }
}

impl TrustedExecutable {
    /// Resolve `name` from the reviewed system-helper directories, without
    /// consulting the daemon's inherited `PATH`.
    pub fn resolve_system(name: &OsStr) -> Result<Self, ResolveError> {
        Self::resolve_system_in(name, SYSTEM_HELPER_DIRS.into_iter().map(PathBuf::from))
    }

    /// Resolve from injected absolute system directories. This supports
    /// deterministic trust-policy tests without consulting ambient `PATH`.
    pub fn resolve_system_in(
        name: &OsStr,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, ResolveError> {
        Self::resolve_with_policy(
            name,
            paths,
            TrustPolicy {
                owner: 0,
                anchor: PathBuf::from("/"),
            },
        )
    }

    fn resolve_with_policy(
        name: &OsStr,
        paths: impl IntoIterator<Item = PathBuf>,
        policy: TrustPolicy,
    ) -> Result<Self, ResolveError> {
        if Path::new(name).file_name() != Some(name) {
            return Err(ResolveError::Untrusted);
        }
        ensure_close_range_support()?;
        let mut best = ResolveError::Missing;
        for directory in paths {
            if !directory.is_absolute() {
                best = ResolveError::Untrusted;
                continue;
            }
            let candidate = directory.join(name);
            match Self::open_candidate(candidate, policy.clone()) {
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

    fn open_candidate(launch_path: PathBuf, policy: TrustPolicy) -> Result<Self, ResolveError> {
        let launch_metadata = match std::fs::symlink_metadata(&launch_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ResolveError::Missing);
            }
            Err(_) => return Err(ResolveError::Untrusted),
        };
        validate_launch_leaf(&launch_metadata, policy.owner)?;
        validate_ancestry(&launch_path, &policy)?;
        let canonical_target =
            std::fs::canonicalize(&launch_path).map_err(|_| ResolveError::Untrusted)?;
        if !canonical_target.starts_with(&policy.anchor) {
            return Err(ResolveError::Untrusted);
        }
        validate_ancestry(&canonical_target, &policy)?;
        let target_metadata =
            std::fs::symlink_metadata(&canonical_target).map_err(|_| ResolveError::Untrusted)?;
        validate_target(&target_metadata, policy.owner)?;
        Ok(Self {
            launch_path,
            canonical_target,
            launch_identity: FileIdentity::from(&launch_metadata),
            target_identity: FileIdentity::from(&target_metadata),
            policy,
        })
    }

    /// Absolute alias retained for argv[0]-sensitive tools such as pw-record.
    #[must_use]
    pub fn launch_path(&self) -> &Path {
        &self.launch_path
    }

    /// Recheck alias, target, and path trust before spawn.
    pub fn revalidate(&self) -> Result<(), ResolveError> {
        let launch_metadata = std::fs::symlink_metadata(&self.launch_path)
            .map_err(|_| ResolveError::IdentityChanged)?;
        if FileIdentity::from(&launch_metadata) != self.launch_identity {
            return Err(ResolveError::IdentityChanged);
        }
        validate_launch_leaf(&launch_metadata, self.policy.owner)
            .map_err(|_| ResolveError::IdentityChanged)?;
        validate_ancestry(&self.launch_path, &self.policy)
            .map_err(|_| ResolveError::IdentityChanged)?;
        let canonical =
            std::fs::canonicalize(&self.launch_path).map_err(|_| ResolveError::IdentityChanged)?;
        if canonical != self.canonical_target {
            return Err(ResolveError::IdentityChanged);
        }
        validate_ancestry(&canonical, &self.policy).map_err(|_| ResolveError::IdentityChanged)?;
        let target_metadata =
            std::fs::symlink_metadata(&canonical).map_err(|_| ResolveError::IdentityChanged)?;
        validate_target(&target_metadata, self.policy.owner)
            .map_err(|_| ResolveError::IdentityChanged)?;
        if FileIdentity::from(&target_metadata) != self.target_identity {
            return Err(ResolveError::IdentityChanged);
        }
        Ok(())
    }

    /// Construct a scrubbed, isolated command after revalidation. Callers
    /// still own argv and stdio because those are payload-specific contracts.
    pub fn command(&self, environment: HelperEnvironment) -> Result<TrustedCommand, ResolveError> {
        self.revalidate()?;
        ensure_close_range_support()?;
        Ok(TrustedCommand {
            executable: self.clone(),
            environment,
            arguments: Vec::new(),
            stdin: None,
            stdout: None,
            stderr: None,
        })
    }

    fn base_command(&self, environment: HelperEnvironment) -> Command {
        let mut command = Command::new(&self.launch_path);
        command.env_clear().current_dir("/").process_group(0);
        for key in environment.inherited_keys() {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        if environment == HelperEnvironment::PipeWireCapture {
            command.env("PIPEWIRE_NO_CONFIG", "true");
        }
        if environment == HelperEnvironment::WaylandTextInput {
            // `wtype` converts its text with the C library's multibyte
            // functions; in the default "C" locale anything beyond ASCII
            // (é, ’, —, Urdu) fails to convert (F-H8). A constant, never
            // inherited.
            command.env("LC_ALL", "C.UTF-8");
        }
        install_child_hardening(&mut command, false, None);
        command
    }
}

fn validate_launch_leaf(metadata: &Metadata, owner: u32) -> Result<(), ResolveError> {
    let kind = metadata.file_type();
    if !kind.is_file() && !kind.is_symlink() {
        return Err(ResolveError::Untrusted);
    }
    if metadata.uid() != owner {
        return Err(ResolveError::Untrusted);
    }
    if kind.is_file() {
        validate_target(metadata, owner)?;
    }
    Ok(())
}

fn validate_target(metadata: &Metadata, owner: u32) -> Result<(), ResolveError> {
    if !metadata.file_type().is_file() || metadata.uid() != owner {
        return Err(ResolveError::Untrusted);
    }
    let mode = metadata.permissions().mode();
    if mode & 0o111 == 0 {
        return Err(ResolveError::NonExecutable);
    }
    if mode & 0o6022 != 0 {
        return Err(ResolveError::Untrusted);
    }
    Ok(())
}

fn validate_ancestry(path: &Path, policy: &TrustPolicy) -> Result<(), ResolveError> {
    let parent = path.parent().ok_or(ResolveError::Untrusted)?;
    if !parent.starts_with(&policy.anchor) {
        return Err(ResolveError::Untrusted);
    }
    let mut current = Some(parent);
    let mut reached_anchor = false;
    while let Some(component) = current {
        let metadata = std::fs::symlink_metadata(component).map_err(|_| ResolveError::Untrusted)?;
        if metadata.uid() != policy.owner {
            return Err(ResolveError::Untrusted);
        }
        if metadata.file_type().is_dir() {
            if metadata.permissions().mode() & 0o022 != 0 {
                return Err(ResolveError::Untrusted);
            }
        } else if !metadata.file_type().is_symlink() {
            return Err(ResolveError::Untrusted);
        }
        if component == policy.anchor {
            reached_anchor = true;
            break;
        }
        current = component.parent();
    }
    if !reached_anchor {
        return Err(ResolveError::Untrusted);
    }
    Ok(())
}

fn install_child_hardening(
    command: &mut Command,
    no_new_privileges: bool,
    inherited_read_only_fd: Option<i32>,
) {
    // Linux PDEATHSIG follows the thread that calls spawn, not the lifetime of
    // all threads in this process. A long-lived child therefore needs a spawn
    // owner thread that remains alive until child shutdown; handing the child
    // out of a short-lived provisioning/recovery thread does not transfer this
    // kernel lifetime relationship. Keep this safeguard when repairing owners.
    // https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html
    // SAFETY: only async-signal-safe syscalls run between fork and exec. No
    // allocation, locking, or process-global Rust state is accessed there.
    unsafe {
        let expected_parent = libc::getpid();
        command.pre_exec(move || {
            if no_new_privileges && libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != expected_parent {
                return Err(io::Error::from_raw_os_error(libc::ECHILD));
            }
            libc::umask(0o077);
            let rc = libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            );
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
            if let Some(fd) = inherited_read_only_fd {
                if libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

/// Apply the exact production resident-worker parent-death contract to a
/// caller-built command. Production launches reach this through
/// [`VerifiedArtifactExecutable::command`] and `spawn_with_control`; engine
/// regressions use this entry point to reproduce the same kernel
/// thread-lifetime relationship without a root-owned trust anchor.
pub fn install_resident_worker_lifetime_hardening(command: &mut Command) {
    install_child_hardening(command, false, None);
}

fn ensure_close_range_support() -> Result<(), ResolveError> {
    // This no-op range checks kernel support without changing any live file
    // descriptor. Recovery helpers fail closed when the hardening primitive is
    // unavailable instead of silently inheriting an unknown descriptor set.
    // SAFETY: close_range has no pointer arguments; this range cannot name an
    // open descriptor on Linux, where valid descriptors are signed integers.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_close_range,
            u32::MAX,
            u32::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(ResolveError::UnsupportedPlatform)
    }
}

/// Poll a contained child. If its leader exited, any residual same-group
/// descendant is killed before the exit status is returned.
pub fn try_wait_contained(
    child: &mut Child,
    deadline: Instant,
) -> Result<Option<ExitStatus>, ProcessError> {
    let group = rustix::process::Pid::from_child(child);
    match child.try_wait().map_err(ProcessError::Wait)? {
        Some(status) => {
            if group_is_absent(group)? {
                Ok(Some(status))
            } else {
                terminate_residual_group(group, deadline)?;
                Err(ProcessError::UnexpectedDescendant)
            }
        }
        None => Ok(None),
    }
}

/// Kill the complete helper process group and boundedly reap its direct child.
pub fn terminate_group_and_reap(child: &mut Child, deadline: Instant) -> Result<(), ProcessError> {
    let group = rustix::process::Pid::from_child(child);
    let mut reaped = child.try_wait().map_err(ProcessError::Wait)?.is_some();
    signal_group_if_present(group)?;
    loop {
        if !reaped {
            reaped = child.try_wait().map_err(ProcessError::Wait)?.is_some();
        }
        let group_absent = group_is_absent(group)?;
        if reaped && group_absent {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(ProcessError::TerminationTimeout);
        }
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn terminate_residual_group(
    group: rustix::process::Pid,
    deadline: Instant,
) -> Result<(), ProcessError> {
    if group_is_absent(group)? {
        return Ok(());
    }
    signal_group_if_present(group)?;
    loop {
        if group_is_absent(group)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(ProcessError::TerminationTimeout);
        }
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn signal_group_if_present(group: rustix::process::Pid) -> Result<(), ProcessError> {
    match rustix::process::kill_process_group(group, rustix::process::Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(ProcessError::Signal(io::Error::from_raw_os_error(
            error.raw_os_error(),
        ))),
    }
}

fn group_is_absent(group: rustix::process::Pid) -> Result<bool, ProcessError> {
    match rustix::process::test_kill_process_group(group) {
        Ok(()) => Ok(false),
        Err(rustix::io::Errno::SRCH) => Ok(true),
        Err(error) => Err(ProcessError::Signal(io::Error::from_raw_os_error(
            error.raw_os_error(),
        ))),
    }
}

/// Make the calling process non-dumpable (`PR_SET_DUMPABLE` 0): no core file
/// is written for it, and other processes of the same user cannot ptrace it
/// or read its private `/proc` entries (`environ`, `mem`, `fd`). Call it
/// first thing in `main`: `execve` resets the attribute for an ordinary
/// binary, so every worker the daemon starts calls it again itself.
///
/// # Errors
/// The kernel's error, which callers treat as fatal.
pub fn forbid_core_dumps() -> io::Result<()> {
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
        .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Bound for one test child, start to exit. Generous on purpose: it only
    /// caps a hang, so a slow machine makes a test slower, never failed.
    const CHILD_DEADLINE: Duration = Duration::from_secs(30);

    /// Collect a contained child's whole stdout and wait for it to exit, both
    /// under one deadline (F-M16). The tests used to read the pipe to EOF
    /// before their deadline loop, so a hung child, or a descendant holding
    /// the pipe, blocked the test forever and the deadline never applied.
    /// Here the read runs on its own thread; a missed deadline kills the
    /// group and fails the test.
    fn collect_contained(
        child: &mut Child,
    ) -> Result<(ExitStatus, Vec<u8>), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + CHILD_DEADLINE;
        let mut stdout = child.stdout.take().ok_or("missing child stdout")?;
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = sender.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
        });
        let output = match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(read) => {
                let _ = reader.join();
                read?
            }
            Err(_) => {
                let _ = terminate_group_and_reap(child, Instant::now() + Duration::from_secs(2));
                return Err("child output did not end before the deadline".into());
            }
        };
        loop {
            match try_wait_contained(child, deadline)? {
                Some(status) => return Ok((status, output)),
                None if Instant::now() < deadline => std::thread::sleep(POLL),
                None => {
                    let _ =
                        terminate_group_and_reap(child, Instant::now() + Duration::from_secs(2));
                    return Err("child did not exit before the deadline".into());
                }
            }
        }
    }

    const NON_DUMPABLE_CHILD: &str = "NARRIVY_TEST_NON_DUMPABLE_CHILD";

    /// `forbid_core_dumps` in a child copy of this test binary: the child
    /// reports itself non-dumpable, another process of the same user is
    /// refused its `environ`, and its `status` (what the resource sampler
    /// reads) stays readable.
    #[test]
    fn forbid_core_dumps_hides_the_process_from_the_same_user() -> TestResult {
        if std::env::var_os(NON_DUMPABLE_CHILD).is_some() {
            forbid_core_dumps()?;
            let behavior = rustix::process::dumpable_behavior()?;
            if behavior != rustix::process::DumpableBehavior::NotDumpable {
                return Err(format!("still dumpable: {behavior:?}").into());
            }
            println!("ready");
            let mut rest = Vec::new();
            io::stdin().read_to_end(&mut rest)?;
            return Ok(());
        }
        let mut child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "tests::forbid_core_dumps_hides_the_process_from_the_same_user",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(NON_DUMPABLE_CHILD, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let deadline = Instant::now() + CHILD_DEADLINE;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        // Read up to "ready" on a helper thread so a child that never gets
        // there fails the test at the deadline instead of hanging it.
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut seen = String::new();
            let mut byte = [0_u8; 1];
            while !seen.contains("ready\n") {
                match stdout.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => seen.push(char::from(byte[0])),
                }
            }
            let _ = sender.send(seen);
            // Keep the pipe open and drained until the child ends: closing
            // it early would fail the child's own later output.
            let _ = io::copy(&mut stdout, &mut io::sink());
        });
        let Ok(seen) = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the child never reported ready".into());
        };
        let pid = child.id();
        let environ = std::fs::read(format!("/proc/{pid}/environ"));
        let status = std::fs::read_to_string(format!("/proc/{pid}/status"));
        drop(child.stdin.take());
        let exit = loop {
            if let Some(exit) = child.try_wait()? {
                break exit;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the child did not exit before the deadline".into());
            }
            std::thread::sleep(POLL);
        };
        assert!(seen.contains("ready\n"), "child output: {seen:?}");
        assert!(exit.success(), "child failed: {exit:?}");
        if !rustix::process::geteuid().is_root() {
            let error = environ.err().ok_or("environ was readable")?;
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        }
        assert!(status?.contains("VmRSS:"));
        Ok(())
    }

    /// Resident worker parent-death hardening must follow the thread that
    /// calls `spawn`: a worker created by a bounded provisioning or recovery
    /// thread is killed when that thread exits. The engine's persistent spawn
    /// owner exists precisely because of this kernel contract, so the
    /// regression that pins the contract lives next to the hardening
    /// primitive.
    #[test]
    fn resident_lifetime_hardening_binds_child_to_the_spawning_thread() -> TestResult {
        let spawner = std::thread::spawn(|| {
            let mut command = Command::new("/bin/sleep");
            command
                .arg("30")
                .env_clear()
                .current_dir("/")
                .process_group(0)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            install_resident_worker_lifetime_hardening(&mut command);
            let child = command.spawn()?;
            // Let the child reach a steady running state before this owner
            // thread returns; the parent-death signal binds to this thread.
            std::thread::sleep(std::time::Duration::from_millis(250));
            Ok::<_, std::io::Error>(child)
        });
        let mut child = spawner
            .join()
            .map_err(|_| std::io::Error::other("spawner thread panicked"))??;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait()? {
                assert_eq!(
                    status.signal(),
                    Some(libc::SIGKILL),
                    "hardened child did not receive the parent-death signal"
                );
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    std::io::Error::other("hardened child outlived its spawning thread").into(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn test_policy(anchor: &Path) -> TrustPolicy {
        TrustPolicy {
            // SAFETY: geteuid has no preconditions and only reads process credentials.
            owner: unsafe { libc::geteuid() },
            anchor: anchor.to_path_buf(),
        }
    }

    fn executable(path: &Path, body: &str) -> TestResult {
        std::fs::write(path, body)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    fn copy_native_artifact(
        source: &Path,
        destination: &Path,
    ) -> Result<(String, u64), Box<dyn std::error::Error>> {
        let bytes = std::fs::read(source)?;
        std::fs::write(destination, &bytes)?;
        std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o700))?;
        Ok((digest_hex(&bytes), u64::try_from(bytes.len())?))
    }

    fn digest_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    }

    #[test]
    fn controlled_artifact_rejects_missing_wrong_digest_size_and_unsafe_mode() -> TestResult {
        let dir = tempfile::tempdir()?;
        let missing = dir.path().join("missing");
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(&missing, &"0".repeat(64), 1),
            Err(ArtifactError::Missing)
        ));

        let artifact = dir.path().join("worker");
        let (digest, size) = copy_native_artifact(Path::new("/bin/true"), &artifact)?;
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(&artifact, "", size),
            Err(ArtifactError::InvalidDigest)
        ));
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(&artifact, &"0".repeat(64), size),
            Err(ArtifactError::DigestMismatch)
        ));
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(&artifact, &digest, size + 1),
            Err(ArtifactError::SizeMismatch)
        ));
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(
                &artifact,
                &digest,
                MAX_CONTROLLED_EXECUTABLE_BYTES + 1
            ),
            Err(ArtifactError::InvalidSize)
        ));
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o722))?;
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(&artifact, &digest, size),
            Err(ArtifactError::Untrusted)
        ));
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o4700))?;
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(&artifact, &digest, size),
            Err(ArtifactError::Untrusted)
        ));
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o700))?;
        let link = dir.path().join("worker-link");
        std::os::unix::fs::symlink(&artifact, &link)?;
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(&link, &digest, size),
            Err(ArtifactError::Untrusted)
        ));
        Ok(())
    }

    #[test]
    fn trust_anchored_artifact_requires_exact_owner_mode_and_nofollow_tree() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let trusted = dir.path().join("trusted");
        std::fs::create_dir(&trusted)?;
        std::fs::set_permissions(&trusted, std::fs::Permissions::from_mode(0o755))?;
        let artifact = trusted.join("worker");
        let (digest, size) = copy_native_artifact(Path::new("/bin/true"), &artifact)?;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755))?;

        let bound = VerifiedArtifactExecutable::bind_trust_anchored(
            &artifact,
            &digest,
            size,
            dir.path(),
            owner,
        )?;
        assert_eq!(
            bound.provenance(),
            ArtifactProvenance::TrustAnchorDigestBound
        );

        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o750))?;
        assert!(matches!(
            VerifiedArtifactExecutable::bind_trust_anchored(
                &artifact,
                &digest,
                size,
                dir.path(),
                owner,
            ),
            Err(ArtifactError::Untrusted)
        ));
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755))?;
        std::fs::set_permissions(&trusted, std::fs::Permissions::from_mode(0o777))?;
        assert!(matches!(
            VerifiedArtifactExecutable::bind_trust_anchored(
                &artifact,
                &digest,
                size,
                dir.path(),
                owner,
            ),
            Err(ArtifactError::Untrusted)
        ));
        std::fs::set_permissions(&trusted, std::fs::Permissions::from_mode(0o755))?;

        let real = dir.path().join("real");
        std::fs::create_dir(&real)?;
        let through_link = dir.path().join("linked");
        std::os::unix::fs::symlink(&real, &through_link)?;
        let linked_artifact = real.join("worker");
        std::fs::copy("/bin/true", &linked_artifact)?;
        std::fs::set_permissions(&linked_artifact, std::fs::Permissions::from_mode(0o755))?;
        assert!(matches!(
            VerifiedArtifactExecutable::bind_trust_anchored(
                &through_link.join("worker"),
                &digest,
                size,
                dir.path(),
                owner,
            ),
            Err(ArtifactError::Untrusted)
        ));
        Ok(())
    }

    #[test]
    fn trust_anchored_spawn_cancels_during_digest_revalidation_without_launch() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let artifact = dir.path().join("worker");
        let (digest, size) = copy_native_artifact(Path::new("/bin/sh"), &artifact)?;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755))?;
        let bound = VerifiedArtifactExecutable::bind_trust_anchored(
            &artifact,
            &digest,
            size,
            dir.path(),
            owner,
        )?;
        assert_eq!(
            bound.provenance(),
            ArtifactProvenance::TrustAnchorDigestBound
        );
        let marker = dir.path().join("must-not-launch");
        let cancellation = SessionCancellation::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut command =
            bound.command_with_control(HelperEnvironment::ResidentAsr, &cancellation, deadline)?;
        command
            .args([
                OsStr::new("-c"),
                OsStr::new("printf launched > \"$1\""),
                OsStr::new("narrivy-controlled-test"),
                marker.as_os_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let worker_cancellation = cancellation.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let control = ArtifactControl {
                cancellation: &worker_cancellation,
                deadline,
                now: &Instant::now,
            };
            let mut paused = false;
            command.spawn_inner(Some(&control), &mut |checkpoint| {
                if !paused
                    && matches!(checkpoint, ArtifactCheckpoint::RevalidatedChunk(bytes) if bytes > 0)
                {
                    paused = true;
                    let _ = entered_tx.send(());
                    let _ = resume_rx.recv();
                }
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(2))?;
        cancellation.cancel();
        resume_tx.send(())?;
        let result = worker
            .join()
            .map_err(|_| "controlled executable spawn thread panicked")?;
        if let Ok(mut child) = result {
            let _ = terminate_group_and_reap(&mut child, Instant::now() + Duration::from_secs(2));
            return Err("cancelled digest revalidation launched a child".into());
        }
        assert!(matches!(
            result,
            Err(ArtifactLaunchError::Artifact(ArtifactError::Cancelled))
        ));
        assert!(!marker.exists());
        Ok(())
    }

    #[test]
    fn trust_anchored_spawn_times_out_during_digest_revalidation() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let artifact = dir.path().join("worker");
        let (digest, size) = copy_native_artifact(Path::new("/bin/true"), &artifact)?;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755))?;
        let bound = VerifiedArtifactExecutable::bind_trust_anchored(
            &artifact,
            &digest,
            size,
            dir.path(),
            owner,
        )?;
        let cancellation = SessionCancellation::new();
        let command = bound.command_with_control(
            HelperEnvironment::ResidentAsr,
            &cancellation,
            Instant::now() + Duration::from_secs(5),
        )?;
        let before_deadline = Instant::now();
        let deadline = before_deadline + Duration::from_secs(5);
        let expired = AtomicBool::new(false);
        let now = || {
            if expired.load(Ordering::SeqCst) {
                deadline
            } else {
                before_deadline
            }
        };
        let control = ArtifactControl {
            cancellation: &cancellation,
            deadline,
            now: &now,
        };
        let result = command.spawn_inner(Some(&control), &mut |checkpoint| {
            if matches!(checkpoint, ArtifactCheckpoint::RevalidatedChunk(bytes) if bytes > 0) {
                expired.store(true, Ordering::SeqCst);
            }
        });
        assert!(matches!(
            result,
            Err(ArtifactLaunchError::Artifact(ArtifactError::TimedOut))
        ));
        Ok(())
    }

    #[test]
    fn trust_anchored_spawn_rechecks_deadline_at_final_launch_boundary() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let artifact = dir.path().join("worker");
        let (digest, size) = copy_native_artifact(Path::new("/bin/sh"), &artifact)?;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755))?;
        let bound = VerifiedArtifactExecutable::bind_trust_anchored(
            &artifact,
            &digest,
            size,
            dir.path(),
            owner,
        )?;
        let marker = dir.path().join("must-not-launch");
        let cancellation = SessionCancellation::new();
        let mut command = bound.command_with_control(
            HelperEnvironment::ResidentAsr,
            &cancellation,
            Instant::now() + Duration::from_secs(5),
        )?;
        command
            .args([
                OsStr::new("-c"),
                OsStr::new("printf launched > \"$1\""),
                OsStr::new("narrivy-controlled-test"),
                marker.as_os_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let before_deadline = Instant::now();
        let deadline = before_deadline + Duration::from_secs(5);
        let expired = AtomicBool::new(false);
        let now = || {
            if expired.load(Ordering::SeqCst) {
                deadline
            } else {
                before_deadline
            }
        };
        let control = ArtifactControl {
            cancellation: &cancellation,
            deadline,
            now: &now,
        };
        let result = command.spawn_inner(Some(&control), &mut |checkpoint| {
            if checkpoint == ArtifactCheckpoint::BeforeSpawn {
                expired.store(true, Ordering::SeqCst);
            }
        });
        assert!(matches!(
            result,
            Err(ArtifactLaunchError::Artifact(ArtifactError::TimedOut))
        ));
        assert!(!marker.exists());

        cancellation.cancel();
        let errors = [ArtifactError::Cancelled, ArtifactError::TimedOut];
        for error in errors {
            let rendered = error.to_string();
            assert!(!rendered.contains(&artifact.display().to_string()));
            assert!(!rendered.contains(&digest));
        }
        assert_eq!(
            bound.revalidate_with_control(&cancellation, Instant::now()),
            Err(ArtifactError::Cancelled),
            "cancellation must win when cancellation and expiry coincide"
        );
        Ok(())
    }

    #[test]
    fn controlled_data_is_immutable_digest_bound_and_inherited_only_by_request() -> TestResult {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("model.gguf");
        let original = b"GGUF\x03\0\0\0controlled-model";
        std::fs::write(&source, original)?;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600))?;
        let digest = digest_hex(original);
        let size = u64::try_from(original.len())?;
        let data = VerifiedArtifactData::bind_controlled(&source, &digest, size)?;
        assert_eq!(data.provenance(), ArtifactProvenance::ControlledBuild);
        assert_eq!(data.size(), size);
        assert_eq!(data.sha256(), parse_data_sha256(&digest)?);
        assert_eq!(
            rustix::fs::fcntl_get_seals(data.snapshot.as_ref())?,
            data_seals()
        );
        assert_eq!(
            data.snapshot.metadata()?.permissions().mode() & 0o7777,
            0o400
        );
        let mut writable = data.snapshot.try_clone()?;
        assert!(writable.write_all(b"x").is_err());
        assert!(data.snapshot.set_len(size.saturating_add(1)).is_err());

        let drift = VerifiedArtifactData::bind_controlled(&source, &digest, size)?;
        std::fs::set_permissions(
            proc_fd_path(drift.snapshot.as_raw_fd()),
            std::fs::Permissions::from_mode(0o600),
        )?;
        assert_eq!(
            drift.revalidate(),
            Err(DataArtifactError::IdentityChanged),
            "data snapshot mode drift was accepted"
        );

        std::fs::write(&source, b"GGUF\x03\0\0\0replacement-bytes")?;
        std::fs::remove_file(&source)?;
        data.revalidate()?;

        let executable_path = dir.path().join("cat");
        let (executable_digest, executable_size) =
            copy_native_artifact(Path::new("/bin/cat"), &executable_path)?;
        let executable = VerifiedArtifactExecutable::bind_controlled(
            &executable_path,
            &executable_digest,
            executable_size,
        )?;
        let mut command = executable.command(HelperEnvironment::ResidentAsr)?;
        command
            .inherited_read_only_arg(&data)?
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let (status, output) = collect_contained(&mut child)?;
        assert!(status.success());
        assert_eq!(output, original);
        Ok(())
    }

    #[test]
    fn trust_anchored_data_is_descriptor_bound_and_rejects_symlink_components() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let models = dir.path().join("models");
        std::fs::create_dir(&models)?;
        std::fs::set_permissions(&models, std::fs::Permissions::from_mode(0o700))?;
        let model = models.join("model.gguf");
        let bytes = b"GGUF\x03\0\0\0trust-anchored-model";
        std::fs::write(&model, bytes)?;
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600))?;
        let bound = VerifiedArtifactData::bind_trust_anchored(
            &model,
            &digest_hex(bytes),
            u64::try_from(bytes.len())?,
            dir.path(),
            owner,
        )?;
        assert_eq!(
            bound.provenance(),
            ArtifactProvenance::TrustAnchorDigestBound
        );
        std::fs::write(&model, b"replacement")?;
        std::fs::remove_file(&model)?;
        bound.revalidate()?;

        let real = dir.path().join("real-models");
        std::fs::create_dir(&real)?;
        let linked = dir.path().join("linked-models");
        std::os::unix::fs::symlink(&real, &linked)?;
        let through_link = real.join("model.gguf");
        std::fs::write(&through_link, bytes)?;
        std::fs::set_permissions(&through_link, std::fs::Permissions::from_mode(0o600))?;
        assert!(matches!(
            VerifiedArtifactData::bind_trust_anchored(
                &linked.join("model.gguf"),
                &digest_hex(bytes),
                u64::try_from(bytes.len())?,
                dir.path(),
                owner,
            ),
            Err(DataArtifactError::Untrusted)
        ));
        Ok(())
    }

    #[test]
    fn trust_anchored_data_cancellation_mid_ancestry_has_priority() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let first = dir.path().join("first");
        let second = first.join("second");
        std::fs::create_dir_all(&second)?;
        std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&second, std::fs::Permissions::from_mode(0o700))?;
        let model = second.join("model.gguf");
        let bytes = b"ancestry-cancellation";
        std::fs::write(&model, bytes)?;
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600))?;

        let cancellation = SessionCancellation::new();
        let before_deadline = Instant::now();
        let deadline = before_deadline + Duration::from_secs(5);
        let forced_expiry = AtomicBool::new(false);
        let now = || {
            if forced_expiry.load(Ordering::SeqCst) {
                deadline
            } else {
                before_deadline
            }
        };
        let control = DataSnapshotControl {
            cancellation: &cancellation,
            deadline,
            now: &now,
        };
        let mut reached_component = false;
        let result = VerifiedArtifactData::bind_trust_anchored_inner(
            &model,
            &digest_hex(bytes),
            u64::try_from(bytes.len())?,
            dir.path(),
            owner,
            Some(&control),
            |checkpoint| {
                if checkpoint == DataSnapshotCheckpoint::AncestryComponent(1) {
                    reached_component = true;
                    forced_expiry.store(true, Ordering::SeqCst);
                    cancellation.cancel();
                }
            },
        );

        assert!(reached_component, "ancestry checkpoint was not reached");
        assert!(matches!(result, Err(DataArtifactError::Cancelled)));
        Ok(())
    }

    #[test]
    fn trust_anchored_data_deadline_expiry_mid_ancestry_is_typed() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let first = dir.path().join("first");
        let second = first.join("second");
        std::fs::create_dir_all(&second)?;
        std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&second, std::fs::Permissions::from_mode(0o700))?;
        let model = second.join("model.gguf");
        let bytes = b"ancestry-deadline";
        std::fs::write(&model, bytes)?;
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600))?;

        let cancellation = SessionCancellation::new();
        let before_deadline = Instant::now();
        let deadline = before_deadline + Duration::from_secs(5);
        let forced_expiry = AtomicBool::new(false);
        let now = || {
            if forced_expiry.load(Ordering::SeqCst) {
                deadline
            } else {
                before_deadline
            }
        };
        let control = DataSnapshotControl {
            cancellation: &cancellation,
            deadline,
            now: &now,
        };
        let mut reached_component = false;
        let result = VerifiedArtifactData::bind_trust_anchored_inner(
            &model,
            &digest_hex(bytes),
            u64::try_from(bytes.len())?,
            dir.path(),
            owner,
            Some(&control),
            |checkpoint| {
                if checkpoint == DataSnapshotCheckpoint::AncestryComponent(1) {
                    reached_component = true;
                    forced_expiry.store(true, Ordering::SeqCst);
                }
            },
        );

        assert!(reached_component, "ancestry checkpoint was not reached");
        assert!(matches!(result, Err(DataArtifactError::TimedOut)));
        Ok(())
    }

    #[test]
    fn trust_anchored_data_cancellation_mid_copy_cannot_report_success() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let model = dir.path().join("model.gguf");
        let bytes = vec![0x5a; 3 * 64 * 1024];
        std::fs::write(&model, &bytes)?;
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600))?;
        let digest = digest_hex(&bytes);
        let size = u64::try_from(bytes.len())?;
        let anchor = dir.path().to_path_buf();
        let cancellation = SessionCancellation::new();
        let thread_cancellation = cancellation.clone();
        let (reached_tx, reached_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);

        let worker = std::thread::spawn(move || {
            let control = DataSnapshotControl {
                cancellation: &thread_cancellation,
                deadline: Instant::now() + Duration::from_secs(5),
                now: &Instant::now,
            };
            let mut paused = false;
            VerifiedArtifactData::bind_trust_anchored_inner(
                &model,
                &digest,
                size,
                &anchor,
                owner,
                Some(&control),
                |checkpoint| {
                    if !paused && matches!(checkpoint, DataSnapshotCheckpoint::CopiedChunk(_)) {
                        paused = true;
                        reached_tx.send(()).ok();
                        release_rx.recv().ok();
                    }
                },
            )
        });

        reached_rx.recv()?;
        cancellation.cancel();
        release_tx.send(())?;
        assert!(matches!(
            worker.join().map_err(|_| "snapshot worker panicked")?,
            Err(DataArtifactError::Cancelled)
        ));
        Ok(())
    }

    #[test]
    fn trust_anchored_data_cancellation_after_copy_cannot_report_success() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let model = dir.path().join("model.gguf");
        let bytes = vec![0x33; 64 * 1024 + 17];
        std::fs::write(&model, &bytes)?;
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600))?;
        let digest = digest_hex(&bytes);
        let size = u64::try_from(bytes.len())?;
        let anchor = dir.path().to_path_buf();
        let cancellation = SessionCancellation::new();
        let thread_cancellation = cancellation.clone();
        let (reached_tx, reached_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);

        let worker = std::thread::spawn(move || {
            let control = DataSnapshotControl {
                cancellation: &thread_cancellation,
                deadline: Instant::now() + Duration::from_secs(5),
                now: &Instant::now,
            };
            VerifiedArtifactData::bind_trust_anchored_inner(
                &model,
                &digest,
                size,
                &anchor,
                owner,
                Some(&control),
                |checkpoint| {
                    if checkpoint == DataSnapshotCheckpoint::CopyComplete {
                        reached_tx.send(()).ok();
                        release_rx.recv().ok();
                    }
                },
            )
        });

        reached_rx.recv()?;
        cancellation.cancel();
        release_tx.send(())?;
        assert!(matches!(
            worker.join().map_err(|_| "snapshot worker panicked")?,
            Err(DataArtifactError::Cancelled)
        ));
        Ok(())
    }

    #[test]
    fn trust_anchored_data_expiry_and_cancellation_are_typed_and_content_free() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let model = dir.path().join("secret-model-name.gguf");
        let bytes = b"bounded-model";
        std::fs::write(&model, bytes)?;
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600))?;
        let digest = digest_hex(bytes);
        let size = u64::try_from(bytes.len())?;

        let live = SessionCancellation::new();
        let expired = VerifiedArtifactData::bind_trust_anchored_with_control(
            &model,
            &digest,
            size,
            dir.path(),
            owner,
            &live,
            Instant::now(),
        );
        assert!(matches!(expired, Err(DataArtifactError::TimedOut)));

        let cancelled = SessionCancellation::new();
        cancelled.cancel();
        let cancelled_result = VerifiedArtifactData::bind_trust_anchored_with_control(
            &model,
            &digest,
            size,
            dir.path(),
            owner,
            &cancelled,
            Instant::now(),
        );
        assert!(matches!(
            cancelled_result,
            Err(DataArtifactError::Cancelled)
        ));
        for error in [DataArtifactError::Cancelled, DataArtifactError::TimedOut] {
            assert!(!error.to_string().contains("secret-model-name"));
        }
        Ok(())
    }

    #[test]
    fn trust_anchored_data_expiry_mid_copy_cannot_report_success() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let model = dir.path().join("model.gguf");
        let bytes = vec![0x71; 3 * 64 * 1024];
        std::fs::write(&model, &bytes)?;
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600))?;
        let digest = digest_hex(&bytes);
        let size = u64::try_from(bytes.len())?;
        let anchor = dir.path().to_path_buf();
        let cancellation = SessionCancellation::new();
        let forced_expiry = Arc::new(AtomicBool::new(false));
        let thread_expiry = Arc::clone(&forced_expiry);
        let (reached_tx, reached_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);

        let worker = std::thread::spawn(move || {
            let before_deadline = Instant::now();
            let deadline = before_deadline + Duration::from_secs(5);
            let now = || {
                if thread_expiry.load(Ordering::SeqCst) {
                    deadline
                } else {
                    before_deadline
                }
            };
            let control = DataSnapshotControl {
                cancellation: &cancellation,
                deadline,
                now: &now,
            };
            let mut paused = false;
            VerifiedArtifactData::bind_trust_anchored_inner(
                &model,
                &digest,
                size,
                &anchor,
                owner,
                Some(&control),
                |checkpoint| {
                    if !paused && matches!(checkpoint, DataSnapshotCheckpoint::CopiedChunk(_)) {
                        paused = true;
                        reached_tx.send(()).ok();
                        release_rx.recv().ok();
                    }
                },
            )
        });

        reached_rx.recv()?;
        forced_expiry.store(true, Ordering::SeqCst);
        release_tx.send(())?;
        assert!(matches!(
            worker.join().map_err(|_| "snapshot worker panicked")?,
            Err(DataArtifactError::TimedOut)
        ));
        Ok(())
    }

    #[test]
    fn bounded_trust_anchored_data_returns_a_sealed_immutable_snapshot() -> TestResult {
        let dir = tempfile::tempdir()?;
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let model = dir.path().join("model.gguf");
        let bytes = b"sealed-bounded-model";
        std::fs::write(&model, bytes)?;
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600))?;
        let cancellation = SessionCancellation::new();
        let bound = VerifiedArtifactData::bind_trust_anchored_with_control(
            &model,
            &digest_hex(bytes),
            u64::try_from(bytes.len())?,
            dir.path(),
            owner,
            &cancellation,
            Instant::now() + Duration::from_secs(2),
        )?;

        assert_eq!(
            bound.provenance(),
            ArtifactProvenance::TrustAnchorDigestBound
        );
        assert_eq!(
            rustix::fs::fcntl_get_seals(bound.snapshot.as_ref())?,
            data_seals()
        );
        std::fs::write(&model, b"source replacement")?;
        std::fs::remove_file(&model)?;
        bound.revalidate()?;
        let mut writable = bound.snapshot.try_clone()?;
        assert!(writable.write_all(b"x").is_err());
        Ok(())
    }

    #[test]
    fn controlled_data_spawn_inherits_only_the_selected_descriptor() -> TestResult {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("model.gguf");
        let bytes = b"GGUF\x03\0\0\0descriptor-probe";
        std::fs::write(&source, bytes)?;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600))?;
        let data = VerifiedArtifactData::bind_controlled(
            &source,
            &digest_hex(bytes),
            u64::try_from(bytes.len())?,
        )?;

        let executable_path = dir.path().join("shell");
        let (executable_digest, executable_size) =
            copy_native_artifact(Path::new("/bin/sh"), &executable_path)?;
        let executable = VerifiedArtifactExecutable::bind_controlled(
            &executable_path,
            &executable_digest,
            executable_size,
        )?;

        // Deliberately make an unrelated parent descriptor inheritable. The
        // child hardening boundary must still close it while admitting only
        // the selected immutable data descriptor.
        let unrelated = File::open("/dev/null")?;
        if unsafe { libc::fcntl(unrelated.as_raw_fd(), libc::F_SETFD, 0) } != 0 {
            return Err(io::Error::last_os_error().into());
        }

        let mut command = executable.command(HelperEnvironment::ResidentAsr)?;
        command
            .args([
                "-c",
                "for path in /proc/self/fd/*; do fd=${path##*/}; if [ \"$fd\" -ge 3 ] && [ -e \"$path\" ]; then printf 'fd=%s\\n' \"$fd\"; fi; done; printf 'arg=%s\\n' \"$1\"",
                "narrivy-fd-probe",
            ])
            .inherited_read_only_arg(&data)?
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let (status, output) = collect_contained(&mut child)?;
        assert!(status.success());
        let output = String::from_utf8(output)?;

        let mut lines = output.lines();
        let inherited = lines
            .next()
            .ok_or("selected descriptor was not inherited")?;
        let argument = lines
            .next()
            .ok_or("selected descriptor argument was absent")?;
        assert!(lines.next().is_none(), "unrelated descriptor was inherited");
        let fd = inherited
            .strip_prefix("fd=")
            .ok_or("malformed inherited descriptor")?;
        assert_eq!(argument, format!("arg=/proc/self/fd/{fd}"));
        assert!(fd.parse::<i32>()? >= INHERITED_DATA_FD_MIN);
        Ok(())
    }

    #[test]
    fn controlled_data_rejects_bad_identity_and_duplicate_inheritance() -> TestResult {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("model.gguf");
        let bytes = b"GGUF\x03\0\0\0model";
        std::fs::write(&source, bytes)?;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600))?;
        let digest = digest_hex(bytes);
        let size = u64::try_from(bytes.len())?;
        assert!(matches!(
            VerifiedArtifactData::bind_controlled(&source, "", size),
            Err(DataArtifactError::InvalidDigest)
        ));
        assert!(matches!(
            VerifiedArtifactData::bind_controlled(&source, &"0".repeat(64), size),
            Err(DataArtifactError::DigestMismatch)
        ));
        assert!(matches!(
            VerifiedArtifactData::bind_controlled(&source, &digest, size + 1),
            Err(DataArtifactError::SizeMismatch)
        ));
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o622))?;
        assert!(matches!(
            VerifiedArtifactData::bind_controlled(&source, &digest, size),
            Err(DataArtifactError::Untrusted)
        ));
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600))?;
        let data = VerifiedArtifactData::bind_controlled(&source, &digest, size)?;

        let executable_path = dir.path().join("true");
        let (executable_digest, executable_size) =
            copy_native_artifact(Path::new("/bin/true"), &executable_path)?;
        let executable = VerifiedArtifactExecutable::bind_controlled(
            &executable_path,
            &executable_digest,
            executable_size,
        )?;
        let mut command = executable.command(HelperEnvironment::ResidentAsr)?;
        command.inherited_read_only_arg(&data)?;
        assert!(matches!(
            command.inherited_read_only_arg(&data),
            Err(DataArtifactError::Untrusted)
        ));
        Ok(())
    }

    #[test]
    fn controlled_artifact_rejects_malformed_and_wrong_architecture_elf() -> TestResult {
        let dir = tempfile::tempdir()?;
        let source = std::fs::read("/bin/true")?;
        for (name, mut bytes) in [
            ("wrong-class", source.clone()),
            ("wrong-endian", source.clone()),
            ("wrong-version", source.clone()),
            ("wrong-type", source.clone()),
            ("wrong-machine", source.clone()),
        ] {
            match name {
                "wrong-class" => bytes[4] = if size_of::<usize>() == 8 { 1 } else { 2 },
                "wrong-endian" => bytes[5] = if cfg!(target_endian = "little") { 2 } else { 1 },
                "wrong-version" => bytes[6] = 0,
                "wrong-type" => bytes[16..18].copy_from_slice(&0_u16.to_le_bytes()),
                "wrong-machine" => bytes[18..20].copy_from_slice(&u16::MAX.to_le_bytes()),
                _ => unreachable!(),
            }
            let artifact = dir.path().join(name);
            std::fs::write(&artifact, &bytes)?;
            std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o700))?;
            assert!(matches!(
                VerifiedArtifactExecutable::bind_controlled(
                    &artifact,
                    &digest_hex(&bytes),
                    u64::try_from(bytes.len())?
                ),
                Err(ArtifactError::NotNativeElf)
            ));
        }

        let truncated = source[..ELF64_HEADER_BYTES.min(source.len())].to_vec();
        let artifact = dir.path().join("truncated-polyglot");
        std::fs::write(&artifact, &truncated)?;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o700))?;
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(
                &artifact,
                &digest_hex(&truncated),
                u64::try_from(truncated.len())?
            ),
            Err(ArtifactError::NotNativeElf)
        ));

        let mut no_executable_load = source;
        if size_of::<usize>() == 8 {
            let phoff = u64::from_le_bytes(no_executable_load[32..40].try_into()?) as usize;
            let phentsize = u16::from_le_bytes(no_executable_load[54..56].try_into()?) as usize;
            let phnum = u16::from_le_bytes(no_executable_load[56..58].try_into()?) as usize;
            for index in 0..phnum {
                let offset = phoff + index * phentsize;
                if u32::from_le_bytes(no_executable_load[offset..offset + 4].try_into()?) == 1 {
                    no_executable_load[offset + 4..offset + 8]
                        .copy_from_slice(&4_u32.to_le_bytes());
                }
            }
        }
        let artifact = dir.path().join("no-executable-load");
        std::fs::write(&artifact, &no_executable_load)?;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o700))?;
        assert!(matches!(
            VerifiedArtifactExecutable::bind_controlled(
                &artifact,
                &digest_hex(&no_executable_load),
                u64::try_from(no_executable_load.len())?
            ),
            Err(ArtifactError::NotNativeElf)
        ));
        Ok(())
    }

    #[test]
    fn sealed_snapshot_rejects_every_mutation_and_rehashes_exact_bytes() -> TestResult {
        let dir = tempfile::tempdir()?;
        let artifact = dir.path().join("worker");
        let (digest, size) = copy_native_artifact(Path::new("/bin/true"), &artifact)?;
        let bound = VerifiedArtifactExecutable::bind_controlled(&artifact, &digest, size)?;
        assert_eq!(
            rustix::fs::fcntl_get_seals(bound.snapshot.as_ref())?,
            artifact_seals()
        );
        assert_eq!(bound.snapshot.metadata()?.len(), size);
        assert_eq!(bound.sha256(), parse_sha256(&digest)?);
        bound.revalidate()?;

        let mut writable = bound.snapshot.try_clone()?;
        assert!(writable.write_all(b"x").is_err());
        assert!(bound.snapshot.set_len(size - 1).is_err());
        assert!(bound.snapshot.set_len(size + 1).is_err());
        assert!(
            std::fs::set_permissions(
                proc_fd_path(bound.snapshot.as_raw_fd()),
                std::fs::Permissions::from_mode(0o400)
            )
            .is_err(),
            "F_SEAL_EXEC allowed execute-bit removal"
        );
        assert_eq!(
            bound.snapshot.metadata()?.permissions().mode() & 0o7777,
            0o500
        );
        bound.revalidate()?;

        // Linux F_SEAL_EXEC does not forbid adding write/set-ID mode bits
        // while retaining execute. Exact-mode revalidation must catch every
        // such drift before a later spawn.
        for mode in [0o700, 0o2500, 0o4500] {
            let rebound = VerifiedArtifactExecutable::bind_controlled(&artifact, &digest, size)?;
            std::fs::set_permissions(
                proc_fd_path(rebound.snapshot.as_raw_fd()),
                std::fs::Permissions::from_mode(mode),
            )?;
            assert_eq!(
                rebound.revalidate(),
                Err(ArtifactError::IdentityChanged),
                "snapshot mode drift {mode:o} was accepted"
            );
        }
        Ok(())
    }

    #[test]
    fn artifact_debug_is_redacted_and_child_has_no_privilege_or_fd_inheritance() -> TestResult {
        let dir = tempfile::tempdir()?;
        let artifact = dir.path().join("shell-worker");
        let (digest, size) = copy_native_artifact(Path::new("/bin/sh"), &artifact)?;
        let bound = VerifiedArtifactExecutable::bind_controlled(&artifact, &digest, size)?;
        let mut command = bound.command(HelperEnvironment::ResidentAsr)?;
        let secret = "SECRET-ARTIFACT-ARGUMENT";
        command
            .args([
                "-c",
                "while IFS= read -r line; do case \"$line\" in NoNewPrivs:*) printf '%s\\n' \"$line\";; esac; done < /proc/self/status; for fd in 3 4 5 6 7 8 9; do if [ -e \"/proc/self/fd/$fd\" ]; then printf '%s ' \"$fd\"; fi; done; printf '\\n'",
                secret,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let debug = format!("{bound:?} {command:?}");
        assert!(!debug.contains(secret));
        assert!(!debug.contains(&artifact.display().to_string()));
        assert!(!debug.contains(&digest));

        let mut child = command.spawn()?;
        let (status, output) = collect_contained(&mut child)?;
        assert!(status.success());
        let output = String::from_utf8(output)?;
        let mut lines = output.lines();
        assert_eq!(lines.next().map(str::trim), Some("NoNewPrivs:\t1"));
        assert_eq!(lines.next(), Some(""), "artifact inherited descriptor(s)");
        Ok(())
    }

    #[test]
    fn sealed_controlled_artifact_ignores_later_source_drift() -> TestResult {
        let dir = tempfile::tempdir()?;
        let artifact = dir.path().join("worker");
        let (digest, size) = copy_native_artifact(Path::new("/bin/true"), &artifact)?;
        let bound = VerifiedArtifactExecutable::bind_controlled(&artifact, &digest, size)?;
        assert_eq!(bound.provenance(), ArtifactProvenance::ControlledBuild);
        assert_ne!(
            bound.provenance(),
            ArtifactProvenance::TrustAnchorDigestBound,
            "controlled binding must never become trust-anchor provenance"
        );

        let replacement = std::fs::read("/bin/false")?;
        std::fs::write(&artifact, replacement)?;
        std::fs::remove_file(&artifact)?;
        let mut command = bound.command(HelperEnvironment::ResidentAsr)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match try_wait_contained(&mut child, deadline)? {
                Some(status) => break status,
                None if Instant::now() < deadline => std::thread::sleep(POLL),
                None => return Err("sealed artifact did not exit".into()),
            }
        };
        assert!(status.success(), "source drift changed the sealed snapshot");
        Ok(())
    }

    #[test]
    fn resident_artifact_environment_is_empty() -> TestResult {
        let dir = tempfile::tempdir()?;
        let artifact = dir.path().join("env-worker");
        let (digest, size) = copy_native_artifact(Path::new("/usr/bin/env"), &artifact)?;
        let bound = VerifiedArtifactExecutable::bind_controlled(&artifact, &digest, size)?;
        let mut command = bound.command(HelperEnvironment::ResidentAsr)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let (status, bytes) = collect_contained(&mut child)?;
        assert!(status.success());
        assert!(bytes.is_empty(), "resident artifact inherited environment");
        Ok(())
    }

    #[test]
    #[ignore = "spawned only by the sealed-artifact descendant test"]
    fn controlled_artifact_descendant_helper() {
        let _ = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    #[test]
    fn sealed_artifact_residual_descendant_is_typed_and_killed() -> TestResult {
        let source = std::env::current_exe()?;
        let dir = tempfile::tempdir()?;
        let artifact = dir.path().join("test-worker");
        let (digest, size) = copy_native_artifact(&source, &artifact)?;
        let bound = VerifiedArtifactExecutable::bind_controlled(&artifact, &digest, size)?;
        let mut command = bound.command(HelperEnvironment::ResidentAsr)?;
        command
            .args([
                "--exact",
                "tests::controlled_artifact_descendant_helper",
                "--ignored",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match try_wait_contained(&mut child, deadline) {
                Err(ProcessError::UnexpectedDescendant) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
                other => return Err(format!("unexpected artifact wait result: {other:?}").into()),
            }
        }
        Ok(())
    }

    fn resolve_test(dir: &Path, name: &str) -> Result<TrustedExecutable, ResolveError> {
        TrustedExecutable::resolve_with_policy(
            OsStr::new(name),
            [dir.to_path_buf()],
            test_policy(dir),
        )
    }

    #[test]
    fn safe_alias_is_retained_and_identity_drift_fails_closed() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let target = dir.path().join("pw-cat");
        executable(&target, "#!/bin/sh\nexit 0\n")?;
        let alias = dir.path().join("pw-record");
        std::os::unix::fs::symlink("pw-cat", &alias)?;
        let trusted = resolve_test(dir.path(), "pw-record")?;
        assert_eq!(trusted.launch_path(), alias);
        trusted.revalidate()?;
        std::fs::remove_file(&alias)?;
        std::os::unix::fs::symlink("missing", &alias)?;
        assert_eq!(trusted.revalidate(), Err(ResolveError::IdentityChanged));
        Ok(())
    }

    #[test]
    fn identity_drift_after_command_construction_fails_at_spawn() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let target = dir.path().join("pw-cat");
        executable(&target, "#!/bin/sh\nexit 0\n")?;
        let alias = dir.path().join("pw-record");
        std::os::unix::fs::symlink("pw-cat", &alias)?;
        let trusted = resolve_test(dir.path(), "pw-record")?;
        let command = trusted.command(HelperEnvironment::PipeWireCapture)?;
        std::fs::remove_file(&alias)?;
        std::os::unix::fs::symlink("missing", &alias)?;
        assert!(matches!(
            command.spawn(),
            Err(LaunchError::Resolve(ResolveError::IdentityChanged))
        ));
        Ok(())
    }

    #[test]
    fn unsafe_resolution_cases_are_rejected() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let helper = dir.path().join("helper");
        executable(&helper, "#!/bin/sh\nexit 0\n")?;
        assert!(matches!(
            TrustedExecutable::resolve_with_policy(
                OsStr::new("helper"),
                [PathBuf::from("relative")],
                test_policy(dir.path())
            ),
            Err(ResolveError::Untrusted)
        ));
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o722))?;
        assert!(matches!(
            resolve_test(dir.path(), "helper"),
            Err(ResolveError::Untrusted)
        ));
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o600))?;
        assert!(matches!(
            resolve_test(dir.path(), "helper"),
            Err(ResolveError::NonExecutable)
        ));
        Ok(())
    }

    #[test]
    fn command_scrubs_parent_environment() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let helper = dir.path().join("env-helper");
        executable(&helper, "#!/bin/sh\nenv\n")?;
        let trusted = resolve_test(dir.path(), "env-helper")?;
        let mut command = trusted.command(HelperEnvironment::WaylandTextInput)?;
        command.stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = command.spawn()?;
        let (_, output) = collect_contained(&mut child)?;
        let output = String::from_utf8(output)?;
        assert!(!output.contains("PATH="));
        assert!(!output.contains("LD_PRELOAD="));
        // F-H8: the typing helper always gets a UTF-8 locale.
        assert!(output.lines().any(|line| line == "LC_ALL=C.UTF-8"));
        Ok(())
    }

    #[test]
    fn termination_kills_leader_and_same_group_descendant() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let helper = dir.path().join("tree-helper");
        let leader_marker = dir.path().join("leader.pid");
        let child_marker = dir.path().join("child.pid");
        executable(
            &helper,
            &format!(
                "#!/bin/sh\necho $$ > '{}'\nsleep 30 &\necho $! > '{}'\nwait\n",
                leader_marker.display(),
                child_marker.display()
            ),
        )?;
        let trusted = resolve_test(dir.path(), "tree-helper")?;
        let mut command = trusted.command(HelperEnvironment::WaylandTextInput)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let wait_deadline = Instant::now() + Duration::from_secs(2);
        while (!leader_marker.exists() || !child_marker.exists()) && Instant::now() < wait_deadline
        {
            std::thread::sleep(POLL);
        }
        let leader = std::fs::read_to_string(&leader_marker)?.trim().to_owned();
        let descendant = std::fs::read_to_string(&child_marker)?.trim().to_owned();
        terminate_group_and_reap(&mut child, Instant::now() + Duration::from_secs(2))?;
        assert!(!Path::new("/proc").join(leader).exists());
        assert!(!Path::new("/proc").join(descendant).exists());
        Ok(())
    }

    #[test]
    fn debug_does_not_expose_environment_or_arguments() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let helper = dir.path().join("helper");
        executable(&helper, "#!/bin/sh\nexit 0\n")?;
        let trusted = resolve_test(dir.path(), "helper")?;
        let rendered = format!("{trusted:?}");
        assert!(rendered.contains("system-owned-revalidated"));
        assert!(!rendered.contains("WAYLAND_DISPLAY"));
        let mut command = trusted.command(HelperEnvironment::WaylandTextInput)?;
        command.arg("private-payload");
        let rendered = format!("{command:?}");
        assert!(!rendered.contains("private-payload"));
        Ok(())
    }

    #[test]
    fn helper_name_must_be_one_path_component() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        for name in ["subdir/helper", "/bin/sh", "..", ".", ""] {
            assert!(matches!(
                TrustedExecutable::resolve_with_policy(
                    OsStr::new(name),
                    [dir.path().to_path_buf()],
                    test_policy(dir.path()),
                ),
                Err(ResolveError::Untrusted)
            ));
        }
        Ok(())
    }

    #[test]
    fn anchor_escape_unsafe_ancestry_and_setid_are_rejected() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let outside = tempfile::tempdir()?;
        std::fs::set_permissions(outside.path(), std::fs::Permissions::from_mode(0o700))?;
        let outside_helper = outside.path().join("outside-helper");
        executable(&outside_helper, "#!/bin/sh\nexit 0\n")?;
        std::os::unix::fs::symlink(&outside_helper, dir.path().join("escape"))?;
        assert!(matches!(
            resolve_test(dir.path(), "escape"),
            Err(ResolveError::Untrusted)
        ));

        let writable = dir.path().join("writable");
        std::fs::create_dir(&writable)?;
        std::fs::set_permissions(&writable, std::fs::Permissions::from_mode(0o777))?;
        executable(&writable.join("helper"), "#!/bin/sh\nexit 0\n")?;
        assert!(matches!(
            TrustedExecutable::resolve_with_policy(
                OsStr::new("helper"),
                [writable],
                test_policy(dir.path()),
            ),
            Err(ResolveError::Untrusted)
        ));

        let setid = dir.path().join("setid");
        executable(&setid, "#!/bin/sh\nexit 0\n")?;
        std::fs::set_permissions(&setid, std::fs::Permissions::from_mode(0o4700))?;
        assert!(matches!(
            resolve_test(dir.path(), "setid"),
            Err(ResolveError::Untrusted)
        ));
        Ok(())
    }

    #[test]
    fn successful_leader_with_live_descendant_is_not_success() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let helper = dir.path().join("residual-helper");
        executable(&helper, "#!/bin/sh\nsleep 30 &\nexit 0\n")?;
        let trusted = resolve_test(dir.path(), "residual-helper")?;
        let mut command = trusted.command(HelperEnvironment::WaylandTextInput)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match try_wait_contained(&mut child, deadline) {
                Err(ProcessError::UnexpectedDescendant) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
                other => return Err(format!("unexpected contained wait result: {other:?}").into()),
            }
        }
        Ok(())
    }
}
