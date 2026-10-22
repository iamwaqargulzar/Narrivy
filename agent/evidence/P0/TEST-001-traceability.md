# TEST-001 Evidence — Every Release-gated Requirement Maps to Verification and Evidence

## Verification: VER-TEST-001 (ci/architecture)
**Requirement:** Every release-gated requirement maps to a named automated or manual verification and evidence location.

## Implementation
- Single source of truth: `docs/01-product/product-requirements.md` catalog (126 IDs)
- Traceability: `spec/requirements-traceability.csv` with columns: requirement_id, severity, scope, primary_phase, secondary_phase, component, acceptance_summary, verification_id, verification_layer, release_gate, status, evidence
- Validator: `scripts/verify_traceability.py` checks:
  - Every catalog ID appears exactly once in CSV
  - No duplicate IDs
  - Required columns present
  - Valid status/release_gate/phase values
  - Passed rows have non-empty evidence pointing to existing file (enforced at phase/release gates)
  - 103 Linux 1.0 gated, 23 roadmap
- Phase validator: `scripts/verify_phase_model.py` checks acceptance vs traceability alignment
- CI: pack job runs verify_traceability, verify_phase_model, check_action_pins, verify-boundaries, verify_manifest

## Local Verification
```
python3 scripts/verify_traceability.py → Traceability OK: 126 catalog requirements mapped exactly once; 103 Linux 1.0 gated, 23 roadmap.
python3 scripts/validate_pack.py → Pack static validation OK.
```

## Evidence Files
- `spec/requirements-traceability.csv`
- `scripts/verify_traceability.py`
- `docs/01-product/product-requirements.md`
