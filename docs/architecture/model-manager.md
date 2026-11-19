# Model Manager

## Responsibilities

- catalog immutable model identities/revisions;
- acquire/import/verify/install/remove models atomically;
- expose languages/tasks/engine/device compatibility;
- enforce model/source/license metadata completeness;
- coordinate Hot/Warm/Cold residency;
- track disk/RAM/VRAM estimates and actual measurements;
- key compiled-artifact caches by every compatibility input;
- recover interrupted downloads/conversions;
- coordinate optional runtime packs without coupling model identity to one vendor runtime;
- track cache ownership/reference counts so benchmark cleanup is safe;
- provide offline bundle support without weakening verification.

## Manifest authority

`spec/schemas/model-manifest.schema.json` is the machine-readable contract. A distributable model entry must include provenance, immutable revision, file hashes/sizes, format/precision, compatible engine/runtime constraints, supported tasks/languages, device claims, memory estimates where known, and license/redistribution metadata.

A device claim such as `NPU` means “eligible for qualification”, not “known working everywhere”. Runtime health records separately determine usable device/model pairs.

Recovery 4Q-C1 does not promote the live version-1 model registry to this
authority. That registry is a narrower compatibility catalog. Path presence is
reported only as `PresentUnverified`; `verify_installed` can produce a bounded,
registry-observed `InstalledModelBinding`, but Linux composition must still
copy and attest those bytes through the immutable descriptor boundary in
ADR-0050. See ADR-0051.

Recovery 4Q-C2 composes this observation only through the Linux no-follow,
descriptor-relative trust-anchor binder. Installed startup selects a bounded
registry model ID, never a raw model pathname/digest tuple; the root runtime
manifest remains a separate worker authority. See ADR-0052. Package model-pack
membership, signatures, SBOM linkage, and external release provenance remain
unavailable.

## Safe install transaction

1. resolve catalog entry/immutable revision;
2. verify license/redistribution policy before automatic distribution;
3. ensure disk budget;
4. download/import to a staging location with path-safe filenames;
5. verify every expected file hash/size and optional signature;
6. perform any conversion/optimization into a versioned staging directory;
7. validate engine load/inference health using a non-sensitive fixture;
8. atomically publish the completed model directory/manifest;
9. never expose a partial directory as installed.

Recovery 4Q-C1 closes existence-only, path-component, declared-size, digest,
identity-metadata, unbounded verification, and stable descendant-symlink drift,
but does not claim this complete transaction or defeat a concurrent same-UID
pathname race.
Completed-directory publication, install locks/references, and composition
with the normative distribution manifest remain release blockers.

## Cache key

Compiled/optimized cache keys include at minimum: model immutable revision/hash, conversion recipe version, model format/precision, runtime name/version, engine adapter version, device identity/capabilities, driver/runtime compatibility identifiers that affect compiled output, and relevant compile options.

Unknown metadata invalidates rather than reuses a potentially incompatible cache.

## Removal

A model cannot be deleted while an active session owns it. Mark pending removal, release after references drain, then delete cache/model atomically enough that restart can reconcile state.

## Runtime packs

Vendor/specialized runtimes are managed independently from model weights. A model may be compatible with multiple runtime packs, and a runtime pack may serve multiple models. Detection does not install everything: acquire only relevant packs after compatibility/policy/user checks. See `docs/roadmap/speech/runtime-packs.md`.

## Benchmark cleanup

Conversions/compiled blobs created only to evaluate losing candidates are eligible for cleanup after references drain. Never remove the immutable source model, selected optimized artifacts, CPU fallback requirements, shared/reference-counted artifacts, or user-pinned files.
