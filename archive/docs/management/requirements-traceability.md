# Requirements Traceability

`spec/requirements-traceability.csv` is the authoritative implementation/evidence map. `scripts/verify_traceability.py` requires every requirement in the product catalog to occur exactly once.

## Columns

- `requirement_id` — immutable catalog key;
- `severity` — `must`/`should`/`roadmap` engineering priority semantics;
- `scope` — `linux-1.0` or `post-1.0`;
- `primary_phase` — phase responsible for initial closure;
- `secondary_phase` — later qualification/hardening phase when relevant;
- `component` — expected implementation area;
- `acceptance_summary` — human-readable normative behavior copied from the catalog;
- `verification_id` — stable identifier for test/report/manual verification;
- `verification_layer` — expected evidence class;
- `release_gate` — `required` or `roadmap`;
- `status` — `planned`, `implementing`, `passed`, `blocked`, or `roadmap`;
- `evidence` — repository-relative path to real evidence.

## Closure rule

Use `scripts/mark_requirement.py` to mark a requirement passed only after its evidence exists. A phase gate checks all release-required rows whose primary phase is that phase. P11/release gate checks all required rows again.

Roadmap rows remain mapped and visible but do not block Linux 1.0. Moving a row from required to roadmap or vice versa is a product scope change and must be explicitly justified, not used to make a failing gate green.
