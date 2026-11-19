# Error Code Registry

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Use stable machine-readable error codes for diagnostics/support while UI text remains user-friendly and localizable.

Families:
- `NAR-PLAT-*` platform/capability
- `NAR-AUDIO-*` capture/audio policy
- `NAR-ASR-*` recognition/model/runtime
- `NAR-OUT-*` delivery
- `NAR-POL-*` policy/privacy
- `NAR-IPC-*` protocol/transport
- `NAR-DATA-*` storage/migration
- `NAR-EXT-*` plugin/integration

Each code registry entry records severity, retryability, user impact, safe diagnostic fields and recommended UI treatment.
