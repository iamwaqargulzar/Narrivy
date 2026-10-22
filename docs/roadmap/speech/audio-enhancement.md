# Audio Enhancement and Conditioning

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Audio enhancement is independent from ASR and is built as a composable module chain/graph.

Capability slots may include:
- canonicalization/resampling/channel handling;
- high-pass/DC/hum cleanup and limiter;
- acoustic echo cancellation using optional playback reference;
- beamforming/spatial processing for known multi-mic geometry;
- dereverberation;
- noise suppression;
- source separation / target-speaker extraction;
- AGC/normalization;
- VAD and separate endpointing;
- optional acoustic scene/audio tagging used by Auto policy.

Enable stages only when measured ASR benefit exceeds latency/quality/power cost. A clean microphone path should not be overprocessed. Auto changes selected enhancement chains between utterances rather than unexpectedly changing acoustics halfway through a session.

Linux 1.0 ships a **lean conservative built-in path**; heavier processors are optional modules/packs. Candidate implementations live in `module-candidate-catalog.md` and must be re-qualified before integration.
