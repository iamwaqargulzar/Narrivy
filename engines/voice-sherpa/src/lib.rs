//! Voice mode's worker (D34): listening and speaking for a spoken
//! conversation with the assistant, in its own process
//! (`narrivy-voice-worker`).
//!
//! - It listens on the microphone and finds turns with Silero voice
//!   detection; the audio of a turn goes to the daemon, which recognises it
//!   with the speech model already loaded for dictation (so voice mode needs
//!   no second speech model).
//! - It speaks the reply sentence by sentence with Kokoro as it arrives;
//!   Urdu sentences go to the separate Urdu program ([`urdu`]).
//!
//! This library holds the parts with no native code: the text the voice
//! reads ([`text`]), the cues ([`cue`]), resampling ([`resample`]) and the
//! audio framing both sides share ([`wire`]). The daemon uses [`wire`]; the
//! binary adds sherpa-onnx and the audio devices.

#![forbid(unsafe_code)]

#[cfg(feature = "worker")]
pub mod audio;
pub mod cue;
pub mod phonemes;
pub mod resample;
#[cfg(feature = "worker")]
pub mod speech;
pub mod text;
pub mod urdu;
pub mod wire;

/// Rate of the microphone audio the worker hands over, in Hz.
pub const RATE: u32 = 16_000;
/// The variables the daemon may pass to the worker (`--env KEY=VALUE`),
/// for PipeWire's same-user session routing only.
pub const ROUTING_KEYS: &[&str] = &["XDG_RUNTIME_DIR", "PIPEWIRE_RUNTIME_DIR", "PIPEWIRE_REMOTE"];
/// Most Kokoro speakers (`--speaker` 0 to this, inclusive).
pub const MAX_SPEAKER: u32 = 52;
