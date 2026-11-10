# ADR-0019 — Portable settings import/export and future sync boundary

**Status:** Accepted  
**Date:** 2026-08-22

## Decision

Narrivy provides versioned **settings import/export** without requiring an account. Export can cover the whole portable configuration or selected Pipelines/Profiles/dictionaries.

Default exports contain no secret values, OAuth tokens, API keys, local session credentials, raw history/audio, device qualification caches, compiled accelerator caches, or host-specific temporary paths. Secret references may be represented only as unresolved provider slots.

Import validates schema/version, resource references, permissions, and migrations before changing active configuration. Required optional modules/models are presented as a dependency plan and downloaded only after approval/policy checks.

## Future sync

An optional account-backed sync service may be added later, but Narrivy must remain fully functional without login. Sync should move portable intent/configuration, not large binaries or machine-specific qualification results. Each machine resolves the user's Pipeline against its own hardware and downloads/qualifies appropriate resources locally.
