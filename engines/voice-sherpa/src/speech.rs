//! Kokoro (speaking) and Silero (finding speech) through sherpa-onnx.

use std::path::Path;

use crate::phonemes::Lexicon;
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, SileroVadModelConfig, VadModelConfig,
    VoiceActivityDetector,
};

/// Files a Kokoro voice pack holds (its manifest lists them all).
pub const KOKORO_FILES: &[&str] = &[
    "model.onnx",
    "voices.bin",
    "tokens.txt",
    "lexicon-us-en.txt",
];

fn path(dir: &Path, name: &str) -> Option<String> {
    dir.join(name).to_str().map(str::to_owned)
}

/// The English voice.
pub struct Voice {
    tts: OfflineTts,
    words: Lexicon,
}

impl Voice {
    /// Load Kokoro from `dir` (a verified voice pack).
    pub fn kokoro(dir: &Path, threads: i32) -> Result<Self, &'static str> {
        if KOKORO_FILES.iter().any(|name| !dir.join(name).is_file()) {
            return Err("voice_model_missing");
        }
        let mut config = OfflineTtsConfig::default();
        let kokoro = &mut config.model.kokoro;
        kokoro.model = path(dir, "model.onnx");
        kokoro.voices = path(dir, "voices.bin");
        kokoro.tokens = path(dir, "tokens.txt");
        kokoro.lexicon = path(dir, "lexicon-us-en.txt");
        if dir.join("dict").is_dir() {
            kokoro.dict_dir = path(dir, "dict");
        }
        config.model.num_threads = threads;
        config.max_num_sentences = 1;
        let tts = OfflineTts::create(&config).ok_or("voice_model_invalid")?;
        let words = std::fs::read_to_string(dir.join("lexicon-us-en.txt"))
            .map(|text| Lexicon::parse(&text))
            .map_err(|_| "voice_model_invalid")?;
        Ok(Self { tts, words })
    }

    /// Speakers the voice has.
    #[must_use]
    pub fn speakers(&self) -> i32 {
        self.tts.num_speakers()
    }

    /// `(samples, rate)` for `text` in voice `speaker` at `speed`. The
    /// text is turned into phonemes here ([`Lexicon::sentence`]), so no
    /// word is skipped.
    #[must_use]
    pub fn speak(&self, text: &str, speaker: i32, speed: f32) -> Option<(Vec<f32>, u32)> {
        let phonemes = self.words.sentence(&crate::text::numbers_as_words(text));
        if phonemes.is_empty() {
            return None;
        }
        let config = GenerationConfig {
            sid: speaker.clamp(0, (self.speakers() - 1).max(0)),
            speed,
            phoneme_codepoints: Some(vec![phonemes.chars().map(|c| c as i32).collect()]),
            ..GenerationConfig::default()
        };
        let audio = self
            .tts
            .generate_with_config::<fn(&[f32], f32) -> bool>(text, &config, None)?;
        let rate = u32::try_from(audio.sample_rate()).ok()?;
        Some((audio.samples().to_vec(), rate))
    }
}

/// Silero window at 16 kHz.
pub const WINDOW: usize = 512;

/// A Silero detector: `min_silence` seconds of quiet end a segment.
pub fn detector(
    model: &Path,
    min_silence: f32,
    min_speech: f32,
) -> Result<VoiceActivityDetector, &'static str> {
    detector_at(model, 0.5, min_silence, min_speech)
}

fn detector_at(
    model: &Path,
    threshold: f32,
    min_silence: f32,
    min_speech: f32,
) -> Result<VoiceActivityDetector, &'static str> {
    let config = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: model.to_str().map(str::to_owned),
            threshold,
            min_silence_duration: min_silence,
            min_speech_duration: min_speech,
            window_size: WINDOW as i32,
            max_speech_duration: 30.0,
        },
        sample_rate: crate::RATE as i32,
        num_threads: 1,
        ..VadModelConfig::default()
    };
    VoiceActivityDetector::create(&config, 60.0).ok_or("voice_detector_invalid")
}

/// Shortest stretch [`speech_ms`] counts as speech, in seconds: a click, a
/// cough or a door is shorter than a spoken word.
pub const MEASURE_MIN_SPEECH: f32 = 0.25;

/// How sure Silero must be that a window is speech for [`speech_ms`]. Voice
/// mode's 0.5 also takes a key press or a chair at -30 dBFS for a word
/// (seen in room recordings on Windows); 0.7 leaves those out and still
/// finds one short word at -34 dBFS.
pub const MEASURE_THRESHOLD: f32 = 0.7;

/// Milliseconds of speech Silero finds in `samples` (16 kHz mono), for the
/// dictation's no-speech check: room noise, hum and clicks count as none.
pub fn speech_ms(model: &Path, samples: &[f32]) -> Result<u64, &'static str> {
    let vad = detector_at(model, MEASURE_THRESHOLD, 0.3, MEASURE_MIN_SPEECH)?;
    let mut speech_samples = 0_u64;
    let mut drain = |vad: &VoiceActivityDetector| {
        while let Some(segment) = vad.front() {
            speech_samples += u64::try_from(segment.samples().len()).unwrap_or(0);
            vad.pop();
        }
    };
    for window in samples.chunks_exact(WINDOW) {
        vad.accept_waveform(window);
        drain(&vad);
    }
    vad.flush();
    drain(&vad);
    Ok(speech_samples * 1000 / u64::from(crate::RATE))
}
