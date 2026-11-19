//! Language-model packs downloaded from their upstream publisher (D58).
//!
//! Some assistant models are fetched straight from the OpenVINO
//! organisation's Hugging Face exports, at a pinned 40-hex commit, instead of
//! a copy in `narrivy/store`. The upstream repository has no Narrivy
//! manifest, so the manifest and the prompt profile are built into the
//! daemon here (made by `scripts/build_llm_pack.py --upstream-seed`). The
//! catalog pins the manifest's size and SHA-256 exactly as for a Store pack,
//! the manifest pins every file, and nothing fetched is trusted until it
//! matches.

/// Organisations whose exports may be downloaded directly. Only packs with a
/// built-in manifest below are accepted from them.
pub const UPSTREAM_ORGS: [&str; 1] = ["OpenVINO"];

/// `(pack id, file path, bytes)` for every built-in file.
const FILES: [(&str, &str, &str); 4] = [
    (
        "gemma-4-e4b-openvino",
        crate::download::MODEL_PACK_MANIFEST,
        include_str!("../seed/upstream/gemma-4-e4b-openvino/model-manifest.json"),
    ),
    (
        "gemma-4-e4b-openvino",
        "narrivy-refine.json",
        include_str!("../seed/upstream/gemma-4-e4b-openvino/narrivy-refine.json"),
    ),
    (
        "qwen3.5-9b-openvino",
        crate::download::MODEL_PACK_MANIFEST,
        include_str!("../seed/upstream/qwen3.5-9b-openvino/model-manifest.json"),
    ),
    (
        "qwen3.5-9b-openvino",
        "narrivy-refine.json",
        include_str!("../seed/upstream/qwen3.5-9b-openvino/narrivy-refine.json"),
    ),
];

/// A file of pack `id` that Narrivy itself supplies (its manifest and
/// prompt profile), or `None` when the file comes from the network.
#[must_use]
pub fn built_in_file(id: &str, path: &str) -> Option<&'static [u8]> {
    FILES
        .iter()
        .find(|(pack, name, _)| *pack == id && *name == path)
        .map(|(_, _, text)| text.as_bytes())
}

/// Whether pack `id` has a built-in manifest, so it may come from upstream.
#[must_use]
pub fn has_built_in_manifest(id: &str) -> bool {
    built_in_file(id, crate::download::MODEL_PACK_MANIFEST).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn every_upstream_pack_in_the_catalog_matches_its_built_in_manifest() -> Result<(), String> {
        let upstream: Vec<_> = crate::catalog::language_model_catalog()
            .into_iter()
            .filter(|model| {
                UPSTREAM_ORGS
                    .iter()
                    .any(|org| model.source_repo.starts_with(&format!("{org}/")))
            })
            .collect();
        assert_eq!(upstream.len(), 2);
        for model in upstream {
            let manifest = built_in_file(&model.id, crate::download::MODEL_PACK_MANIFEST)
                .ok_or_else(|| format!("{} has no built-in manifest", model.id))?;
            assert_eq!(manifest.len() as u64, model.manifest_size, "{}", model.id);
            assert_eq!(
                hex::encode(sha2::Sha256::digest(manifest)),
                model.manifest_sha256,
                "{}",
                model.id
            );
            let parsed: serde_json::Value =
                serde_json::from_slice(manifest).map_err(|e| e.to_string())?;
            assert_eq!(parsed["id"], model.id.as_str());
            assert_eq!(parsed["source"]["locator"], model.source_repo.as_str());
            assert_eq!(
                parsed["source"]["immutable_revision"],
                model.source_revision.as_deref().unwrap_or_default()
            );
            let files = parsed["files"].as_array().cloned().unwrap_or_default();
            let total: u64 = files
                .iter()
                .filter_map(|file| file["size_bytes"].as_u64())
                .sum();
            assert_eq!(total, model.size_bytes, "{}", model.id);
            // The profile is listed, and the built-in copy is what it pins.
            let bytes = built_in_file(&model.id, "narrivy-refine.json").unwrap_or_default();
            assert!(
                files
                    .iter()
                    .any(|file| file["path"] == "narrivy-refine.json"
                        && file["sha256"] == hex::encode(sha2::Sha256::digest(bytes)).as_str()
                        && file["size_bytes"].as_u64() == Some(bytes.len() as u64)),
                "{}",
                model.id
            );
            // D58: never the NPU.
            assert_eq!(parsed["devices"], serde_json::json!(["GPU", "CPU"]));
            assert_eq!(model.devices, ["GPU", "CPU"]);
        }
        Ok(())
    }
}
