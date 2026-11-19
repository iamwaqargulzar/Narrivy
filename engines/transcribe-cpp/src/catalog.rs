//! Catalog of GGUF model variants the transcribe.cpp lane is expected to run.
//!
//! PORT-008: models are versioned resources, not product entities. Entries are
//! metadata only — this crate never embeds URLs, download locations, or
//! weights. Unselected entries remain provisional until the model lock. A
//! controlled qualification canary may carry an exact revision/hash/size
//! without becoming a default or release-qualified model.

use std::borrow::Cow;

/// One cataloged GGUF model variant for the transcribe.cpp runtime lane.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GgufModelSpec {
    /// Model family (e.g. `whisper`, `parakeet`, `moonshine`, `vosk`).
    pub family: Cow<'static, str>,
    /// Family-internal variant name (e.g. `large-v3-turbo`).
    pub variant: Cow<'static, str>,
    /// GGUF quantization label; provisional until the P5 lock.
    pub quant: Cow<'static, str>,
    /// On-disk size in bytes; approximate unless the entry has an integrity pin.
    pub size_bytes: u64,
    /// Integrity digest of the exact weight file, when pinned.
    pub sha256: Option<Cow<'static, str>>,
    /// Upstream source revision this entry is pinned to.
    ///
    /// Empty for provisional entries; immutable repository revision when pinned.
    pub source_revision: Cow<'static, str>,
    /// SPDX-style license identifier of the weight artifact.
    ///
    /// Note: weight licenses are independent of the transcribe.cpp code
    /// license; both must pass review before any gate uses the `runtime`
    /// feature (see ../BUILD-NOTES.md).
    pub license: Cow<'static, str>,
}

const fn pinned_spec(
    family: &'static str,
    variant: &'static str,
    quant: &'static str,
    size_bytes: u64,
    sha256: &'static str,
    source_revision: &'static str,
    license: &'static str,
) -> GgufModelSpec {
    GgufModelSpec {
        family: Cow::Borrowed(family),
        variant: Cow::Borrowed(variant),
        quant: Cow::Borrowed(quant),
        size_bytes,
        sha256: Some(Cow::Borrowed(sha256)),
        source_revision: Cow::Borrowed(source_revision),
        license: Cow::Borrowed(license),
    }
}

impl GgufModelSpec {
    /// Stable resource identity `<family>/<variant>` (PORT-008 addressing).
    pub fn id(&self) -> String {
        format!("{}/{}", self.family, self.variant)
    }
}

const fn spec(
    family: &'static str,
    variant: &'static str,
    quant: &'static str,
    size_bytes: u64,
    license: &'static str,
) -> GgufModelSpec {
    GgufModelSpec {
        family: Cow::Borrowed(family),
        variant: Cow::Borrowed(variant),
        quant: Cow::Borrowed(quant),
        size_bytes,
        sha256: None,
        // Empty until the P5 lock; see field documentation.
        source_revision: Cow::Borrowed(""),
        license: Cow::Borrowed(license),
    }
}

/// Well-known-family GGUF variants targeted by the owner decision of
/// 2026-08-22. Sizes/quants are approximate placeholders pending the P5 lock.
pub const MODEL_CATALOG: &[GgufModelSpec] = &[
    // OpenAI Whisper family (weights Apache-2.0).
    spec("whisper", "large-v3", "Q5_K_M", 1_680_000_000, "Apache-2.0"),
    spec(
        "whisper",
        "large-v3-turbo",
        "Q5_K_M",
        600_000_000,
        "Apache-2.0",
    ),
    spec("whisper", "medium", "Q5_K_M", 520_000_000, "Apache-2.0"),
    spec("whisper", "small", "Q5_K_M", 190_000_000, "Apache-2.0"),
    spec("whisper", "base", "Q5_K_M", 80_000_000, "Apache-2.0"),
    spec("whisper", "tiny", "Q5_K_M", 31_000_000, "Apache-2.0"),
    // Distil-Whisper large-v3 distillation (weights MIT).
    spec("whisper", "distil-large-v3", "Q5_K_M", 800_000_000, "MIT"),
    // Parakeet TDT 0.6B multilingual v3 (NVIDIA weights CC-BY-4.0).
    spec(
        "parakeet",
        "parakeet-tdt-0.6b-v3",
        "Q8_0",
        450_000_000,
        "CC-BY-4.0",
    ),
    // Moonshine v2 (Apache-2.0).
    spec(
        "moonshine",
        "moonshine-v2-tiny",
        "Q8_0",
        30_000_000,
        "Apache-2.0",
    ),
    spec(
        "moonshine",
        "moonshine-v2-base",
        "Q8_0",
        70_000_000,
        "Apache-2.0",
    ),
    // Distinct English-only native-streaming family (MIT weights).
    pinned_spec(
        "moonshine_streaming",
        "moonshine-streaming-tiny",
        "Q8_0",
        50_462_816,
        "930e4622ad3a24158b91406c30c977fa6a26b34cb32d6ac3e57cfb23383a869e",
        "85ddff612fa3a2cf40b2f745abcfa90ef82f293b",
        "MIT",
    ),
    // Vosk small-class (Apache-2.0).
    spec("vosk", "vosk-small", "Q8_0", 45_000_000, "Apache-2.0"),
];

/// Failure of a catalog lookup (typed; no panics).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("model '{family}/{variant}' is not in the transcribe.cpp GGUF catalog")]
pub struct CatalogError {
    pub family: String,
    pub variant: String,
}

/// Looks up one catalog entry by family and variant name.
pub fn lookup(family: &str, variant: &str) -> Result<&'static GgufModelSpec, CatalogError> {
    MODEL_CATALOG
        .iter()
        .find(|entry| entry.family.as_ref() == family && entry.variant.as_ref() == variant)
        .ok_or_else(|| CatalogError {
            family: family.to_owned(),
            variant: variant.to_owned(),
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_round_trips_through_serde() {
        let json = serde_json::to_string(MODEL_CATALOG).unwrap();
        let back: Vec<GgufModelSpec> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), MODEL_CATALOG.len());
        for (original, restored) in MODEL_CATALOG.iter().zip(back.iter()) {
            assert_eq!(original, restored);
            // Owned deserialization must not change observable identity data.
            assert_eq!(restored.id(), original.id());
        }
    }

    #[test]
    fn only_the_controlled_canary_is_integrity_pinned() {
        for entry in MODEL_CATALOG {
            if entry.id() == "moonshine_streaming/moonshine-streaming-tiny" {
                assert_eq!(entry.size_bytes, 50_462_816);
                assert_eq!(
                    entry.sha256.as_deref(),
                    Some("930e4622ad3a24158b91406c30c977fa6a26b34cb32d6ac3e57cfb23383a869e")
                );
                assert_eq!(
                    entry.source_revision,
                    "85ddff612fa3a2cf40b2f745abcfa90ef82f293b"
                );
            } else {
                assert!(
                    entry.sha256.is_none(),
                    "{} has an unreviewed pin",
                    entry.id()
                );
                assert!(entry.source_revision.is_empty(), "{} floats", entry.id());
            }
        }
    }

    #[test]
    fn licenses_are_non_empty() {
        for entry in MODEL_CATALOG {
            assert!(!entry.license.is_empty(), "{} lacks a license", entry.id());
        }
    }

    #[test]
    fn entries_carry_no_url_fields_port008() {
        let json = serde_json::to_string(MODEL_CATALOG).unwrap().to_lowercase();
        assert!(
            !json.contains("url"),
            "catalog leaked a URL-like field: {json}"
        );
        assert!(!json.contains("http"), "catalog leaked a URL value");
        for entry in MODEL_CATALOG {
            let one = serde_json::to_value(entry).unwrap();
            let map = one.as_object().unwrap();
            for key in map.keys() {
                assert!(
                    !key.to_lowercase().contains("url"),
                    "{key} violates PORT-008"
                );
            }
        }
    }

    #[test]
    fn lookup_hits_known_variants_and_misses_unknowns() {
        assert_eq!(
            lookup("whisper", "tiny").unwrap().license.as_ref(),
            "Apache-2.0"
        );
        assert_eq!(
            lookup("parakeet", "parakeet-tdt-0.6b-v3")
                .unwrap()
                .quant
                .as_ref(),
            "Q8_0"
        );
        let miss = lookup("whisper", "nope").unwrap_err();
        assert_eq!(miss.family, "whisper");
        assert_eq!(miss.variant, "nope");
    }
}
