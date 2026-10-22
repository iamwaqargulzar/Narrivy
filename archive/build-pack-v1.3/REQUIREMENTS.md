# Requirements Index

The authoritative catalog is `docs/01-product/product-requirements.md` and the authoritative implementation map is `spec/requirements-traceability.csv`.

Requirement families:

- `FUN-*` functional behavior
- `UX-*` product/UI behavior
- `PERF-*` measurable performance
- `REL-*` reliability/recovery
- `PORT-*` portability/architecture
- `PRIV-*` privacy
- `SEC-*` security/supply chain
- `ACC-*` accessibility
- `TEST-*` testability/verification
- `OPS-*` installation, operation, release, and supportability

## Rule

Every requirement ID in the catalog must occur exactly once in the traceability CSV. `python3 scripts/verify_traceability.py` enforces this.
