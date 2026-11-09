# Settings, Pipeline Import/Export, and Portability

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

## User-facing exports

Narrivy supports:

- **Export all portable settings**;
- **Export selected Pipeline(s)**;
- **Export profiles/dictionaries** where applicable.

The default export is safe to share and excludes secrets and machine-specific caches.

## Portable intent versus host resolution

Export the user's intent (for example `Auto`, preferred capability/module, model family, privacy policy), not stale resolved hardware state such as “NPU device 0 with driver X”. On import, the receiving machine re-runs compatibility/qualification and resolves the Pipeline against its own hardware.

## Secrets

API keys, OAuth refresh/access tokens, cookies/session credentials and local IPC credentials stay in the platform secret store and are omitted by default. An encrypted full-backup feature may be designed later as a separate explicit security feature; it must not be an accidental side effect of settings export.

## Dependency import

A Pipeline file contains stable resource requirements. Import shows missing modules/models/runtimes and estimated downloads, then installs only after policy/user authorization. Unknown/untrusted repositories never gain permission merely because an imported Pipeline references them.

## Account independence

No login is required for local use or import/export. Optional encrypted account sync is post-1.0 and synchronizes configuration/recipes rather than heavyweight binaries or host qualification caches.
