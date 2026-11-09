# Extension / Plugin Permission Contract

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

A public third-party plugin runtime is post-1.0 unless explicitly promoted, but internal module and extension permission vocabulary is defined now to prevent unsafe architecture. Trusted built-in/runtime modules still must not bypass policy merely because they are not third-party plugins.

Canonical permission names are enumerated by `spec/schemas/plugin-manifest.schema.json`; arbitrary strings are invalid. Current namespaces include scoped transcript/context/clipboard/history/network/notification/text-delivery/model/dictionary permissions.

Rules:
- manifest requests capabilities; policy grants a subset;
- network is deny-by-default and any allowlist is explicit;
- permissions are checked at the service boundary, not trusted because a plugin says it needs them;
- optional-denied permissions must degrade safely;
- secrets are referenced through named secure handles, never injected wholesale;
- plugin archive/artifact integrity and safe extraction are mandatory before loading;
- future runtime sandbox design must have its own ADR/threat review before executable third-party plugins ship.

Executable extension forms follow ADR-0014: data-only/WASM/sidecar are preferred; public arbitrary Rust dylib loading is prohibited. `audio.read`/`audio.process` and other sensitive permissions are independent grants.
