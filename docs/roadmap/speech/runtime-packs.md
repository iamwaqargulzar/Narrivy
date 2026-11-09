# Runtime Packs and Lean Compute Provisioning

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

## Principle

The normal Narrivy installer stays lean. Vendor accelerator stacks and specialized optimized artifacts are acquired only for hardware the user actually has and only after policy allows the download.

## Base package

The base package contains:

- Narrivy desktop/client/service binaries;
- hardware/capability discovery and qualification tooling;
- the reference CPU correctness runtime/path;
- a lean, broadly compatible Vulkan-capable general GPU path for qualified dGPU/iGPU use where redistribution/system-runtime constraints permit, with CPU fallback always available;
- runtime-pack/model-manager logic and trusted metadata.

Large model weights do not have to live in the installer. First-run model acquisition is staged, verified and resumable. A separate offline bundle can provide fully offline provisioning without bloating the default download.

## Runtime pack contract

A runtime pack is a versioned, independently managed accelerator/runtime bundle. Manifest metadata records:

- immutable pack ID/version and supported Narrivy API range;
- platform/architecture and hardware/device eligibility;
- runtime/vendor identity and version;
- required system driver/kernel/runtime constraints;
- files, sizes and SHA-256; signature/provenance status where available;
- license/redistribution metadata;
- provided capabilities and supported model formats/precisions;
- install footprint and dependency relationships;
- self-test/qualification entry point.

Install is stage → verify → self-test → atomically publish. A failed pack never becomes active.

## Candidate hierarchy

Auto discovers candidates in this preference order:

**NPU → discrete GPU → integrated GPU → CPU**

Vulkan is an execution API inside the GPU pools, not its own hardware tier. OpenVINO or another runtime may serve more than one device class.

The hierarchy does not override qualification. Auto may select a lower tier if it is more stable, accurate, efficient, or faster for the actual model/runtime tuple.

## Manual choice

Advanced users may select exact device/runtime/model/quantization. The manager validates compatibility and health before switching. Manual preferred retains fallback; manual locked surfaces failure rather than silently changing the requested path.

## Dynamic acquisition

On first run or hardware change:

1. discover CPU/GPU/NPU and driver/runtime facts;
2. map hardware to potentially compatible packs;
3. avoid downloading anything for clearly irrelevant vendors;
4. present the recommendation and size/network impact;
5. honor network/privacy policy;
6. download and verify only selected/recommended packs;
7. run bounded qualification using non-private fixtures;
8. persist the qualified tuple and resolved selection.

A driver/runtime/application update invalidates only the affected qualification/cache keys.

## Cleanup

Benchmark and conversion artifacts carry ownership/reference metadata. After selection, cleanup may remove unreferenced artifacts generated only for losing candidates. It must retain:

- immutable original model/source artifacts;
- selected optimized artifacts;
- CPU fallback requirements;
- shared or pinned artifacts;
- summary measurements/evidence.

Cleanup is reference-aware and restart-reconcilable; never `rm -rf` a broad model/runtime directory based solely on benchmark rank.

## Resource registry integration

Runtime packs are resolved through the same trusted resource registry as models/modules. Upstream source is preferred when suitable, Narrivy mirror is a verified fallback, and immutable version/hash/provenance are the identity anchors rather than URL. See `docs/roadmap/design/resource-registry.md`.
