//! OpenVINO speech engine.
//!
//! The daemon never loads OpenVINO. It starts `narrivy-openvino-worker`, a
//! separate process that speaks the shared resident protocol
//! ([`narrivy_asr_api::resident_protocol`]), and the worker loads the
//! OpenVINO runtime from a runtime pack the user installed from the Store.
//!
//! This library holds the parts both sides need and that touch no native
//! code: the pack manifests (what files a model or runtime pack has, and
//! their sizes and SHA-256 digests), their verification, and the runtime
//! layout.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[cfg(feature = "worker")]
#[allow(unsafe_code)]
pub mod genai;
pub mod refine_protocol;
#[cfg(feature = "worker")]
#[allow(unsafe_code)]
pub mod static_llm;

/// The manifest file at the root of every OpenVINO model pack.
pub const MODEL_MANIFEST: &str = "model-manifest.json";
/// The manifest file at the root of an installed runtime pack.
pub const RUNTIME_MANIFEST: &str = "runtime-manifest.json";
/// Where a runtime pack keeps its libraries.
pub const RUNTIME_LIB_DIR: &str = "lib";
/// The runtime version this worker is built for (D21: OpenVINO 2026.4).
pub const RUNTIME_VERSION: &str = "2026.4";
/// The runtime pack's id, as the Store and the install directory name it.
pub const RUNTIME_PACK_ID: &str = "openvino-runtime-2026.4-linux-x86_64";

/// The threading library, loaded first from the runtime pack.
pub const TBB_LIBRARY: &str = "libtbb.so.12";
/// The OpenVINO libraries the worker loads after it, in dependency order,
/// from the runtime pack's `lib` directory, without their version suffix
/// (`.2640` for 2026.4: see [`runtime_soname`]). Loading them by full path
/// first means the libraries that name each other by soname find the
/// copies in the pack, without a search path. The NPU driver pair is
/// optional: without it the NPU is simply not offered.
pub const RUNTIME_LIBRARY_STEMS: &[&str] = &[
    "libopenvino.so",
    "libopenvino_c.so",
    "libopenvino_gguf_frontend.so",
    "libopenvino_genai.so",
    "libopenvino_genai_c.so",
];
/// Of those, the ones only some OpenVINO releases have (GenAI links the
/// GGUF frontend from 2026.5 on): loaded when the pack has them.
pub const OPTIONAL_RUNTIME_LIBRARY_STEMS: &[&str] = &["libopenvino_gguf_frontend.so"];

/// The version suffix of the OpenVINO in a runtime pack's `lib` directory
/// (`2640` for 2026.4): the one `libopenvino.so.<digits>` there. `None`
/// when there is none or more than one. The pack itself was verified
/// against its manifest before the worker is started on it.
#[must_use]
pub fn runtime_soname(lib_dir: &Path) -> Option<String> {
    let mut found = None;
    for entry in std::fs::read_dir(lib_dir).ok()? {
        let name = entry.ok()?.file_name();
        let Some(suffix) = name
            .to_str()
            .and_then(|n| n.strip_prefix("libopenvino.so."))
        else {
            continue;
        };
        if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if found.replace(suffix.to_owned()).is_some() {
            return None;
        }
    }
    found
}
/// The Level Zero loader and Intel's NPU user-space driver.
pub const NPU_DRIVER_LIBRARIES: &[&str] = &["libze_loader.so.1", "libze_intel_npu.so.1"];

/// Largest manifest accepted, so a damaged file cannot exhaust memory.
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// Most files one pack may list.
pub const MAX_PACK_FILES: usize = 512;

/// Why a pack failed verification. Content-free: no paths or text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackError {
    /// The manifest is missing, unreadable, too large or malformed.
    Manifest,
    /// A listed path is absolute, climbs out of the pack, or is not plain.
    UnsafePath,
    /// A listed file is missing, not a regular file, or a link.
    Missing,
    /// A listed file has the wrong size.
    Size,
    /// A listed file has the wrong SHA-256.
    Digest,
}

impl PackError {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Manifest => "pack_manifest_invalid",
            Self::UnsafePath => "pack_path_unsafe",
            Self::Missing => "pack_file_missing",
            Self::Size => "pack_file_size_mismatch",
            Self::Digest => "pack_file_digest_mismatch",
        }
    }
}

impl std::fmt::Display for PackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for PackError {}

/// One file a pack contains.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PackFile {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// The parts of a model or runtime manifest verification needs. Other
/// fields (source, licence, display name) are kept by the files themselves
/// and ignored here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PackManifest {
    pub schema_version: u32,
    pub id: String,
    pub files: Vec<PackFile>,
}

impl PackManifest {
    /// Parse a manifest and check its shape: a known schema, a bounded file
    /// list, safe relative paths and well-formed digests.
    pub fn parse(bytes: &[u8]) -> Result<Self, PackError> {
        let manifest: Self = serde_json::from_slice(bytes).map_err(|_| PackError::Manifest)?;
        if manifest.schema_version != 1
            || manifest.files.is_empty()
            || manifest.files.len() > MAX_PACK_FILES
        {
            return Err(PackError::Manifest);
        }
        for file in &manifest.files {
            if !is_plain_relative(&file.path) {
                return Err(PackError::UnsafePath);
            }
            if parse_sha256(&file.sha256).is_none() {
                return Err(PackError::Manifest);
            }
        }
        Ok(manifest)
    }

    /// Total bytes of every listed file.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files
            .iter()
            .fold(0_u64, |sum, file| sum.saturating_add(file.size_bytes))
    }

    /// Check every listed file under `root` has its listed size and digest.
    /// Links are refused: the pack must hold real files.
    pub fn verify(&self, root: &Path) -> Result<(), PackError> {
        for file in &self.files {
            verify_file(&root.join(&file.path), file)?;
        }
        Ok(())
    }
}

/// Read and parse a manifest file, refusing links and oversized files.
pub fn read_manifest(path: &Path) -> Result<PackManifest, PackError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| PackError::Manifest)?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(PackError::Manifest);
    }
    let bytes = std::fs::read(path).map_err(|_| PackError::Manifest)?;
    PackManifest::parse(&bytes)
}

/// One file against its listing.
pub fn verify_file(path: &Path, expected: &PackFile) -> Result<(), PackError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| PackError::Missing)?;
    if !metadata.is_file() {
        return Err(PackError::Missing);
    }
    if metadata.len() != expected.size_bytes {
        return Err(PackError::Size);
    }
    let digest = sha256_file(path).map_err(|_| PackError::Missing)?;
    if Some(digest) != parse_sha256(&expected.sha256) {
        return Err(PackError::Digest);
    }
    Ok(())
}

/// SHA-256 of a file's contents.
pub fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().into())
}

/// Lower-case hex digest to bytes.
#[must_use]
pub fn parse_sha256(value: &str) -> Option<[u8; 32]> {
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

/// A relative path made only of ordinary names: no root, no `..`, no `.`,
/// at most two levels deep, and written in exactly that form. Each name is
/// checked as written, not as `Path` normalises it, because callers split
/// the path on `/` themselves: `a//b` or `a/./b` would hand them an empty
/// or `.` name, and `/b` joined to a directory replaces it.
#[must_use]
pub fn is_plain_relative(path: &str) -> bool {
    if path.is_empty() || path.len() > 255 || path.contains('\\') {
        return false;
    }
    let names: Vec<&str> = path.split('/').collect();
    names.len() <= 2
        && names.iter().all(|name| {
            !name.is_empty()
                && *name != "."
                && *name != ".."
                && !name.chars().any(char::is_control)
                && matches!(
                    Path::new(name).components().collect::<Vec<_>>()[..],
                    [Component::Normal(_)]
                )
        })
}

/// Where an installed runtime pack's libraries are.
#[must_use]
pub fn runtime_lib_dir(runtime_root: &Path) -> PathBuf {
    runtime_root.join(RUNTIME_LIB_DIR)
}

/// The languages a Whisper model pack can be told to use, read from its
/// `generation_config.json` (`lang_to_id` keys such as `<|en|>`). An
/// English-only model has no such table and yields an empty list.
#[must_use]
pub fn whisper_languages(generation_config: &[u8]) -> Vec<String> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(generation_config) else {
        return Vec::new();
    };
    let Some(table) = value
        .get("lang_to_id")
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };
    let mut languages: Vec<String> = table
        .keys()
        .filter_map(|key| key.strip_prefix("<|")?.strip_suffix("|>"))
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 8
                && code.bytes().all(|byte| byte.is_ascii_lowercase())
        })
        .map(str::to_owned)
        .collect();
    languages.sort();
    languages.dedup();
    languages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_version_suffix_comes_from_the_one_core_library_in_the_pack()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        assert_eq!(runtime_soname(dir.path()), None);
        for name in [
            "libopenvino.so.2650",
            "libopenvino_c.so.2650",
            "libopenvino.so",
            "libtbb.so.12",
        ] {
            std::fs::write(dir.path().join(name), b"")?;
        }
        assert_eq!(runtime_soname(dir.path()).as_deref(), Some("2650"));
        // Two versions side by side: none is chosen.
        std::fs::write(dir.path().join("libopenvino.so.2640"), b"")?;
        assert_eq!(runtime_soname(dir.path()), None);
        Ok(())
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn manifest_for(files: &[(&str, &[u8])]) -> String {
        let listed: Vec<serde_json::Value> = files
            .iter()
            .map(|(path, bytes)| {
                let digest: [u8; 32] = Sha256::digest(bytes).into();
                serde_json::json!({
                    "path": path,
                    "sha256": digest.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                    "size_bytes": bytes.len(),
                    "optional_signature": null,
                })
            })
            .collect();
        serde_json::json!({ "schema_version": 1, "id": "narrivy/test", "files": listed })
            .to_string()
    }

    #[test]
    fn a_pack_verifies_only_when_every_file_matches() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("a.bin"), b"alpha")?;
        std::fs::create_dir(dir.path().join("lib"))?;
        std::fs::write(dir.path().join("lib/b.so"), b"beta")?;
        let manifest = PackManifest::parse(
            manifest_for(&[("a.bin", b"alpha"), ("lib/b.so", b"beta")]).as_bytes(),
        )?;
        assert_eq!(manifest.total_bytes(), 9);
        assert_eq!(manifest.verify(dir.path()), Ok(()));

        std::fs::write(dir.path().join("a.bin"), b"alphA")?;
        assert_eq!(manifest.verify(dir.path()), Err(PackError::Digest));
        std::fs::write(dir.path().join("a.bin"), b"alpha!")?;
        assert_eq!(manifest.verify(dir.path()), Err(PackError::Size));
        std::fs::remove_file(dir.path().join("a.bin"))?;
        std::os::unix::fs::symlink(dir.path().join("lib/b.so"), dir.path().join("a.bin"))?;
        assert_eq!(manifest.verify(dir.path()), Err(PackError::Missing));
        Ok(())
    }

    #[test]
    fn unsafe_paths_and_bad_digests_are_refused() {
        for path in [
            "/etc/passwd",
            "../x",
            "a/../b",
            "./a",
            "a/b/c",
            "",
            "a\\b",
            // `Path` normalises these to two plain names; the callers that
            // split on `/` would not (`a//b` gave them `/b`, an absolute path).
            "a//b",
            "a/./b",
            "a/",
            "a/.",
            "/",
            "..",
            "a\0b",
            "a\nb",
        ] {
            let text = manifest_for(&[(path, b"x")]);
            assert!(PackManifest::parse(text.as_bytes()).is_err(), "{path}");
        }
        let text =
            r#"{"schema_version":1,"id":"x","files":[{"path":"a","sha256":"zz","size_bytes":1}]}"#;
        assert_eq!(
            PackManifest::parse(text.as_bytes()),
            Err(PackError::Manifest)
        );
        let empty = r#"{"schema_version":1,"id":"x","files":[]}"#;
        assert_eq!(
            PackManifest::parse(empty.as_bytes()),
            Err(PackError::Manifest)
        );
    }

    #[test]
    fn whisper_languages_come_from_the_generation_config() {
        let config = br#"{"lang_to_id": {"<|en|>": 50259, "<|fr|>": 50265, "<|haw|>": 50300, "<|bad code|>": 1}}"#;
        assert_eq!(whisper_languages(config), ["en", "fr", "haw"]);
        assert!(whisper_languages(br#"{"no_table": 1}"#).is_empty());
        assert!(whisper_languages(b"not json").is_empty());
    }

    /// Pack entries that are not regular files are refused: a link, a
    /// device and a socket, each with otherwise matching listings.
    #[test]
    fn links_devices_and_sockets_are_not_pack_files() -> TestResult {
        let dir = tempfile::tempdir()?;
        let listed = |path: &str| PackFile {
            path: path.to_owned(),
            sha256: "0".repeat(64),
            size_bytes: 0,
        };
        std::os::unix::fs::symlink("/dev/null", dir.path().join("link"))?;
        let _socket = std::os::unix::net::UnixListener::bind(dir.path().join("socket"))?;
        std::fs::write(dir.path().join("real"), b"")?;
        std::fs::hard_link(dir.path().join("real"), dir.path().join("hard"))?;
        for name in ["link", "socket"] {
            assert_eq!(
                verify_file(&dir.path().join(name), &listed(name)),
                Err(PackError::Missing),
                "{name}"
            );
        }
        assert_eq!(
            verify_file(Path::new("/dev/null"), &listed("null")),
            Err(PackError::Missing)
        );
        // A hard link is the file itself: it is held to the same digest.
        assert_eq!(
            verify_file(&dir.path().join("hard"), &listed("hard")),
            Err(PackError::Digest)
        );
        Ok(())
    }

    /// Huge declared sizes neither overflow the total nor pass a check.
    #[test]
    fn huge_declared_sizes_saturate_and_fail_verification() -> TestResult {
        let digest = "0".repeat(64);
        let text = format!(
            r#"{{"schema_version":1,"id":"x","files":[{{"path":"a","sha256":"{digest}","size_bytes":{max}}},{{"path":"b","sha256":"{digest}","size_bytes":{max}}}]}}"#,
            max = u64::MAX
        );
        let manifest = PackManifest::parse(text.as_bytes())?;
        assert_eq!(manifest.total_bytes(), u64::MAX);
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("a"), b"x")?;
        assert_eq!(manifest.verify(dir.path()), Err(PackError::Size));
        Ok(())
    }

    /// `cases` per property, unless `PROPTEST_CASES` asks for more (see
    /// `scripts/fuzz-untrusted-input.sh`). No regression files are written.
    fn fuzz_config(cases: u32) -> proptest::prelude::ProptestConfig {
        let mut config = proptest::prelude::ProptestConfig {
            failure_persistence: None,
            ..proptest::prelude::ProptestConfig::default()
        };
        if std::env::var_os("PROPTEST_CASES").is_none() {
            config.cases = cases;
        }
        config
    }

    proptest::proptest! {
        #![proptest_config(fuzz_config(512))]

        /// Whatever passes `is_plain_relative` stays under the pack root,
        /// both joined whole and split on `/` the way the installer does.
        #[test]
        fn plain_relative_paths_stay_under_the_root(path in r"[a./\\\x00\n-]{0,8}") {
            if is_plain_relative(&path) {
                let root = Path::new("/pack");
                proptest::prop_assert!(root.join(&path).starts_with(root));
                let (dir, name) = path.split_once('/').unwrap_or(("", path.as_str()));
                for part in [dir, name] {
                    proptest::prop_assert!(!part.starts_with('/'));
                    proptest::prop_assert!(part != "." && part != "..");
                }
                proptest::prop_assert!(!name.is_empty());
                proptest::prop_assert!(root.join(dir).join(name).starts_with(root));
                proptest::prop_assert!(Path::new(&path)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))));
            }
        }

        /// Any bytes as a manifest: parsed or refused, never a panic.
        #[test]
        fn manifest_parse_never_panics(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256)) {
            let _ = PackManifest::parse(&bytes);
        }
    }
}
