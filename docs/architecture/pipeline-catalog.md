# Pipeline Catalog

This table is the catalog design (`spec/catalogs/`), not what 1.0 runs.
At rc13 the daemon runs two built-in modes: `clean` (the default) and
`raw`. The other recipes are roadmap.

| Pipeline | Status | Trigger | Privacy | Stages |
|---|---|---|---|---|
| Fast Dictation | linux-1.0-default | toggle or hold | local-only | audio.capture → audio.normalize → audio.denoise → speech.vad → speech.endpoint → speech.asr → text.punctuation → text.itn → text.semantic-guard → delivery.direct/delivery.clipboard/delivery.inbox |
| Clean Mic | optional | user binding | local-only | audio.capture → audio.normalize → speech.vad → speech.endpoint → speech.asr → text.punctuation → text.itn → text.semantic-guard → delivery.direct/delivery.clipboard/delivery.inbox |
| Smart Local | optional | user binding | local-only | audio.capture → audio.denoise → speech.vad → speech.asr → text.punctuation → text.itn → text.llm-postprocess → text.semantic-guard → delivery.direct/delivery.clipboard/delivery.inbox |
| Cloud Polish | optional | user binding | cloud-text-explicit | audio.capture → audio.denoise → speech.vad → speech.asr → text.punctuation → text.itn → text.llm-postprocess → text.semantic-guard → delivery.direct/delivery.clipboard/delivery.inbox |
| Speaker-safe | optional | user binding | local-default | audio.capture → audio.aec → audio.denoise → speech.vad → speech.asr → text.itn → delivery.direct/delivery.clipboard/delivery.inbox |
| Noisy Office | optional | user binding | local-default | audio.capture → audio.denoise → speech.vad → speech.asr → text.itn → text.semantic-guard → delivery.direct/delivery.clipboard/delivery.inbox |
| Far-field | roadmap | user binding | local-default | audio.capture → audio.beamform → audio.dereverb → audio.denoise → speech.vad → speech.asr → delivery.direct/delivery.clipboard/delivery.inbox |
| Focus on Me | roadmap | user binding | local-default | audio.capture → audio.target-speaker → audio.denoise → speech.vad → speech.asr → delivery.direct/delivery.clipboard/delivery.inbox |
| Coding | roadmap | user binding | profile-defined | audio.capture → audio.denoise → speech.vad → speech.asr → text.itn → text.llm-postprocess? → text.semantic-guard → delivery.direct/delivery.clipboard/delivery.inbox |
| Translate | roadmap | user binding | profile-defined | audio.capture → audio.denoise → speech.vad → speech.asr → text.translate → text.semantic-guard → delivery.direct/delivery.clipboard/delivery.inbox |
| Meeting | roadmap | toggle/session | profile-defined | audio.capture → audio.denoise → speech.vad → speech.asr → speech.diarization → text.llm-postprocess? |
| Hands-free Accessibility | roadmap | wake-word/command | local-default | activation.wake-word → audio.capture → speech.vad → speech.asr → text.semantic-guard → delivery.direct/delivery.clipboard/delivery.inbox |
