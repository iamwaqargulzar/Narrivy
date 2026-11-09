# Model Residency and Cache Ownership

## States
- **Hot:** fully ready/resident.
- **Warm:** optimized/compiled cache ready; not necessarily resident.
- **Cold:** immutable original/source model only.

Residency considers recent use, active Mode/Profile, RAM/VRAM/NPU memory, battery, selected compute policy and user pins.

## Cache identity

Every generated optimized artifact records enough metadata to invalidate safely on model revision/hash, conversion recipe, runtime/ABI, engine adapter, device, driver, compiler properties, preprocessing contract and relevant options. Unknown compatibility metadata invalidates rather than reuses.

## Ownership classes

Generated files must be classified as one or more of:

- source/immutable;
- selected optimized;
- CPU fallback required;
- shared/reference-counted;
- user-pinned;
- benchmark-temporary;
- stale/invalidated;
- failed/corrupt staging.

## Benchmark cleanup

After Auto qualification/selection, Narrivy may remove **unreferenced** benchmark-temporary or losing converted/compiled artifacts. It must retain source models, selected optimized artifacts, CPU fallback assets, shared/reference-counted files and user-pinned files.

Cleanup is transactional/restart-reconcilable and uses ownership/reference metadata. Never delete a broad directory solely because a candidate ranked last. Offer diagnostics for bytes reclaimed and a developer option to retain benchmark artifacts.
