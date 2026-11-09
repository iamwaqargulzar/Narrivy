//! Trusted model registry (SEC-007).
//!
//! Entries come from a versioned JSON file (`registry.json`, version 1).
//! Sources are *locations, not identity*: identity is the entry id plus the
//! immutable `source_revision` pin and the `sha256` digest. The downloader
//! never accepts caller-supplied URLs — only sources declared here.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ModelManagerError, Result};

/// Current on-disk registry format version.
pub const REGISTRY_VERSION: u32 = 1;

/// Seeded registry shipped with the crate (P5 lock candidates; see
/// `seed/registry.json`).
const SEED_REGISTRY_JSON: &str = include_str!("../seed/registry.json");

// Registry JSON is trusted configuration only after it passes these bounds
// and the semantic checks below. Keep the document cap ahead of serde so an
// attacker-controlled registry cannot force unbounded parser allocation.
const MAX_REGISTRY_BYTES: usize = 1024 * 1024;
const MAX_REGISTRY_ENTRIES: usize = 256;
const MAX_SOURCES_PER_ENTRY: usize = 8;
const MAX_LANGUAGES_PER_ENTRY: usize = 64;
const MAX_SOURCE_LOCATION_BYTES: usize = 4096;
const MAX_LICENSE_BYTES: usize = 120;
const MAX_LANGUAGE_BYTES: usize = 64;

/// A download location for one artifact. `kind` selects how `repo_or_url` is
/// interpreted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    pub kind: SourceKind,
    /// HF repo id (`org/name`) for [`SourceKind::Hf`], or an absolute URL for
    /// [`SourceKind::Url`] (https or file scheme only).
    pub repo_or_url: String,
    pub filename: String,
}

/// Supported source kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// Hugging Face repo resolved at the entry's pinned revision.
    Hf,
    /// Direct URL (restricted scheme set; never granted trust by itself).
    Url,
}

/// Runtime-facing capabilities advertised by a model entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub languages: Vec<String>,
    pub streaming: bool,
}

/// One registry entry: a declared model variant with integrity metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryEntry {
    pub id: String,
    pub family: String,
    pub variant: String,
    pub quant: String,
    pub size_bytes: u64,
    /// Whole-file sha256. Optional while a candidate is unqualified, but
    /// acquisition refuses to run without it (SEC-002).
    #[serde(default)]
    pub sha256: Option<String>,
    pub license: String,
    /// Immutable upstream revision (HF commit sha / tag). Never "latest".
    pub source_revision: String,
    pub sources: Vec<SourceSpec>,
    pub capabilities: Capabilities,
    /// User/operator pin: cleanup must always preserve pinned entries.
    #[serde(default)]
    pub pinned: bool,
    /// True until the variant passed qualification (PERF-005 lane work).
    #[serde(default)]
    pub unqualified: bool,
}

impl RegistryEntry {
    pub(crate) fn validate(&self) -> Result<()> {
        validate_entry(self)
    }

    /// Resolve the first declared source into a concrete URL, enforcing the
    /// trusted-scheme policy (SEC-007).
    pub fn resolve_url(&self) -> Result<String> {
        self.validate()?;
        let source = self
            .sources
            .first()
            .ok_or_else(|| ModelManagerError::NoSource(self.id.clone()))?;
        match source.kind {
            SourceKind::Hf => {
                if source.repo_or_url.contains("://") || source.repo_or_url.starts_with('/') {
                    return Err(ModelManagerError::UntrustedSourceUrl(
                        source.repo_or_url.clone(),
                    ));
                }
                let revision = if self.source_revision.is_empty() {
                    return Err(ModelManagerError::RegistryParse {
                        path: PathBuf::from("<entry>"),
                        message: format!("entry `{}` has empty source_revision", self.id),
                    });
                } else {
                    self.source_revision.as_str()
                };
                Ok(format!(
                    "https://huggingface.co/{}/resolve/{revision}/{}",
                    source.repo_or_url, source.filename
                ))
            }
            SourceKind::Url => {
                let trusted = source.repo_or_url.starts_with("https://")
                    || source.repo_or_url.starts_with("file://");
                if !trusted {
                    return Err(ModelManagerError::UntrustedSourceUrl(
                        source.repo_or_url.clone(),
                    ));
                }
                Ok(source.repo_or_url.clone())
            }
        }
    }
}

/// Top-level registry document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryFile {
    pub version: u32,
    pub entries: Vec<RegistryEntry>,
}

/// Parsed, validated view over a [`RegistryFile`].
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    file: RegistryFile,
}

/// The registry compiled into this exact Narrivy build.
///
/// Unlike [`ModelRegistry`], this type cannot be constructed from caller
/// supplied JSON. It is the only registry authority accepted by offline
/// provisioning, preventing a local manifest from choosing its own digest and
/// then laundering arbitrary bytes into an installed-model observation.
#[derive(Debug, Clone)]
pub struct BundledModelRegistry {
    registry: ModelRegistry,
}

/// Opaque identity for one model compiled into this exact build.
///
/// Callers can inspect the fixed identity fields but cannot construct or
/// replace this authority from parsed JSON or a caller-built registry entry.
#[derive(Clone)]
pub struct BundledModelIdentity {
    entry: RegistryEntry,
    sha256: String,
}

impl std::fmt::Debug for BundledModelIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BundledModelIdentity")
            .field("id", &self.entry.id)
            .field("family", &self.entry.family)
            .field("size_bytes", &self.entry.size_bytes)
            .finish_non_exhaustive()
    }
}

impl BundledModelIdentity {
    pub fn id(&self) -> &str {
        &self.entry.id
    }

    pub fn family(&self) -> &str {
        &self.entry.family
    }

    pub fn source_revision(&self) -> &str {
        &self.entry.source_revision
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn size_bytes(&self) -> u64 {
        self.entry.size_bytes
    }

    pub(crate) fn entry(&self) -> &RegistryEntry {
        &self.entry
    }
}

impl BundledModelRegistry {
    /// Load the statically compiled registry after its normal structural and
    /// semantic validation.
    #[allow(clippy::expect_used)] // static crate data is pinned by unit tests
    pub fn load() -> Self {
        Self {
            registry: ModelRegistry::from_json_str(SEED_REGISTRY_JSON, Path::new("<bundled>"))
                .expect("bundled seed registry must stay parseable"),
        }
    }

    /// Resolve an import/installed-observation authority without echoing an
    /// untrusted caller string in the error.
    pub fn resolve_import(&self, id: &str) -> Result<BundledModelIdentity> {
        let entry = self
            .registry
            .entries()
            .iter()
            .find(|entry| entry.id == id)
            .ok_or(ModelManagerError::UnknownBundledModel)?;
        let sha256 = entry
            .sha256
            .clone()
            .ok_or_else(|| ModelManagerError::ChecksumRequired(entry.id.clone()))?;
        Ok(BundledModelIdentity {
            entry: entry.clone(),
            sha256,
        })
    }

    /// Read-only catalog entries for capability/status presentation. This does
    /// not expose a way to add or replace registry authority.
    pub fn entries(&self) -> &[RegistryEntry] {
        self.registry.entries()
    }
}

impl ModelRegistry {
    /// Parse from a JSON string. Rejects unknown format versions.
    pub fn from_json_str(json: &str, origin: &Path) -> Result<Self> {
        if json.len() > MAX_REGISTRY_BYTES {
            return Err(registry_parse(origin, "registry exceeds the size limit"));
        }
        let file: RegistryFile =
            serde_json::from_str(json).map_err(|e| ModelManagerError::RegistryParse {
                path: origin.to_path_buf(),
                message: e.to_string(),
            })?;
        if file.version != REGISTRY_VERSION {
            return Err(ModelManagerError::RegistryVersion {
                path: origin.to_path_buf(),
                found: file.version,
            });
        }
        if file.entries.len() > MAX_REGISTRY_ENTRIES {
            return Err(registry_parse(origin, "registry has too many entries"));
        }
        let ids: Vec<&str> = file.entries.iter().map(|e| e.id.as_str()).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() != ids.len() {
            return Err(ModelManagerError::RegistryParse {
                path: origin.to_path_buf(),
                message: "duplicate entry ids".to_string(),
            });
        }
        for entry in &file.entries {
            entry.validate()?;
        }
        Ok(Self { file })
    }

    /// Load from disk.
    pub fn from_path(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        if metadata.len() > MAX_REGISTRY_BYTES as u64 {
            return Err(registry_parse(path, "registry exceeds the size limit"));
        }

        // The bounded reader also closes the metadata/read race: if a file
        // grows after the pre-read check, at most limit + 1 bytes are read and
        // `from_json_str` rejects the resulting document.
        let mut json = String::with_capacity(metadata.len() as usize);
        File::open(path)?
            .take((MAX_REGISTRY_BYTES + 1) as u64)
            .read_to_string(&mut json)?;
        Self::from_json_str(&json, path)
    }

    /// The seeded registry bundled with this crate.
    #[allow(clippy::expect_used)] // seed JSON is static crate data; unit tests pin its shape
    pub fn seeded() -> Self {
        Self::from_json_str(SEED_REGISTRY_JSON, Path::new("<seeded>"))
            .expect("bundled seed registry must stay parseable")
    }

    pub fn entries(&self) -> &[RegistryEntry] {
        &self.file.entries
    }

    pub fn get(&self, id: &str) -> Result<&RegistryEntry> {
        self.file
            .entries
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| ModelManagerError::UnknownModel(id.to_string()))
    }
}

fn validate_entry(entry: &RegistryEntry) -> Result<()> {
    for (value, reason) in [
        (entry.id.as_str(), "id is not a safe path component"),
        (entry.family.as_str(), "family is not a safe path component"),
        (entry.variant.as_str(), "variant is invalid"),
        (entry.quant.as_str(), "quantization label is invalid"),
    ] {
        if !safe_component(value) {
            return Err(ModelManagerError::InvalidRegistryEntry {
                id: entry.id.clone(),
                reason,
            });
        }
    }
    if entry.size_bytes == 0 || entry.size_bytes > 4 * 1024 * 1024 * 1024 {
        return Err(ModelManagerError::InvalidRegistryEntry {
            id: entry.id.clone(),
            reason: "size is outside the supported model bound",
        });
    }
    if let Some(digest) = entry.sha256.as_deref()
        && (digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(ModelManagerError::InvalidRegistryEntry {
            id: entry.id.clone(),
            reason: "sha256 is malformed",
        });
    }
    if entry.license.is_empty()
        || entry.license.len() > MAX_LICENSE_BYTES
        || entry.license.chars().any(char::is_control)
        || !safe_component(&entry.source_revision)
        || entry.sources.is_empty()
        || entry.sources.len() > MAX_SOURCES_PER_ENTRY
    {
        return Err(ModelManagerError::InvalidRegistryEntry {
            id: entry.id.clone(),
            reason: "license, revision, or source metadata is incomplete",
        });
    }
    if entry.capabilities.languages.len() > MAX_LANGUAGES_PER_ENTRY
        || entry.capabilities.languages.iter().any(|language| {
            language.is_empty()
                || language.len() > MAX_LANGUAGE_BYTES
                || !language
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
    {
        return Err(ModelManagerError::InvalidRegistryEntry {
            id: entry.id.clone(),
            reason: "language capabilities exceed their supported bounds",
        });
    }
    for source in &entry.sources {
        if !safe_relative_file(&source.filename)
            || source.repo_or_url.is_empty()
            || source.repo_or_url.len() > MAX_SOURCE_LOCATION_BYTES
            || source.filename.len() > MAX_SOURCE_LOCATION_BYTES
            || source.repo_or_url.chars().any(char::is_control)
        {
            return Err(ModelManagerError::InvalidRegistryEntry {
                id: entry.id.clone(),
                reason: "source metadata contains an unsafe or oversized field",
            });
        }
        match source.kind {
            SourceKind::Hf => validate_hf_source(entry, source)?,
            SourceKind::Url => validate_direct_url(entry, &source.repo_or_url)?,
        }
    }
    Ok(())
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn safe_relative_file(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && value.len() <= 255
        && !path.is_absolute()
        && path.components().all(|component| {
            matches!(component, Component::Normal(part) if part.to_str().is_some_and(safe_component))
        })
}

fn validate_hf_source(entry: &RegistryEntry, source: &SourceSpec) -> Result<()> {
    let mut components = source.repo_or_url.split('/');
    let owner = components.next();
    let repository = components.next();
    let exactly_two_safe_components = owner.is_some_and(safe_component)
        && repository.is_some_and(safe_component)
        && components.next().is_none();
    let immutable_revision = entry.source_revision.len() == 40
        && entry
            .source_revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit());
    if !exactly_two_safe_components || !immutable_revision {
        return Err(ModelManagerError::InvalidRegistryEntry {
            id: entry.id.clone(),
            reason: "Hugging Face source must use owner/repository and a 40-hex revision",
        });
    }
    Ok(())
}

fn validate_direct_url(entry: &RegistryEntry, url: &str) -> Result<()> {
    // URL validation does not replace artifact identity. `ModelStore::acquire`
    // independently requires the entry's digest before resolving or fetching
    // this location, preserving the existing typed ChecksumRequired outcome.
    let valid = if let Some(rest) = url.strip_prefix("https://") {
        valid_https_url(rest)
    } else if let Some(rest) = url.strip_prefix("file://") {
        valid_file_url(rest)
    } else {
        false
    };
    if !valid {
        return Err(ModelManagerError::InvalidRegistryEntry {
            id: entry.id.clone(),
            reason: "direct source URL is malformed or uses a forbidden scheme",
        });
    }
    Ok(())
}

fn valid_https_url(rest: &str) -> bool {
    if rest.is_empty()
        || rest.chars().any(|character| {
            character.is_control() || character.is_whitespace() || character == '\\'
        })
    {
        return false;
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    valid_https_authority(&rest[..authority_end])
}

fn valid_https_authority(authority: &str) -> bool {
    if authority.is_empty() || authority.len() > 253 || authority.contains('@') {
        return false;
    }

    if let Some(ipv6) = authority.strip_prefix('[') {
        let Some(close) = ipv6.find(']') else {
            return false;
        };
        let host = &ipv6[..close];
        let suffix = &ipv6[close + 1..];
        return host.parse::<std::net::Ipv6Addr>().is_ok() && valid_optional_port(suffix);
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host, Some(port)),
        Some(_) => return false,
        None => (authority, None),
    };
    !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
        && port.is_none_or(valid_port)
}

fn valid_optional_port(suffix: &str) -> bool {
    suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_port)
}

fn valid_port(port: &str) -> bool {
    port.parse::<u16>().is_ok()
}

fn valid_file_url(rest: &str) -> bool {
    !rest.is_empty()
        && !rest.contains(['?', '#', '\\'])
        && !rest.chars().any(|character| character.is_control())
        && Path::new(rest).is_absolute()
        && Path::new(rest)
            .components()
            .all(|component| !matches!(component, Component::ParentDir))
}

fn registry_parse(path: &Path, message: &str) -> ModelManagerError {
    ModelManagerError::RegistryParse {
        path: path.to_path_buf(),
        message: message.to_string(),
    }
}
