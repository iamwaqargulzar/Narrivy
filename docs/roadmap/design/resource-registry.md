# Resource Registry and Distribution

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

## Goal

Keep Narrivy lean while allowing a large ecosystem of modules, runtimes, models, and pipeline recipes. Narrivy owns trustworthy metadata and resolution; bytes may come from authoritative upstream sources or verified Narrivy mirrors.

## Resource identities

A stable resource key is independent of its current download URL, for example:

- `module:org.narrivy.deepfilternet@3.x`
- `runtime:org.narrivy.openvino.linux-x86_64@2026.x`
- `model:org.author.whisper-small@<immutable-revision>`
- `pipeline:org.narrivy.smart-local@1`

Each registry record includes artifact class, immutable version/revision, compatibility, dependencies, license/redistribution status, expected hashes, optional signature/provenance, sources in priority order, install size, and trust channel.

## Hosting strategy

### Models

1. authoritative upstream repository first;
2. Narrivy Hugging Face organization as a mirror only when redistribution permits;
3. Narrivy-owned converted/quantized variants with explicit source revision/provenance.

Gated resources remain gated and are fetched from the authoritative provider after user authorization.

### Modules and runtime packs

1. authoritative upstream release/package when suitable;
2. Narrivy verified artifact/OCI-style mirror/package;
3. future organization/community repositories only under explicit trust policy.

## Verification

A registry resolution is not permission to execute. Installer verifies expected SHA-256 and any configured signature/provenance, archive safety, license policy, platform/architecture, API compatibility and dependencies. Executable resources self-test in staging before atomic activation.

## Deduplication and cleanup

Resources are content-addressed/reference-counted where practical. Five Pipelines using one model share one installed copy. Removing a Pipeline does not remove shared resources. When the last reference disappears, Narrivy can offer to free unused space.

Storage classes:

- permanent selected/source resources;
- shared cache used by active configurations;
- disposable qualification/conversion cache.

Disposable losing benchmark artifacts are cleaned automatically after qualification unless pinned/developer-retained.

## Offline

A separate signed offline bundle can contain registry snapshots and selected resources. The normal installer is not inflated solely to make every optional module available offline.
