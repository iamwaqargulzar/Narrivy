# Plugin and Extension Architecture

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Narrivy is modular internally from the start, but **does not stabilize a public third-party plugin SDK before core contracts mature**. See `module-system.md` for the Linux 1.0 internal-module rule.

## Terminology

- **module:** an implementation behind an internal Narrivy capability contract; may be built-in, a trusted runtime pack, or later isolated worker.
- **plugin/extension:** an externally distributable component loaded under a supported public extension contract.
- **pack:** data/model/runtime bundle with a manifest and integrity metadata; a pack does not automatically gain executable privileges.

## Future plugin categories

Portable/sandboxable:
- text processors;
- audio processors where latency permits;
- Mode definitions;
- dictionaries/domain packs;
- context transforms;
- safe actions.

Trusted/native or sidecar:
- ASR engines;
- hardware/compute runtimes;
- platform integration;
- specialized low-latency audio backends/processors;
- delivery backends.

## Execution strategy

Preferred order for externally supplied code:

1. data-only manifest packs;
2. WASM/WASI for portable untrusted logic where feasible;
3. versioned sidecar process protocol for native/high-performance code;
4. narrow C ABI only if a later ADR proves it necessary.

A public Rust dylib/trait-object ABI is prohibited because compiler, allocator, panic and dependency ABI are not stable enough for a durable ecosystem boundary.

## Security

Every extension manifest declares permissions. The host grants a subset and enforcement occurs at service boundaries. Audio, transcript, context, filesystem, network, secrets and delivery are separate capabilities. Network is deny-by-default. Native code is not made safe merely by a manifest; trusted native extension distribution requires stronger signing/provenance/review and may require process isolation.

Linux 1.0 exercises this contract through the internal module registry:
`ModulePermissions` keeps declared and host-granted permissions separate,
`ModuleRegistry` rejects undeclared grants, and `narrivyd` authorizes each
operation before invoking module code. An empty grant set is the default, so a
module that declares `network.connect` still cannot connect until the host
explicitly grants it.

The public plugin SDK/marketplace is post-1.0 unless explicitly promoted. Linux 1.0 must still exercise the same internal permission and module boundaries so later extensions do not require an architectural rewrite.
