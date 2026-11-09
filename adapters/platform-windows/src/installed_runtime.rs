//! The installed resident runtime on Windows: the counterpart of the Linux
//! adapter's `installed_runtime`, for a per-machine (MSI) install.
//!
//! Everything sits beside the installed daemon, in a folder only
//! TrustedInstaller, SYSTEM and Administrators can change (Program Files):
//! `installed-runtime-manifest.json` (the same schema as on Linux: the
//! worker's SHA-256 and size), `narrivy-transcribe-worker.exe`,
//! `models\<id>.gguf` for the model bundled with the package, and the
//! language-model worker `narrivy-openvino-llm-worker.exe` with its own
//! `llm-worker-manifest.json` of the same shape. The manifest
//! is read only through those ownership checks; the worker is bound to the
//! manifest's digest and the model to the digest compiled into this build's
//! registry, each copied into an immutable private snapshot. A per-user
//! copy of the same files fails the checks and is refused, never run.

use std::path::{Path, PathBuf};
use std::time::Instant;

use narrivy_model_manager::{BundledInstalledModelBinding, BundledModelIdentity};
use narrivy_process_windows::{
    ArtifactError, ArtifactProvenance, DataArtifactError, VerifiedArtifactData,
    VerifiedArtifactExecutable, read_installed_file,
};
use narrivy_types::SessionCancellation;
use serde::Deserialize;

const MANIFEST_FILE: &str = "installed-runtime-manifest.json";
const WORKER_FILE: &str = "narrivy-transcribe-worker.exe";
/// The same speech worker built for the processor only, used when this
/// computer has no Vulkan loader (`vulkan-1.dll`, installed with the
/// graphics driver): the Vulkan worker needs it just to start.
const CPU_MANIFEST_FILE: &str = "installed-runtime-cpu-manifest.json";
const CPU_WORKER_FILE: &str = "narrivy-transcribe-worker-cpu.exe";
const VULKAN_LOADER: &str = "vulkan-1.dll";
/// The language-model worker (refine and the AI assistant, D48), as on
/// Linux.
const LLM_MANIFEST_FILE: &str = "llm-worker-manifest.json";
const LLM_WORKER_FILE: &str = "narrivy-openvino-llm-worker.exe";
const LLM_RUNTIME_ID: &str = "openvino-llm";
/// The OpenVINO speech worker (the OpenVINO add-on's models), as on Linux.
const OPENVINO_MANIFEST_FILE: &str = "openvino-worker-manifest.json";
const OPENVINO_WORKER_FILE: &str = "narrivy-openvino-worker.exe";
const OPENVINO_RUNTIME_ID: &str = "openvino";
/// Voice mode's worker and its Silero detector, as on Linux.
const VOICE_MANIFEST_FILE: &str = "voice-worker-manifest.json";
const VOICE_WORKER_FILE: &str = "narrivy-voice-worker.exe";
const VOICE_RUNTIME_ID: &str = "voice";
const VOICE_DETECTOR_FILE: &str = "voice\\silero_vad.onnx";
/// Voice mode's Urdu speaker (GPL-3.0-or-later, D51), as on Linux.
const URDU_MANIFEST_FILE: &str = "urdu-speaker-manifest.json";
const URDU_SPEAKER_FILE: &str = "narrivy-urdu-speaker.exe";
const URDU_RUNTIME_ID: &str = "urdu-speaker";
const MODELS_DIR: &str = "models";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
const MAX_WORKER_BYTES: u64 = 256 * 1024 * 1024;
/// Trust anchors are operating-system principals on Windows; the Unix owner
/// argument of the shared artifact API is not used.
const NO_UNIX_OWNER: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstalledRuntimeError {
    ManifestMissing,
    ManifestUntrusted,
    ManifestTooLarge,
    ManifestInvalid,
    UnsupportedRuntime,
    Worker(ArtifactError),
}

impl std::fmt::Display for InstalledRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ManifestMissing => "installed runtime manifest is missing",
            Self::ManifestUntrusted => "installed runtime manifest is untrusted",
            Self::ManifestTooLarge => "installed runtime manifest exceeds its size limit",
            Self::ManifestInvalid => "installed runtime manifest is invalid",
            Self::UnsupportedRuntime => "installed runtime identity is unsupported",
            Self::Worker(_) => "installed runtime worker binding failed",
        })
    }
}

impl std::error::Error for InstalledRuntimeError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestV1 {
    schema_version: u32,
    runtime_id: String,
    worker: WorkerV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerV1 {
    sha256: String,
    size_bytes: u64,
}

/// The installed worker, bound to the installed manifest.
#[derive(Clone)]
pub struct InstalledRuntimeBinding {
    worker: VerifiedArtifactExecutable,
}

impl std::fmt::Debug for InstalledRuntimeBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstalledRuntimeBinding")
            .field("runtime", &"transcribe.cpp")
            .field("worker_size_bytes", &self.worker.size())
            .field("binding", &"installed-runtime-manifest-bound")
            .finish_non_exhaustive()
    }
}

impl InstalledRuntimeBinding {
    #[must_use]
    pub fn worker(&self) -> &VerifiedArtifactExecutable {
        &self.worker
    }

    #[must_use]
    pub fn into_worker(self) -> VerifiedArtifactExecutable {
        self.worker
    }
}

/// The installed model, bound to this build's registry digest.
#[derive(Clone)]
pub struct InstalledModelRuntimeBinding {
    model: VerifiedArtifactData,
    id: String,
    family: String,
    source_revision: String,
}

impl std::fmt::Debug for InstalledModelRuntimeBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstalledModelRuntimeBinding")
            .field("model_size_bytes", &self.model.size())
            .field("binding", &"registry-observed-immutable-digest-bound")
            .finish_non_exhaustive()
    }
}

impl InstalledModelRuntimeBinding {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn family(&self) -> &str {
        &self.family
    }

    #[must_use]
    pub fn source_revision(&self) -> &str {
        &self.source_revision
    }

    #[must_use]
    pub fn model(&self) -> &VerifiedArtifactData {
        &self.model
    }

    #[must_use]
    pub fn into_model(self) -> VerifiedArtifactData {
        self.model
    }
}

/// The folder of the running program: the install folder for an installed
/// daemon.
fn install_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
        .filter(|dir| dir.is_absolute())
}

/// Where the installed model `identity` lives (it is verified only when
/// bound).
#[must_use]
pub fn bundled_model_path(identity: &BundledModelIdentity) -> Option<PathBuf> {
    Some(
        install_dir()?
            .join(MODELS_DIR)
            .join(model_file_name(identity.id())?),
    )
}

/// Bind the installed worker through the installed manifest.
///
/// # Errors
/// A typed [`InstalledRuntimeError`]; nothing is run on failure.
pub fn resolve_installed_runtime() -> Result<InstalledRuntimeBinding, InstalledRuntimeError> {
    let root = install_dir().ok_or(InstalledRuntimeError::ManifestUntrusted)?;
    resolve_under(&root)
}

/// Bind the installed language-model worker through its own installed
/// manifest. Not package or release provenance.
///
/// # Errors
/// A typed [`InstalledRuntimeError`]; nothing is run on failure.
pub fn resolve_installed_llm_worker() -> Result<VerifiedArtifactExecutable, InstalledRuntimeError> {
    let root = install_dir().ok_or(InstalledRuntimeError::ManifestUntrusted)?;
    resolve_worker_under(&root, &LLM_WORKER)
}

/// Bind the installed OpenVINO speech worker through its own installed
/// manifest. Not package or release provenance.
///
/// # Errors
/// A typed [`InstalledRuntimeError`]; nothing is run on failure.
pub fn resolve_installed_openvino_worker()
-> Result<VerifiedArtifactExecutable, InstalledRuntimeError> {
    let root = install_dir().ok_or(InstalledRuntimeError::ManifestUntrusted)?;
    resolve_worker_under(&root, &OPENVINO_WORKER)
}

/// Bind the installed voice-mode worker through its own installed
/// manifest. Not package or release provenance.
///
/// # Errors
/// A typed [`InstalledRuntimeError`]; nothing is run on failure.
pub fn resolve_installed_voice_worker() -> Result<VerifiedArtifactExecutable, InstalledRuntimeError>
{
    let root = install_dir().ok_or(InstalledRuntimeError::ManifestUntrusted)?;
    resolve_worker_under(&root, &VOICE_WORKER)
}

/// Where the MSI puts voice mode's Silero detector (beside the daemon). The
/// caller checks its digest before use.
#[must_use]
pub fn installed_voice_detector() -> PathBuf {
    install_dir().unwrap_or_default().join(VOICE_DETECTOR_FILE)
}

/// Check the installed Urdu speaker against its own installed manifest and
/// return its path beside the daemon: the voice worker starts it by path.
///
/// # Errors
/// A typed [`InstalledRuntimeError`].
pub fn resolve_installed_urdu_speaker() -> Result<PathBuf, InstalledRuntimeError> {
    let root = install_dir().ok_or(InstalledRuntimeError::ManifestUntrusted)?;
    resolve_worker_under(&root, &URDU_SPEAKER)?;
    Ok(root.join(URDU_SPEAKER_FILE))
}

/// One fixed worker and the manifest that names its bytes.
struct FixedWorker {
    manifest: &'static str,
    worker: &'static str,
    runtime_id: &'static str,
}

const TRANSCRIBE_WORKER: FixedWorker = FixedWorker {
    manifest: MANIFEST_FILE,
    worker: WORKER_FILE,
    runtime_id: "transcribe.cpp",
};

const TRANSCRIBE_CPU_WORKER: FixedWorker = FixedWorker {
    manifest: CPU_MANIFEST_FILE,
    worker: CPU_WORKER_FILE,
    runtime_id: "transcribe.cpp",
};

const LLM_WORKER: FixedWorker = FixedWorker {
    manifest: LLM_MANIFEST_FILE,
    worker: LLM_WORKER_FILE,
    runtime_id: LLM_RUNTIME_ID,
};

const OPENVINO_WORKER: FixedWorker = FixedWorker {
    manifest: OPENVINO_MANIFEST_FILE,
    worker: OPENVINO_WORKER_FILE,
    runtime_id: OPENVINO_RUNTIME_ID,
};

const VOICE_WORKER: FixedWorker = FixedWorker {
    manifest: VOICE_MANIFEST_FILE,
    worker: VOICE_WORKER_FILE,
    runtime_id: VOICE_RUNTIME_ID,
};

const URDU_SPEAKER: FixedWorker = FixedWorker {
    manifest: URDU_MANIFEST_FILE,
    worker: URDU_SPEAKER_FILE,
    runtime_id: URDU_RUNTIME_ID,
};

fn resolve_under(root: &Path) -> Result<InstalledRuntimeBinding, InstalledRuntimeError> {
    resolve_worker_under(root, speech_worker(vulkan_loader_present()))
        .map(|worker| InstalledRuntimeBinding { worker })
}

/// The Vulkan worker where the Vulkan loader is installed, else the
/// processor-only one.
fn speech_worker(vulkan: bool) -> &'static FixedWorker {
    if vulkan {
        &TRANSCRIBE_WORKER
    } else {
        &TRANSCRIBE_CPU_WORKER
    }
}

/// Whether the Vulkan loader is in the system folder, where graphics
/// drivers install it and where Windows finds it for the worker. Unknown
/// (no `SystemRoot`) counts as present, the worker then reports its own
/// failure as before.
fn vulkan_loader_present() -> bool {
    std::env::var_os("SystemRoot").is_none_or(|root| {
        Path::new(&root)
            .join("System32")
            .join(VULKAN_LOADER)
            .is_file()
    })
}

fn resolve_worker_under(
    root: &Path,
    fixed: &FixedWorker,
) -> Result<VerifiedArtifactExecutable, InstalledRuntimeError> {
    let manifest_bytes = read_installed_file(&root.join(fixed.manifest), root, MAX_MANIFEST_BYTES)
        .map_err(|error| match error {
            DataArtifactError::Missing => InstalledRuntimeError::ManifestMissing,
            DataArtifactError::InvalidSize => InstalledRuntimeError::ManifestTooLarge,
            _ => InstalledRuntimeError::ManifestUntrusted,
        })?;
    let manifest: ManifestV1 = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| InstalledRuntimeError::ManifestInvalid)?;
    if manifest.schema_version != 1 || manifest.runtime_id != fixed.runtime_id {
        return Err(InstalledRuntimeError::UnsupportedRuntime);
    }
    if manifest.worker.size_bytes == 0
        || manifest.worker.size_bytes > MAX_WORKER_BYTES
        || !is_lower_hex_digest(&manifest.worker.sha256)
    {
        return Err(InstalledRuntimeError::ManifestInvalid);
    }
    let worker = VerifiedArtifactExecutable::bind_trust_anchored(
        &root.join(fixed.worker),
        &manifest.worker.sha256,
        manifest.worker.size_bytes,
        root,
        NO_UNIX_OWNER,
    )
    .map_err(InstalledRuntimeError::Worker)?;
    if worker.provenance() != ArtifactProvenance::TrustAnchorDigestBound {
        return Err(InstalledRuntimeError::Worker(ArtifactError::Untrusted));
    }
    Ok(worker)
}

/// Bind the model the package installed (`models\<id>.gguf`) to this
/// build's registry digest and size for `identity`.
///
/// # Errors
/// [`DataArtifactError::Missing`] when the package has no such model; any
/// other variant when it may not be used.
pub fn bind_bundled_model(
    identity: &BundledModelIdentity,
) -> Result<InstalledModelRuntimeBinding, DataArtifactError> {
    let root = install_dir().ok_or(DataArtifactError::Untrusted)?;
    let file = model_file_name(identity.id()).ok_or(DataArtifactError::Untrusted)?;
    let model = VerifiedArtifactData::bind_trust_anchored(
        &root.join(MODELS_DIR).join(file),
        identity.sha256(),
        identity.size_bytes(),
        &root,
        NO_UNIX_OWNER,
    )?;
    if model.provenance() != ArtifactProvenance::TrustAnchorDigestBound {
        return Err(DataArtifactError::Untrusted);
    }
    Ok(InstalledModelRuntimeBinding {
        model,
        id: identity.id().to_owned(),
        family: identity.family().to_owned(),
        source_revision: identity.source_revision().to_owned(),
    })
}

/// Models imported into a per-user cache (downloads) are Linux-only so far.
///
/// # Errors
/// Always [`DataArtifactError::UnsupportedPlatform`].
pub fn bind_installed_model_controlled(
    _binding: &BundledInstalledModelBinding,
    _cancellation: &SessionCancellation,
    _deadline: Instant,
) -> Result<InstalledModelRuntimeBinding, DataArtifactError> {
    Err(DataArtifactError::UnsupportedPlatform)
}

/// `<id>.gguf` for a registry id that is a plain file name.
fn model_file_name(id: &str) -> Option<String> {
    let plain = !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    plain.then(|| format!("{id}.gguf"))
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_the_vulkan_loader_the_processor_worker_is_used() {
        assert_eq!(speech_worker(true).worker, WORKER_FILE);
        assert_eq!(speech_worker(false).worker, CPU_WORKER_FILE);
        assert_eq!(speech_worker(false).manifest, CPU_MANIFEST_FILE);
        assert_eq!(
            speech_worker(false).runtime_id,
            speech_worker(true).runtime_id
        );
    }

    #[test]
    fn model_files_are_plain_names() {
        assert_eq!(
            model_file_name("whisper-tiny.en-Q8_0").as_deref(),
            Some("whisper-tiny.en-Q8_0.gguf")
        );
        for bad in ["", "..", ".hidden", "a\\b", "a/b", "C:x", "a b"] {
            assert_eq!(model_file_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_user_folder_is_not_an_install() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join(MANIFEST_FILE),
            br#"{"schema_version":1,"runtime_id":"transcribe.cpp","worker":{"sha256":"00","size_bytes":1}}"#,
        )?;
        assert_eq!(
            resolve_under(dir.path()).err(),
            Some(InstalledRuntimeError::ManifestUntrusted)
        );
        assert_eq!(
            resolve_under(&dir.path().join("absent")).err(),
            Some(InstalledRuntimeError::ManifestMissing)
        );
        std::fs::write(
            dir.path().join(LLM_MANIFEST_FILE),
            br#"{"schema_version":1,"runtime_id":"openvino-llm","worker":{"sha256":"00","size_bytes":1}}"#,
        )?;
        assert_eq!(
            resolve_worker_under(dir.path(), &LLM_WORKER).err(),
            Some(InstalledRuntimeError::ManifestUntrusted)
        );
        assert_eq!(
            resolve_worker_under(&dir.path().join("absent"), &LLM_WORKER).err(),
            Some(InstalledRuntimeError::ManifestMissing)
        );
        Ok(())
    }

    #[test]
    fn digests_are_lower_hex() {
        assert!(is_lower_hex_digest(&"a1".repeat(32)));
        assert!(!is_lower_hex_digest(&"A1".repeat(32)));
        assert!(!is_lower_hex_digest("abc"));
    }
}
