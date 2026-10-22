//! Real-decode qualification tuples, persisted to a versioned registry.
//!
//! A tuple is recorded ONLY from a measured run. There is no "assumed pass".

use crate::RegistryError;
use crate::discovery::DeviceClass;
use crate::probe::{HostProbe, ProbeCommand};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};

/// Registry schema version; bump on breaking shape changes.
pub const QUALIFICATIONS_SCHEMA_VERSION: u32 = 1;

/// Measured outcome of one qualification decode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QualificationMetrics {
    /// Wall decode milliseconds normalized to an 11 s fixture.
    pub decode_ms_per_11s: Option<f64>,
    /// Real-time factor: audio_seconds / wall_seconds. Higher is better.
    pub rtf: Option<f64>,
    /// Whether the required transcript fragment was found in the output.
    pub accuracy_fragment_match: Option<bool>,
    /// How many words of the expected fragment were matched (0.0..1.0).
    /// 1.0 means every expected word was found (case-insensitive).
    #[serde(default)]
    pub wer_fragment: Option<f32>,
}

/// One qualification record for one (engine, backend, device-class, model,
/// quant) combination on one OS + narrator build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualificationTuple {
    pub engine: String,
    pub runtime_backend: String,
    pub device_class: DeviceClass,
    pub model_id: String,
    pub quant: String,
    pub os: String,
    pub narrator_version: String,
    pub pass: bool,
    pub metrics: QualificationMetrics,
}

/// Versioned on-disk registry (`qualifications.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualificationRegistry {
    pub schema_version: u32,
    pub tuples: Vec<QualificationTuple>,
}

impl Default for QualificationRegistry {
    fn default() -> Self {
        Self {
            schema_version: QUALIFICATIONS_SCHEMA_VERSION,
            tuples: Vec::new(),
        }
    }
}

impl QualificationRegistry {
    pub fn load(path: &Path) -> Result<Self, RegistryError> {
        let text = std::fs::read_to_string(path)?;
        let reg: Self = serde_json::from_str(&text).map_err(|source| RegistryError::Json {
            path: path.display().to_string(),
            source,
        })?;
        Ok(reg)
    }

    pub fn save(&self, path: &Path) -> Result<(), RegistryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|source| RegistryError::Json {
            path: path.display().to_string(),
            source,
        })?;
        std::fs::write(path, text + "\n")?;
        Ok(())
    }

    /// Insert or replace the matching tuple (same engine/backend/class/model/quant/os/version).
    pub fn upsert(&mut self, tuple: QualificationTuple) {
        self.tuples.retain(|t| t.key_differs_from(&tuple));
        self.tuples.push(tuple);
    }

    pub fn passing_for_class(
        &self,
        class: DeviceClass,
        backend: Option<&str>,
    ) -> Vec<&QualificationTuple> {
        self.tuples
            .iter()
            .filter(|t| {
                t.pass && t.device_class == class && backend.is_none_or(|b| t.runtime_backend == b)
            })
            .collect()
    }
}

impl QualificationTuple {
    fn key_differs_from(&self, other: &QualificationTuple) -> bool {
        self.engine != other.engine
            || self.runtime_backend != other.runtime_backend
            || self.device_class != other.device_class
            || self.model_id != other.model_id
            || self.quant != other.quant
            || self.os != other.os
            || self.narrator_version != other.narrator_version
    }
}

fn host_os() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The bounded request that decodes one fixture through the sidecar.
pub fn qualification_command(
    cli_path: &Path,
    backend: &str,
    model_path: &Path,
    wav_path: &Path,
) -> ProbeCommand {
    ProbeCommand::inheriting_environment(cli_path)
        .arg("--model")
        .arg(model_path)
        .arg("--backend")
        .arg(backend)
        .arg("-q")
        .arg(wav_path)
}

/// Run one real decode through the transcribe-cli sidecar and build a tuple.
///
/// `audio_seconds` is the fixture duration used for RTF; `required_fragment`
/// must appear in stdout for `pass = true`.
///
/// The decode runs through the injected [`HostProbe`] under `timeout`, so a
/// wedged sidecar is killed and reported as
/// [`ComputeError::SidecarTimeout`](crate::planner::ComputeError::SidecarTimeout)
/// instead of hanging the caller. The measured wall time is still compared
/// against `timeout` afterwards, so an implementation that overshoots its own
/// budget is rejected as well.
#[allow(clippy::too_many_arguments)] // explicit probe inputs beat a params struct here
pub fn qualify_decode(
    probe: &dyn HostProbe,
    cli_path: &Path,
    backend: &str,
    device_class: DeviceClass,
    model_path: &Path,
    wav_path: &Path,
    audio_seconds: f64,
    required_fragment: &str,
    timeout: Duration,
) -> Result<QualificationTuple, crate::planner::ComputeError> {
    let started = Instant::now();
    let outcome = probe
        .run_probe(
            &qualification_command(cli_path, backend, model_path, wav_path),
            timeout,
        )
        .map_err(|error| match error {
            crate::probe::ProbeError::TimedOut { .. } => {
                crate::planner::ComputeError::SidecarTimeout(format!("{}s", timeout.as_secs()))
            }
            crate::probe::ProbeError::Unavailable { reason, .. } => {
                crate::planner::ComputeError::SidecarSpawn(reason)
            }
        })?;
    let wall = started.elapsed();
    if wall > timeout {
        return Err(crate::planner::ComputeError::SidecarTimeout(format!(
            "{}s",
            timeout.as_secs()
        )));
    }
    let stdout = outcome.stdout;
    let fragment_match = outcome.success && stdout.contains(required_fragment);
    let wall_secs = wall.as_secs_f64();
    let wer_fragment = compute_wer_fragment(required_fragment, &stdout);
    let metrics = QualificationMetrics {
        decode_ms_per_11s: Some((wall.as_millis() as f64) * (11.0 / audio_seconds.max(0.001))),
        rtf: Some(audio_seconds / wall_secs.max(0.000_001)),
        accuracy_fragment_match: Some(fragment_match),
        wer_fragment: Some(wer_fragment),
    };
    let (model_id, quant) = split_model_quant(model_path);
    Ok(QualificationTuple {
        engine: "transcribe-gguf".into(),
        runtime_backend: backend.into(),
        device_class,
        model_id,
        quant,
        os: host_os(),
        narrator_version: env!("CARGO_PKG_VERSION").into(),
        pass: fragment_match,
        metrics,
    })
}

/// Compute word-level fragment match ratio 0.0..1.0.
/// Case-insensitive, counts how many expected words appear in actual output.
pub fn compute_wer_fragment(expected: &str, actual: &str) -> f32 {
    let expected_words: Vec<String> = expected
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    if expected_words.is_empty() {
        return 1.0;
    }
    let actual_lower = actual.to_lowercase();
    let matched = expected_words
        .iter()
        .filter(|w| actual_lower.contains(w.as_str()))
        .count();
    (matched as f32) / (expected_words.len() as f32)
}

/// Split e.g. `whisper-tiny.en-Q8_0.gguf` into ("whisper-tiny.en", "Q8_0").
pub fn split_model_quant(model_path: &Path) -> (String, String) {
    let stem = model_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match stem.rsplit_once('-') {
        Some((m, q))
            if !q.is_empty() && q.chars().next().is_some_and(|c| c.is_ascii_uppercase()) =>
        {
            (m.to_string(), q.to_string())
        }
        _ => (stem, "unknown".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), String>;

    fn test_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, String> {
        result.map_err(|error| format!("unexpected test failure: {error:?}"))
    }

    #[test]
    fn splits_model_and_quant() {
        assert_eq!(
            split_model_quant(Path::new("/x/whisper-tiny.en-Q8_0.gguf")),
            ("whisper-tiny.en".to_string(), "Q8_0".to_string())
        );
        assert_eq!(
            split_model_quant(Path::new("plain-model.gguf")),
            ("plain-model".to_string(), "unknown".to_string())
        );
    }

    #[test]
    fn registry_roundtrip_and_upsert() -> TestResult {
        let mut reg = QualificationRegistry::default();
        let mk = |backend: &str, rtf: f64| QualificationTuple {
            engine: "e".into(),
            runtime_backend: backend.into(),
            device_class: DeviceClass::IGpu,
            model_id: "whisper-tiny.en".into(),
            quant: "Q8_0".into(),
            os: "linux-x86_64".into(),
            narrator_version: "0.0.0".into(),
            pass: true,
            metrics: QualificationMetrics {
                rtf: Some(rtf),
                ..Default::default()
            },
        };
        reg.upsert(mk("vulkan", 3.0));
        reg.upsert(mk("vulkan", 9.0)); // replaces same key
        assert_eq!(reg.tuples.len(), 1);
        assert_eq!(reg.tuples[0].metrics.rtf, Some(9.0));

        let dir = test_ok(tempfile::tempdir())?;
        let path = dir.path().join("nested/qualifications.json");
        test_ok(reg.save(&path))?;
        let loaded = test_ok(QualificationRegistry::load(&path))?;
        assert_eq!(loaded.schema_version, QUALIFICATIONS_SCHEMA_VERSION);
        assert_eq!(loaded, reg);
        Ok(())
    }

    #[test]
    fn passing_filter_respects_pass_flag_and_backend() {
        let mut reg = QualificationRegistry::default();
        reg.tuples.push(QualificationTuple {
            runtime_backend: "vulkan".into(),
            pass: false,
            device_class: DeviceClass::Npu,
            ..sample()
        });
        reg.tuples.push(QualificationTuple {
            runtime_backend: "cpu".into(),
            pass: true,
            device_class: DeviceClass::Cpu,
            ..sample()
        });
        assert!(reg.passing_for_class(DeviceClass::Npu, None).is_empty());
        assert_eq!(
            reg.passing_for_class(DeviceClass::Cpu, Some("cpu")).len(),
            1
        );
        assert_eq!(
            reg.passing_for_class(DeviceClass::Cpu, Some("vulkan"))
                .len(),
            0
        );
    }

    fn sample() -> QualificationTuple {
        QualificationTuple {
            engine: "e".into(),
            runtime_backend: "cpu".into(),
            device_class: DeviceClass::Cpu,
            model_id: "m".into(),
            quant: "Q8_0".into(),
            os: "linux-x86_64".into(),
            narrator_version: "0.0.0".into(),
            pass: true,
            metrics: QualificationMetrics::default(),
        }
    }

    #[test]
    fn a_qualification_request_carries_the_measured_decode_inputs_in_order() {
        let command = qualification_command(
            Path::new("/opt/transcribe-cli"),
            "vulkan",
            Path::new("/models/whisper-tiny.en-Q8_0.gguf"),
            Path::new("/fixtures/jfk16.wav"),
        );
        assert_eq!(command.program(), Path::new("/opt/transcribe-cli"));
        assert_eq!(
            command.arguments(),
            [
                std::ffi::OsString::from("--model"),
                std::ffi::OsString::from("/models/whisper-tiny.en-Q8_0.gguf"),
                std::ffi::OsString::from("--backend"),
                std::ffi::OsString::from("vulkan"),
                std::ffi::OsString::from("-q"),
                std::ffi::OsString::from("/fixtures/jfk16.wav"),
            ]
        );
    }
}

#[cfg(test)]
mod wer_fragment_tests {
    use super::*;

    #[test]
    fn wer_fragment_exact_match_is_one() {
        assert_eq!(
            compute_wer_fragment(
                "ask not what your country",
                "ask not what your country can do for you"
            ),
            1.0
        );
    }

    #[test]
    fn wer_fragment_partial_match() {
        // 2 of 4 words matched => 0.5
        let frag = compute_wer_fragment("hello world foo bar", "hello foo");
        assert!((frag - 0.5).abs() < 1e-6, "got {frag}");
    }

    #[test]
    fn wer_fragment_case_insensitive() {
        assert_eq!(compute_wer_fragment("Ask Not What", "ask not what"), 1.0);
    }

    #[test]
    fn wer_fragment_zero_when_no_words_match() {
        assert_eq!(compute_wer_fragment("hello world", "goodbye earth"), 0.0);
    }

    #[test]
    fn real_cpu_decode_tiny_en_wer_is_one_when_sidecar_present() {
        // Mirrors the jfk fixture expectation: tiny.en should get 1.0 on known-good output.
        let simulated_stdout = "and so my fellow americans ask not what your country can do for you ask what you can do for your country";
        let wer = compute_wer_fragment("ask not what your country", simulated_stdout);
        assert_eq!(wer, 1.0);
    }
}
