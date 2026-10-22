# Linux 1.0 Qualification Plan

## Required evidence structure

Create during P11:

```text
artifacts/release-evidence/linux-1.0/
├── summary.md
├── qualification.json
├── release-artifacts.json
├── requirements/            # reports referenced by traceability rows
├── environments/
│   └── cosmic-wayland/        # the only 1.0 lane (owner decision 2026-09-28)
├── accessibility/
├── performance/
├── security/
└── packaging/
```

## `qualification.json` contract

Use `spec/schemas/linux-qualification.schema.json` as the authoritative
contract (currently schema version 3), not historical schema-v1 receipts.
Required root fields are `schema_version`, `build_identity`, `git_commit`,
`git_dirty`, `environments`, `cpu_fallback_pass`, `accessibility_pass`,
`privacy_security_pass`, `target_matrix`, and `performance`.

Each environment records exact OS/kernel/desktop/session, native-versus-VM,
CPU/microphone, package type and SHA-256, source commit, E2E result and evidence
path. The `cosmic-wayland` lane requires native platform-sensitive
qualification. GNOME is post-release and has no 1.0 requirement.
The target matrix and performance record must satisfy the schema's complete
nested requirements; a short E2E boolean summary cannot replace them.
Supporting evidence also records portal/PipeWire/runtime/hardware identity and
the actual executed scenarios. Populate pass fields only from inspected results
for the exact candidate artifact.

## `release-artifacts.json`

Use `spec/schemas/release-artifacts.schema.json` (currently schema version 3).
Required root fields are `schema_version`, `version`, `build_identity`,
`git_commit`, `git_dirty`, `builder_identity`, `artifacts`, `sbom_path`,
`provenance_path`, and `signing`. Record the exact artifact hashes and required
nested metadata. The release gate recomputes checksums and checks the evidence
bindings; an installable unqualified candidate is not sealed release provenance.

## No screenshot-only verification

Screenshots may supplement UX evidence but cannot prove text-delivery reliability, recovery, privacy redaction, package upgrades or performance. Use machine-readable/logged test reports with content-safe fixtures and manual environment checklists.
