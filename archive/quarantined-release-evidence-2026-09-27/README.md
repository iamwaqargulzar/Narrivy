# Quarantined release metadata (2026-09-27)

These two directories were git-tracked and are consumed by
`scripts/release_gate.py`. They are moved here, not deleted, so the prior state
remains auditable.

## `release-evidence/linux-1.0/`

`qualification.json` and `release-artifacts.json` asserted a complete pass:

```
cosmic-wayland  native  e2e_pass: true
gnome-wayland   native  e2e_pass: true
cpu_fallback_pass: true   accessibility_pass: true   privacy_security_pass: true
```

Three reasons the claim is not supportable:

1. Both records are bound to commit `14ffd6990fdf…`, which **does not exist in
   this repository** (`git cat-file -t` reports "not a valid object name"). The
   current candidate package is a different artifact.
2. The sibling `summary.md` in the same directory states the GNOME lane was
   **deferred**, contradicting `qualification.json`.
3. `agent/STATE.json` records `release_gate: "blocked"`, and the traceability CSV
   carries 55 blocked release-gated requirements.

## `sbom/`

`provenance.json` is an in-toto statement declaring:

- `builder.id = ghcr.io/narrivy/builder:stub` with a 64-zero digest,
- `metadata.reproducible: true`,
- `materials[0]` at commit `43566ebea4f8…`, which **does not exist**,
- `builder.cargo_lock_hash = 0729c304…` while the real `Cargo.lock` hashes to
  `d32fbf1f…`.

`spdx.json` lists 458 packages with `licenseConcluded`/`licenseDeclared`/
`verificationCode` all `NOASSERTION` and no PURL references — a dependency name
list, not a license inventory.

## Replacement

Generate both from the reviewed candidate path, which labels itself honestly as
`unqualified-candidate-provenance`:

```
python3 scripts/build_candidate_supply_chain.py
```

The existing candidate bundle at `artifacts/candidates/linux-1.0-supply-chain/`
is the honest example: it records `image: "local-host-unqualified"` and declines
to claim a hermetic builder or reproducibility.
