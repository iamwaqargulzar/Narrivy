# Configuration and Migrations

## Stores

Keep distinct concepts:
- user configuration/preferences;
- optional history/content;
- active/recovery journal;
- model/catalog/cache metadata;
- health/benchmark metadata;
- secrets (references only in config; values in secure store).

Do not put everything in one opaque database solely for convenience.

## Versioning

Every persistent structured store has an explicit schema version. A binary declares versions it can read/write and migrations it can apply. Migrations are deterministic, tested on fixtures from prior supported versions, and idempotent or safely restartable where practical.

## Safe migration transaction

1. validate source store/version;
2. acquire appropriate single-writer lock/ownership;
3. preserve a recoverable previous representation or use transactional storage;
4. migrate to staging/transaction;
5. validate invariants;
6. atomically commit new version;
7. on failure retain/read previous state and report actionable error.

Never delete the source before the migrated result is validated.

## Unknown/newer versions

Do not overwrite a configuration/database created by a newer incompatible Narrivy. Open read-only where safe or fail with clear recovery guidance.

## Secret migration

Never serialize plaintext secret values into migration logs/backups/support bundles. Migrate secure-store references through platform secret-store APIs.

## Portable import/export

Narrivy exports versioned portable settings and selected Pipelines without secrets or host-specific qualification/cache state. Import validates schema/migrations, resource dependencies, trust and permissions before activation. See `settings-portability.md` and ADR-0019. Optional account-backed sync is post-1.0 and must not become a requirement for local use.
