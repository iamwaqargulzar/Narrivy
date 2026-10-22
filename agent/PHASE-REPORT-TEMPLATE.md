# Phase reports

Phase reports (`agent/phase-reports/P*.json` plus the narrative `.md`) are
**generated**, never hand-written:

```bash
python3 scripts/generate_phase_report.py --phase <P>
python3 scripts/generate_phase_report.py --phase <P> --write-headline
```

The JSON must validate against `spec/schemas/phase-report.schema.json`, and
the generator refuses to overstate acceptance. The narrative `.md` may add
context (environment, commands run, deviations and ADRs, known non-blockers),
but must never contradict the JSON. See completion plan WP4 T4.6 for the
known phase-report/gate cycle, and WP6 T6.5 for the final P11 report.
