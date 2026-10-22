//! What may be downloaded, and from where: pure rules, no network.
//!
//! A model is downloaded only when the user asks for it. The rules here keep
//! that download narrow (locked strategy, ADR-0017):
//!
//! - only single-file catalog models published in the `narrivy` Hugging Face
//!   organisation, at their pinned 40-hex revision, with a pinned size and
//!   SHA-256;
//! - only `https`, only to Hugging Face hosts, including every redirect;
//! - the URL carries nothing but the repository, revision and the file's
//!   path in the repository (plain names only).
//!
//! The bytes are trusted only after they match the pinned size and SHA-256;
//! the host rules limit where the computer connects, not what is accepted.

use crate::catalog::{CatalogModel, CatalogRuntime, ENGINE_OPENVINO, ENGINE_TRANSCRIBE_CPP};

/// The organisation models are published in.
pub const PUBLISHER_ORG: &str = "narrivy";
const PRIMARY_HOST: &str = "huggingface.co";
/// Hugging Face serves large files from its own content hosts.
const ALLOWED_HOST_SUFFIXES: [&str; 2] = [".huggingface.co", ".hf.co"];
/// Redirect hops followed before giving up.
pub const MAX_REDIRECTS: usize = 5;
const MAX_URL_BYTES: usize = 4_096;

/// Why a catalog model cannot be downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadRefusal {
    /// It needs an engine that is not part of this build.
    NeedsEngine,
    /// It is not one file with a pinned size and hash.
    NotSingleVerifiedFile,
    /// It is not published by the Narrivy organisation at a pinned revision.
    NotPublishedByNarrivy,
}

/// One file to fetch and what it must turn out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadTarget {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// A folder or file path inside the Store repository: at most four plain
/// names (`models/openvino/<pack>/<file>`). Empty is the repository root.
fn is_store_path(value: &str) -> bool {
    value.is_empty() || {
        let parts: Vec<&str> = value.split('/').collect();
        parts.len() <= 4 && parts.iter().all(|part| is_name(part))
    }
}

fn is_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

impl DownloadTarget {
    /// The download for `model`, or why there is none.
    pub fn for_model(model: &CatalogModel) -> Result<Self, DownloadRefusal> {
        if model.engine != ENGINE_TRANSCRIBE_CPP {
            return Err(DownloadRefusal::NeedsEngine);
        }
        let (Some(filename), Some(sha256)) = (&model.filename, &model.sha256) else {
            return Err(DownloadRefusal::NotSingleVerifiedFile);
        };
        let pinned_hash = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
        let path_names_the_file =
            model.source_path == *filename || model.source_path.ends_with(&format!("/{filename}"));
        if !is_name(filename)
            || !is_store_path(&model.source_path)
            || !path_names_the_file
            || !pinned_hash
            || model.size_bytes == 0
        {
            return Err(DownloadRefusal::NotSingleVerifiedFile);
        }
        let repository = model
            .source_repo
            .strip_prefix(PUBLISHER_ORG)
            .and_then(|rest| rest.strip_prefix('/'))
            .filter(|name| is_name(name));
        let revision = model.source_revision.as_deref().unwrap_or_default();
        let pinned_revision =
            revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit());
        let (Some(repository), true) = (repository, pinned_revision) else {
            return Err(DownloadRefusal::NotPublishedByNarrivy);
        };
        Ok(Self {
            id: model.id.clone(),
            url: format!(
                "https://{PRIMARY_HOST}/{PUBLISHER_ORG}/{repository}/resolve/{revision}/{}",
                model.source_path
            ),
            filename: filename.clone(),
            size_bytes: model.size_bytes,
            sha256: sha256.to_ascii_lowercase(),
        })
    }
}

/// A pack of several files (an OpenVINO model or a runtime). Its manifest
/// is pinned by size and SHA-256 here, and the manifest pins every file, so
/// nothing fetched is trusted until it matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackTarget {
    pub id: String,
    /// The pack's folder in the Store repository, and in a local developer
    /// source laid out the same way.
    pub source_path: String,
    /// Where the pack's files are served, ending in `/`. `None` for a pack
    /// that is not published yet: it can only come from a local source.
    pub base_url: Option<String>,
    pub manifest_name: &'static str,
    pub manifest_sha256: String,
    pub manifest_size: u64,
    /// Total size of the files the manifest lists.
    pub size_bytes: u64,
}

/// The manifest file a model pack carries.
pub const MODEL_PACK_MANIFEST: &str = "model-manifest.json";
/// The manifest file a runtime pack carries.
pub const RUNTIME_PACK_MANIFEST: &str = "runtime-manifest.json";

fn pinned_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn narrivy_base_url(
    repo: &str,
    path: &str,
    revision: Option<&str>,
) -> Result<Option<String>, DownloadRefusal> {
    let repository = repo
        .strip_prefix(PUBLISHER_ORG)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|name| is_name(name))
        .ok_or(DownloadRefusal::NotPublishedByNarrivy)?;
    if path.is_empty() || !is_store_path(path) {
        return Err(DownloadRefusal::NotSingleVerifiedFile);
    }
    let Some(revision) = revision else {
        return Ok(None);
    };
    if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DownloadRefusal::NotPublishedByNarrivy);
    }
    Ok(Some(format!(
        "https://{PRIMARY_HOST}/{PUBLISHER_ORG}/{repository}/resolve/{revision}/{path}/"
    )))
}

impl PackTarget {
    /// The download for a multi-file model pack.
    pub fn for_model(model: &CatalogModel) -> Result<Self, DownloadRefusal> {
        if model.engine != ENGINE_OPENVINO {
            return Err(DownloadRefusal::NeedsEngine);
        }
        let (Some(sha256), Some(size)) = (&model.manifest_sha256, model.manifest_size) else {
            return Err(DownloadRefusal::NotSingleVerifiedFile);
        };
        if !pinned_hash(sha256) || size == 0 || !is_name(&model.id) {
            return Err(DownloadRefusal::NotSingleVerifiedFile);
        }
        let base_url = narrivy_base_url(
            &model.source_repo,
            &model.source_path,
            model.source_revision.as_deref(),
        )?;
        Ok(Self {
            id: model.id.clone(),
            source_path: model.source_path.clone(),
            base_url,
            manifest_name: MODEL_PACK_MANIFEST,
            manifest_sha256: sha256.to_ascii_lowercase(),
            manifest_size: size,
            size_bytes: model.size_bytes,
        })
    }

    /// The download for an accelerator runtime pack.
    pub fn for_runtime(runtime: &CatalogRuntime) -> Result<Self, DownloadRefusal> {
        if !pinned_hash(&runtime.manifest_sha256)
            || runtime.manifest_size == 0
            || !is_name(&runtime.id)
        {
            return Err(DownloadRefusal::NotSingleVerifiedFile);
        }
        let base_url = narrivy_base_url(
            &runtime.source_repo,
            &runtime.source_path,
            runtime.source_revision.as_deref(),
        )?;
        Ok(Self {
            id: runtime.id.clone(),
            source_path: runtime.source_path.clone(),
            base_url,
            manifest_name: RUNTIME_PACK_MANIFEST,
            manifest_sha256: runtime.manifest_sha256.to_ascii_lowercase(),
            manifest_size: runtime.manifest_size,
            size_bytes: runtime.size_bytes,
        })
    }

    /// The URL of one file of the pack: each part of `path` must be a plain
    /// name, so a manifest cannot point the download anywhere else.
    #[must_use]
    pub fn file_url(&self, path: &str) -> Option<String> {
        let base = self.base_url.as_deref()?;
        let parts: Vec<&str> = path.split('/').collect();
        (parts.len() <= 2 && parts.iter().all(|part| is_name(part)))
            .then(|| format!("{base}{path}"))
    }
}

/// The host of an `https` URL that carries no credentials and no explicit
/// port, or `None` for anything else.
fn https_host(url: &str) -> Option<&str> {
    if url.len() > MAX_URL_BYTES || url.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        return None;
    }
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains(['@', ':']) {
        return None;
    }
    Some(authority)
}

/// Whether the computer may connect to `url` for a model download.
#[must_use]
pub fn url_allowed(url: &str) -> bool {
    https_host(url).is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        host == PRIMARY_HOST
            || ALLOWED_HOST_SUFFIXES
                .iter()
                .any(|suffix| host.ends_with(suffix) && host.len() > suffix.len())
    })
}

/// Whether `url` is on the primary host, where a redirect may still follow.
/// Content hosts serve the file itself and are fetched directly.
#[must_use]
pub fn is_primary_host(url: &str) -> bool {
    https_host(url).is_some_and(|host| host.eq_ignore_ascii_case(PRIMARY_HOST))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::model_catalog;

    type TestResult = std::result::Result<(), String>;

    fn catalog(id: &str) -> std::result::Result<CatalogModel, String> {
        model_catalog()
            .into_iter()
            .find(|model| model.id == id)
            .ok_or_else(|| format!("{id} is not in the catalog"))
    }

    #[test]
    fn a_narrivy_single_file_model_downloads_from_its_pinned_revision() -> TestResult {
        let model = catalog("parakeet-unified-en-0.6b-Q8_0")?;
        let target = DownloadTarget::for_model(&model)
            .map_err(|refusal| format!("the published model has no download: {refusal:?}"))?;
        assert_eq!(
            target.url,
            format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                model.source_repo,
                model.source_revision.as_deref().ok_or("unpublished")?,
                model.source_path
            )
        );
        assert_eq!(target.size_bytes, model.size_bytes);
        assert!(url_allowed(&target.url) && is_primary_host(&target.url));
        assert_eq!(Some(target.sha256), model.sha256);
        Ok(())
    }

    #[test]
    fn a_pack_is_pinned_by_its_manifest_and_names_only_plain_files() -> TestResult {
        let mut model = catalog("whisper-large-v3-turbo-openvino-int8")?;
        assert_eq!(
            model.source_path,
            "models/openvino/whisper-large-v3-turbo-int8"
        );
        model.source_revision = Some("b".repeat(40));
        let pack = PackTarget::for_model(&model).map_err(|r| format!("{r:?}"))?;
        assert_eq!(pack.manifest_name, MODEL_PACK_MANIFEST);
        assert_eq!(pack.manifest_sha256.len(), 64);
        assert_eq!(pack.source_path, model.source_path);
        let url = pack
            .file_url("openvino_encoder_model.bin")
            .ok_or("no url")?;
        assert_eq!(
            url,
            format!(
                "https://huggingface.co/narrivy/store/resolve/{}/models/openvino/whisper-large-v3-turbo-int8/openvino_encoder_model.bin",
                "b".repeat(40)
            )
        );
        assert!(url_allowed(&url));
        for bad in ["", "../x", "models/../x", "a/b/c/d/e", "models/.x"] {
            let mut altered = model.clone();
            altered.source_path = bad.to_owned();
            assert_eq!(
                PackTarget::for_model(&altered),
                Err(DownloadRefusal::NotSingleVerifiedFile),
                "{bad}"
            );
        }
        for bad in ["../x", "a/../b", "/etc/passwd", "a/b/c", ".hidden", "a b"] {
            assert_eq!(pack.file_url(bad), None, "{bad}");
        }
        assert!(pack.file_url("lib/libopenvino.so.2640").is_some());

        let runtime = crate::catalog::runtime_catalog()
            .into_iter()
            .next()
            .ok_or("no runtime in the catalog")?;
        let published = PackTarget::for_runtime(&runtime).map_err(|r| format!("{r:?}"))?;
        assert!(
            published
                .file_url("lib/libtbb.so.12")
                .is_some_and(|url| url.contains("/runtimes/openvino/2026.4-linux-x86_64/lib/"))
        );
        let mut unpublished = runtime.clone();
        unpublished.source_revision = None;
        let unpublished = PackTarget::for_runtime(&unpublished).map_err(|r| format!("{r:?}"))?;
        assert_eq!(unpublished.base_url, None);
        assert_eq!(unpublished.file_url("lib/libtbb.so.12"), None);
        let mut published = runtime.clone();
        published.source_revision = Some("main".to_owned());
        assert_eq!(
            PackTarget::for_runtime(&published),
            Err(DownloadRefusal::NotPublishedByNarrivy)
        );
        assert_eq!(
            PackTarget::for_model(&catalog("parakeet-unified-en-0.6b-Q8_0")?),
            Err(DownloadRefusal::NeedsEngine)
        );
        Ok(())
    }

    #[test]
    fn other_publishers_packs_and_unpinned_models_are_refused() -> TestResult {
        assert_eq!(
            DownloadTarget::for_model(&catalog("whisper-tiny.en-Q8_0")?),
            Err(DownloadRefusal::NotPublishedByNarrivy)
        );
        assert_eq!(
            DownloadTarget::for_model(&catalog("whisper-large-v3-openvino-int8")?),
            Err(DownloadRefusal::NeedsEngine)
        );
        let good = catalog("parakeet-unified-en-0.6b-Q8_0")?;
        let refused = |change: fn(&mut CatalogModel)| {
            let mut model = good.clone();
            change(&mut model);
            DownloadTarget::for_model(&model).err()
        };
        assert_eq!(
            refused(|m| m.source_revision = Some("main".to_owned())),
            Some(DownloadRefusal::NotPublishedByNarrivy)
        );
        assert_eq!(
            refused(|m| m.source_repo = "narrivy/../other".to_owned()),
            Some(DownloadRefusal::NotPublishedByNarrivy)
        );
        assert_eq!(
            refused(|m| m.source_repo = "narrivyx/model".to_owned()),
            Some(DownloadRefusal::NotPublishedByNarrivy)
        );
        assert_eq!(
            refused(|m| m.filename = Some("../model.gguf".to_owned())),
            Some(DownloadRefusal::NotSingleVerifiedFile)
        );
        assert_eq!(
            refused(|m| m.source_path = "models/gguf/../x.gguf".to_owned()),
            Some(DownloadRefusal::NotSingleVerifiedFile)
        );
        assert_eq!(
            refused(|m| m.source_path = "models/gguf/other/other.gguf".to_owned()),
            Some(DownloadRefusal::NotSingleVerifiedFile)
        );
        assert_eq!(
            refused(|m| m.sha256 = None),
            Some(DownloadRefusal::NotSingleVerifiedFile)
        );
        assert_eq!(
            refused(|m| m.size_bytes = 0),
            Some(DownloadRefusal::NotSingleVerifiedFile)
        );
        Ok(())
    }

    #[test]
    fn only_https_to_hugging_face_hosts_is_allowed() {
        for allowed in [
            "https://huggingface.co/narrivy/x/resolve/abc/file.gguf",
            "https://cdn-lfs.hf.co/repos/aa/bb/cc?X-Amz-Signature=1",
            "https://cas-bridge.xethub.hf.co/xet-bridge-us/abc",
            "https://cdn-lfs-us-1.huggingface.co/x",
            "https://HuggingFace.co/x",
        ] {
            assert!(url_allowed(allowed), "{allowed}");
        }
        for refused in [
            "http://huggingface.co/x",
            "https://example.com/huggingface.co/x",
            "https://huggingface.co.example.com/x",
            "https://evilhf.co/x",
            "https://hf.co.evil.com/x",
            "https://user@huggingface.co/x",
            "https://huggingface.co@evil.com/x",
            "https://huggingface.co:8443/x",
            "https://.hf.co/x",
            "https:///x",
            "file:///etc/passwd",
            "https://huggingface.co/x y",
            "",
        ] {
            assert!(!url_allowed(refused), "{refused}");
        }
        assert!(is_primary_host("https://huggingface.co/a"));
        assert!(!is_primary_host("https://cdn-lfs.hf.co/a"));
    }
}
