//! Downloading a model the user asked for (`DownloadModel`).
//!
//! This is the daemon's only network use, and it happens only on that
//! request. The daemon has no network code of its own: it runs the system's
//! `curl`, resolved and revalidated as a root-owned system tool, with an
//! empty environment and its configuration file disabled. What may be
//! fetched, and from which hosts, is decided by
//! `narrivy_model_manager::download`; every redirect is checked against the
//! same rule before it is followed.
//!
//! The file is written as `<name>.part` (mode 0600) beside the loaded model
//! and resumed if a partial file is there. It is renamed into place only
//! after its size and SHA-256 match the catalog's pins; anything else is
//! deleted. Nothing but the model's id, byte counts and content-free reason
//! codes is reported.

#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use narrivy_core::CancelFlag;
use narrivy_model_manager::download::{MAX_REDIRECTS, is_primary_host, url_allowed};
use narrivy_model_manager::{DownloadTarget, PackTarget};
use narrivy_process_linux::{HelperEnvironment, TrustedExecutable, terminate_group_and_reap};

const POLL: Duration = Duration::from_millis(200);
const TERMINATION: Duration = Duration::from_secs(2);
/// One redirect probe may take this long.
const PROBE_SECONDS: &str = "40";
const CONNECT_SECONDS: &str = "20";
/// A transfer slower than 1 KiB/s for a minute is treated as stalled.
const STALL_BYTES_PER_SECOND: &str = "1024";
const STALL_SECONDS: &str = "60";
const USER_AGENT: &str = "narrivy";

enum Phase {
    Idle,
    Active {
        id: String,
        received: Arc<AtomicU64>,
        cancel: CancelFlag,
    },
    Failed {
        id: String,
        code: &'static str,
    },
}

/// The one download that may run at a time, and how the last one ended.
pub struct ModelDownloads {
    phase: Mutex<Phase>,
}

impl Default for ModelDownloads {
    fn default() -> Self {
        Self {
            phase: Mutex::new(Phase::Idle),
        }
    }
}

impl ModelDownloads {
    fn lock(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The model being downloaded and the bytes on disk so far.
    pub fn progress(&self) -> Option<(String, u64)> {
        match &*self.lock() {
            Phase::Active { id, received, .. } => {
                Some((id.clone(), received.load(Ordering::Acquire)))
            }
            Phase::Idle | Phase::Failed { .. } => None,
        }
    }

    /// The item whose last download failed, and why.
    pub fn failure(&self) -> Option<(String, &'static str)> {
        match &*self.lock() {
            Phase::Failed { id, code } => Some((id.clone(), code)),
            Phase::Idle | Phase::Active { .. } => None,
        }
    }

    /// `model.download=idle`, `downloading` or `failed:<code>`.
    pub fn capability(&self) -> String {
        match &*self.lock() {
            Phase::Idle => "model.download=idle".to_owned(),
            Phase::Active { .. } => "model.download=downloading".to_owned(),
            Phase::Failed { code, .. } => format!("model.download=failed:{code}"),
        }
    }

    /// Start a download. Refused while another one runs.
    pub fn begin(&self, id: &str) -> Option<(Arc<AtomicU64>, CancelFlag)> {
        let mut phase = self.lock();
        if matches!(*phase, Phase::Active { .. }) {
            return None;
        }
        let received = Arc::new(AtomicU64::new(0));
        let cancel = CancelFlag::new();
        *phase = Phase::Active {
            id: id.to_owned(),
            received: Arc::clone(&received),
            cancel: cancel.clone(),
        };
        Some((received, cancel))
    }

    /// A cancelled download is not a failure: it just stops.
    pub fn finish(&self, outcome: Result<(), &'static str>) {
        let mut phase = self.lock();
        let id = match &*phase {
            Phase::Active { id, .. } | Phase::Failed { id, .. } => id.clone(),
            Phase::Idle => String::new(),
        };
        *phase = match outcome {
            Ok(()) | Err("cancelled") => Phase::Idle,
            Err(code) => Phase::Failed { id, code },
        };
    }

    /// Ask the running download of `id` to stop. The partial file is kept,
    /// so a later download resumes. False when `id` is not downloading.
    pub fn cancel(&self, id: &str) -> bool {
        match &*self.lock() {
            Phase::Active {
                id: active, cancel, ..
            } if active == id => {
                cancel.cancel();
                true
            }
            _ => false,
        }
    }
}

/// Where a downloaded model goes, and the partial file it grows in.
pub fn destination(dir: &Path, target: &DownloadTarget) -> (PathBuf, PathBuf) {
    (
        dir.join(&target.filename),
        dir.join(format!("{}.part", target.filename)),
    )
}

/// Download `target` into `dir`, verify it, and move it into place.
pub fn run(
    target: &DownloadTarget,
    dir: &Path,
    received: &AtomicU64,
    cancel: &CancelFlag,
) -> Result<(), &'static str> {
    run_at(target, dir, &Progress { received, base: 0 }, cancel)
}

/// Bytes received so far: those of earlier files of a pack (`base`) plus
/// the file being fetched.
struct Progress<'a> {
    received: &'a AtomicU64,
    base: u64,
}

impl Progress<'_> {
    fn set(&self, bytes: u64) {
        self.received
            .store(self.base.saturating_add(bytes), Ordering::Release);
    }
}

fn run_at(
    target: &DownloadTarget,
    dir: &Path,
    received: &Progress<'_>,
    cancel: &CancelFlag,
) -> Result<(), &'static str> {
    let curl = TrustedExecutable::resolve_system(OsStr::new("curl"))
        .map_err(|_| "downloader_unavailable")?;
    // The engine loads models only from a directory nobody else can write.
    let directory = std::fs::symlink_metadata(dir).map_err(|_| "models_directory_missing")?;
    if !directory.is_dir() || directory.permissions().mode() & 0o077 != 0 {
        return Err("models_directory_unsafe");
    }
    let (final_path, part) = destination(dir, target);
    prepare_partial(&part, target.size_bytes)?;
    received.set(length(&part));

    if length(&part) < target.size_bytes {
        let url = resolve(&curl, &target.url, cancel)?;
        match fetch(&curl, &url, &part, target.size_bytes, received, cancel) {
            Ok(()) => {}
            // The server would not resume: start the file again, once.
            Err("resume_refused") => {
                std::fs::remove_file(&part).map_err(|_| "disk_write_failed")?;
                prepare_partial(&part, target.size_bytes)?;
                received.set(0);
                let url = resolve(&curl, &target.url, cancel)?;
                fetch(&curl, &url, &part, target.size_bytes, received, cancel)?;
            }
            Err(code) => return Err(code),
        }
    }

    // Trust the bytes only now: exact size, then the pinned hash.
    let verified = length(&part) == target.size_bytes
        && narrivy_model_manager::store::sha256_file(&part)
            .is_ok_and(|digest| digest.eq_ignore_ascii_case(&target.sha256));
    if !verified {
        let _ = std::fs::remove_file(&part);
        received.set(0);
        return Err("integrity_mismatch");
    }
    std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o600))
        .and_then(|()| std::fs::rename(&part, &final_path))
        .map_err(|_| "disk_write_failed")
}

/// Where a pack is installed under `parent`, and the staging directory it
/// is assembled in.
pub fn pack_destination(parent: &Path, pack: &PackTarget) -> (PathBuf, PathBuf) {
    (
        parent.join(&pack.id),
        parent.join(format!("{}.partial", pack.id)),
    )
}

/// Install a pack of several files under `parent`: its pinned manifest
/// first, then every file the manifest lists, each checked against it,
/// into a private staging directory that is renamed into place only when
/// all are present. An interrupted install resumes; files already verified
/// are kept.
///
/// `local` is a developer source: a directory holding the pack (as the
/// staging tree that is published), used instead of the network. Its files
/// are checked exactly as downloaded ones are.
pub fn run_pack(
    pack: &PackTarget,
    parent: &Path,
    local: Option<&Path>,
    received: &AtomicU64,
    cancel: &CancelFlag,
) -> Result<(), &'static str> {
    private_directory(parent, "models_directory_unsafe")?;
    let (final_dir, staging) = pack_destination(parent, pack);
    if std::fs::symlink_metadata(&final_dir).is_ok() {
        return Err("already_installed");
    }
    private_directory(&staging, "disk_write_failed")?;
    let source = local.map(|dir| dir.join(&pack.source_path));
    let fetch_file = |path: &str, sha256: &str, size: u64, base: u64| {
        let progress = Progress { received, base };
        pack_file(
            pack,
            source.as_deref(),
            &staging,
            path,
            sha256,
            size,
            &progress,
            cancel,
        )
    };
    fetch_file(
        pack.manifest_name,
        &pack.manifest_sha256,
        pack.manifest_size,
        0,
    )?;
    let manifest = narrivy_asr_openvino::read_manifest(&staging.join(pack.manifest_name))
        .map_err(|_| "integrity_mismatch")?;
    if manifest.total_bytes() != pack.size_bytes {
        return Err("integrity_mismatch");
    }
    let mut done = 0_u64;
    for file in &manifest.files {
        if cancel.is_cancelled() {
            return Err("cancelled");
        }
        fetch_file(&file.path, &file.sha256, file.size_bytes, done)?;
        done = done.saturating_add(file.size_bytes);
    }
    std::fs::rename(&staging, &final_dir).map_err(|_| "disk_write_failed")
}

#[allow(clippy::too_many_arguments)]
fn pack_file(
    pack: &PackTarget,
    source: Option<&Path>,
    staging: &Path,
    path: &str,
    sha256: &str,
    size: u64,
    progress: &Progress<'_>,
    cancel: &CancelFlag,
) -> Result<(), &'static str> {
    if !narrivy_asr_openvino::is_plain_relative(path) {
        return Err("integrity_mismatch");
    }
    let (dir, name) = match path.split_once('/') {
        Some((sub, name)) => (staging.join(sub), name),
        None => (staging.to_path_buf(), path),
    };
    private_directory(&dir, "disk_write_failed")?;
    let expected = narrivy_asr_openvino::PackFile {
        path: path.to_owned(),
        sha256: sha256.to_owned(),
        size_bytes: size,
    };
    // Kept from an earlier, interrupted install.
    if narrivy_asr_openvino::verify_file(&dir.join(name), &expected).is_ok() {
        progress.set(size);
        return Ok(());
    }
    let target = DownloadTarget {
        id: pack.id.clone(),
        url: String::new(),
        filename: name.to_owned(),
        size_bytes: size,
        sha256: sha256.to_ascii_lowercase(),
    };
    match source {
        Some(source) => copy_local(&source.join(path), &target, &dir, progress, cancel),
        None => {
            let url = pack.file_url(path).ok_or("not_published")?;
            run_at(&DownloadTarget { url, ..target }, &dir, progress, cancel)
        }
    }
}

/// Copy one file from a local developer source, then check it like a
/// download.
fn copy_local(
    source: &Path,
    target: &DownloadTarget,
    dir: &Path,
    progress: &Progress<'_>,
    cancel: &CancelFlag,
) -> Result<(), &'static str> {
    let (final_path, part) = destination(dir, target);
    let _ = std::fs::remove_file(&part);
    prepare_partial(&part, target.size_bytes)?;
    // Not through a link, and without waiting on a FIFO's writer: only a
    // regular file can be the pack's file. A device or FIFO would never end.
    let mut input = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)
        .map_err(|error| match error.raw_os_error() {
            Some(libc::ELOOP) => "integrity_mismatch",
            _ => "local_source_missing",
        })?;
    if !input.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err("integrity_mismatch");
    }
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .open(&part)
        .map_err(|_| "disk_write_failed")?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut copied = 0_u64;
    loop {
        if cancel.is_cancelled() {
            return Err("cancelled");
        }
        let count = input
            .read(&mut buffer)
            .map_err(|_| "local_source_missing")?;
        if count == 0 {
            break;
        }
        std::io::Write::write_all(&mut output, &buffer[..count])
            .map_err(|_| "disk_write_failed")?;
        copied = copied.saturating_add(count as u64);
        if copied > target.size_bytes {
            break;
        }
        progress.set(copied);
    }
    drop(output);
    let verified = length(&part) == target.size_bytes
        && narrivy_model_manager::store::sha256_file(&part)
            .is_ok_and(|digest| digest.eq_ignore_ascii_case(&target.sha256));
    if !verified {
        let _ = std::fs::remove_file(&part);
        progress.set(0);
        return Err("integrity_mismatch");
    }
    std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o600))
        .and_then(|()| std::fs::rename(&part, &final_path))
        .map_err(|_| "disk_write_failed")
}

/// Make sure `dir` is a real directory only this user can write, creating
/// it (mode 0700) when it is missing.
fn private_directory(dir: &Path, unsafe_code: &'static str) -> Result<(), &'static str> {
    match std::fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0 => Ok(()),
        Ok(_) => Err(unsafe_code),
        Err(_) => std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|_| "disk_write_failed"),
    }
}

fn length(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// Leave a private, regular partial file no larger than the model. Anything
/// else at that path (a link, a directory, an oversized file) is replaced.
fn prepare_partial(part: &Path, size: u64) -> Result<(), &'static str> {
    if let Ok(metadata) = std::fs::symlink_metadata(part) {
        let reusable = metadata.file_type().is_file()
            && metadata.len() <= size
            && metadata.permissions().mode() & 0o077 == 0;
        if reusable {
            return Ok(());
        }
        if metadata.is_dir() {
            return Err("disk_write_failed");
        }
        std::fs::remove_file(part).map_err(|_| "disk_write_failed")?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(part)
        .map(|_| ())
        .map_err(|_| "disk_write_failed")
}

/// Arguments every `curl` run starts with: no configuration file, quiet,
/// `https` only, no redirect followed on its own.
fn base_arguments() -> [&'static str; 12] {
    [
        "-q",
        "-sS",
        "--proto",
        "=https",
        "--tlsv1.2",
        "-L",
        "--max-redirs",
        "0",
        "--connect-timeout",
        CONNECT_SECONDS,
        "-A",
        USER_AGENT,
    ]
}

/// Follow redirects on the primary host by hand, checking each against the
/// download rules, until the URL that serves the file is known.
fn resolve(
    curl: &TrustedExecutable,
    url: &str,
    cancel: &CancelFlag,
) -> Result<String, &'static str> {
    let mut url = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        if !url_allowed(&url) {
            return Err("redirect_refused");
        }
        if !is_primary_host(&url) {
            return Ok(url);
        }
        let mut command = curl
            .command(HelperEnvironment::HttpsDownload)
            .map_err(|_| "downloader_unavailable")?;
        command
            // `-L` is left out here: the redirect is reported, not followed.
            .args(["-q", "-sS", "-I", "--proto", "=https", "--tlsv1.2"])
            .args(["--max-redirs", "0", "--connect-timeout", CONNECT_SECONDS])
            .args(["--max-time", PROBE_SECONDS, "-A", USER_AGENT])
            .args(["-o", "/dev/null", "-w", "%{http_code}\n%{redirect_url}\n"])
            .arg(&url)
            .stdout(Stdio::piped());
        let mut child = command.spawn().map_err(|_| "downloader_unavailable")?;
        let status = wait(&mut child, cancel, || {})?;
        let mut report = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            let _ = stdout.by_ref().take(8 * 1024).read_to_string(&mut report);
        }
        if status != 0 {
            return Err(exit_reason(status));
        }
        let mut lines = report.lines();
        let code = lines.next().unwrap_or_default();
        let next = lines.next().unwrap_or_default();
        match code {
            "200" => return Ok(url),
            "301" | "302" | "303" | "307" | "308" if !next.is_empty() => url = next.to_owned(),
            _ => return Err("http_error"),
        }
    }
    Err("redirect_refused")
}

/// Fetch `url` into the partial file, resuming from what is already there.
fn fetch(
    curl: &TrustedExecutable,
    url: &str,
    part: &Path,
    size: u64,
    received: &Progress<'_>,
    cancel: &CancelFlag,
) -> Result<(), &'static str> {
    if !url_allowed(url) {
        return Err("redirect_refused");
    }
    let mut command = curl
        .command(HelperEnvironment::HttpsDownload)
        .map_err(|_| "downloader_unavailable")?;
    command
        .args(base_arguments())
        .args(["--fail", "--speed-limit", STALL_BYTES_PER_SECOND])
        .args(["--speed-time", STALL_SECONDS])
        .arg("--max-filesize")
        .arg(size.to_string())
        .args(["-C", "-", "-o"])
        .arg(part)
        .arg(url);
    let mut child = command.spawn().map_err(|_| "downloader_unavailable")?;
    let status = wait(&mut child, cancel, || received.set(length(part)))?;
    received.set(length(part));
    match status {
        0 => Ok(()),
        // 33: the server ignored the range; 36: the partial file could not
        // be continued.
        33 | 36 => Err("resume_refused"),
        other => Err(exit_reason(other)),
    }
}

/// Wait for `child`, calling `tick` while it runs. Cancellation kills its
/// whole process group.
fn wait(
    child: &mut std::process::Child,
    cancel: &CancelFlag,
    mut tick: impl FnMut(),
) -> Result<i32, &'static str> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.code().unwrap_or(-1)),
            Ok(None) => {}
            Err(_) => return Err("download_failed"),
        }
        if cancel.is_cancelled() {
            let _ = terminate_group_and_reap(child, Instant::now() + TERMINATION);
            return Err("cancelled");
        }
        tick();
        std::thread::sleep(POLL);
    }
}

/// A content-free reason for a `curl` exit status.
fn exit_reason(status: i32) -> &'static str {
    match status {
        5..=7 => "network_unreachable",
        22 => "http_error",
        23 => "disk_write_failed",
        28 => "stalled",
        35 | 51 | 58 | 60 | 77 => "tls_failed",
        47 => "redirect_refused",
        63 => "too_large",
        _ => "download_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn one_download_at_a_time_and_a_cancel_is_not_a_failure() {
        let downloads = ModelDownloads::default();
        assert_eq!(downloads.capability(), "model.download=idle");
        assert_eq!(downloads.progress(), None);
        let Some((received, cancel)) = downloads.begin("model-a") else {
            return;
        };
        assert!(
            downloads.begin("model-b").is_none(),
            "a second download started"
        );
        received.store(4_096, Ordering::Release);
        assert_eq!(downloads.progress(), Some(("model-a".to_owned(), 4_096)));
        assert_eq!(downloads.capability(), "model.download=downloading");

        assert!(!downloads.cancel("model-b"));
        assert!(!cancel.is_cancelled());
        assert!(downloads.cancel("model-a"));
        assert!(cancel.is_cancelled());
        downloads.finish(Err("cancelled"));
        assert_eq!(downloads.capability(), "model.download=idle");

        assert!(downloads.begin("model-a").is_some());
        downloads.finish(Err("integrity_mismatch"));
        assert_eq!(
            downloads.capability(),
            "model.download=failed:integrity_mismatch"
        );
        // A failure does not block the next attempt.
        assert!(downloads.begin("model-a").is_some());
        downloads.finish(Ok(()));
        assert_eq!(downloads.capability(), "model.download=idle");
    }

    #[test]
    fn a_partial_file_is_private_regular_and_never_larger_than_the_model() -> TestResult {
        let dir = tempfile::tempdir()?;
        let part = dir.path().join("model.gguf.part");

        prepare_partial(&part, 100).map_err(str::to_owned)?;
        assert_eq!(
            std::fs::metadata(&part)?.permissions().mode() & 0o777,
            0o600
        );

        // A partial download is kept for resuming.
        std::fs::write(&part, [7_u8; 40])?;
        prepare_partial(&part, 100).map_err(str::to_owned)?;
        assert_eq!(length(&part), 40);

        // Larger than the model, readable by others, or a link: replaced.
        std::fs::write(&part, [7_u8; 101])?;
        prepare_partial(&part, 100).map_err(str::to_owned)?;
        assert_eq!(length(&part), 0);
        std::fs::write(&part, [7_u8; 10])?;
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o644))?;
        prepare_partial(&part, 100).map_err(str::to_owned)?;
        assert_eq!(length(&part), 0);
        std::fs::remove_file(&part)?;
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, b"not a model")?;
        std::os::unix::fs::symlink(&elsewhere, &part)?;
        prepare_partial(&part, 100).map_err(str::to_owned)?;
        assert!(std::fs::symlink_metadata(&part)?.file_type().is_file());
        assert_eq!(std::fs::read(&elsewhere)?, b"not a model");
        Ok(())
    }

    #[test]
    fn a_shared_models_directory_is_refused_before_any_connection() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))?;
        let target = DownloadTarget {
            id: "m".to_owned(),
            url: "https://huggingface.co/narrivy/m/resolve/0/m.gguf".to_owned(),
            filename: "m.gguf".to_owned(),
            size_bytes: 10,
            sha256: "0".repeat(64),
        };
        let outcome = run(&target, dir.path(), &AtomicU64::new(0), &CancelFlag::new());
        // Without the system downloader the refusal is that instead; either
        // way nothing was written.
        assert!(matches!(
            outcome,
            Err("models_directory_unsafe" | "downloader_unavailable")
        ));
        assert!(!dir.path().join("m.gguf.part").exists());
        Ok(())
    }

    #[test]
    fn exit_statuses_map_to_content_free_reasons() {
        assert_eq!(exit_reason(6), "network_unreachable");
        assert_eq!(exit_reason(22), "http_error");
        assert_eq!(exit_reason(28), "stalled");
        assert_eq!(exit_reason(47), "redirect_refused");
        assert_eq!(exit_reason(60), "tls_failed");
        assert_eq!(exit_reason(63), "too_large");
        assert_eq!(exit_reason(1), "download_failed");
    }

    fn pack(size: u64) -> PackTarget {
        PackTarget {
            id: "pack".to_owned(),
            source_path: "src".to_owned(),
            base_url: None,
            manifest_name: "model-manifest.json",
            manifest_sha256: "0".repeat(64),
            manifest_size: 1,
            size_bytes: size,
        }
    }

    fn hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// A pack file named `a//b` passes a check that normalises paths, and
    /// split on `/` it named `/b`: the copy went to the filesystem root.
    /// Every unsafe entry name is refused before anything is written.
    #[test]
    fn unsafe_pack_entry_names_are_refused_before_any_write() -> TestResult {
        let parent = tempfile::tempdir()?;
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700))?;
        let staging = parent.path().join("pack.partial");
        std::fs::DirBuilder::new().mode(0o700).create(&staging)?;
        let source = tempfile::tempdir()?;
        let progress = AtomicU64::new(0);
        for path in [
            "a//b", "a/./b", "a/", "/abs", "../up", "a/../b", "a/b/c", "", "a\\b", "a\0b",
        ] {
            let outcome = pack_file(
                &pack(1),
                Some(source.path()),
                &staging,
                path,
                &"0".repeat(64),
                1,
                &Progress {
                    received: &progress,
                    base: 0,
                },
                &CancelFlag::new(),
            );
            assert_eq!(outcome, Err("integrity_mismatch"), "{path:?}");
        }
        assert_eq!(std::fs::read_dir(&staging)?.count(), 0);
        Ok(())
    }

    /// A local source that is a link, a FIFO, a device or the wrong bytes is
    /// refused with a typed code, without blocking and without copying more
    /// than the declared size.
    #[test]
    fn local_sources_must_be_regular_files_with_the_pinned_bytes() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let source = tempfile::tempdir()?;
        let good = b"model bytes";
        std::fs::write(source.path().join("real"), good)?;
        std::os::unix::fs::symlink(source.path().join("real"), source.path().join("link"))?;
        // The `mkfifo` tool keeps this crate free of `unsafe`.
        let made = std::process::Command::new("mkfifo")
            .arg(source.path().join("fifo"))
            .status()
            .is_ok_and(|status| status.success());
        let target = |sha256: String, size: u64| DownloadTarget {
            id: "m".to_owned(),
            url: String::new(),
            filename: "m.bin".to_owned(),
            size_bytes: size,
            sha256,
        };
        let progress = AtomicU64::new(0);
        let copy = |name: &str, target: &DownloadTarget| {
            copy_local(
                &source.path().join(name),
                target,
                dir.path(),
                &Progress {
                    received: &progress,
                    base: 0,
                },
                &CancelFlag::new(),
            )
        };
        let pinned = target(hex(good), good.len() as u64);
        assert_eq!(copy("link", &pinned), Err("integrity_mismatch"));
        if made {
            assert_eq!(copy("fifo", &pinned), Err("integrity_mismatch"));
        }
        assert_eq!(
            copy_local(
                Path::new("/dev/zero"),
                &target(hex(good), u64::MAX),
                dir.path(),
                &Progress {
                    received: &progress,
                    base: 0,
                },
                &CancelFlag::new(),
            ),
            Err("integrity_mismatch")
        );
        assert_eq!(copy("missing", &pinned), Err("local_source_missing"));
        // The hash or the size does not match the pin.
        assert_eq!(
            copy("real", &target(hex(b"other"), good.len() as u64)),
            Err("integrity_mismatch")
        );
        assert_eq!(
            copy("real", &target(hex(good), u64::MAX)),
            Err("integrity_mismatch")
        );
        assert_eq!(
            copy("real", &target(hex(good), 3)),
            Err("integrity_mismatch")
        );
        assert!(!dir.path().join("m.bin").exists());
        assert!(!dir.path().join("m.bin.part").exists());
        // The real file with its pins is accepted.
        assert_eq!(copy("real", &pinned), Ok(()));
        assert_eq!(std::fs::read(dir.path().join("m.bin"))?, good);
        Ok(())
    }
}
