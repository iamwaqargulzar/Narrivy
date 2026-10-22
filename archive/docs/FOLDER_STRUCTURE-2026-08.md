# Repository Structure

```text
/
├── AGENTS.md                    engineering invariants
├── agent/                       resumable state, decisions, blockers, research
├── apps/                        desktop/CLI clients
├── services/                    narrivyd composition/lifecycle
├── crates/                      portable domain/contracts
├── adapters/                    OS/platform implementations
├── engines/                     ASR/runtime implementations
├── docs/
│   ├── 01-product/
│   ├── 02-architecture/
│   ├── 03-ui-ux/
│   ├── 04-platforms/
│   ├── 05-ml-inference/
│   ├── 06-security-privacy/
│   ├── 07-testing/
│   ├── 08-operations/
│   ├── 09-phases/
│   ├── 10-agent/
│   ├── 11-adrs/
│   ├── 12-references/
│   ├── 13-management/
│   ├── 14-i18n/
│   ├── 15-autonomous-build/
│   └── 16-release/
├── spec/
│   ├── acceptance/              executable phase gate descriptors
│   ├── schemas/                 versioned data/package schemas
│   ├── examples/
│   ├── protocols/
│   ├── ui/
│   └── requirements-traceability.csv
├── tests/                       contract/integration/e2e/failure/perf/a11y/schema
├── packaging/linux/             packaging source
├── tools/                       conversion/reference tooling
├── scripts/                     doctor/validation/gates/bootstrap
└── artifacts/                   generated evidence/releases (created during build)
```

Keep portable product code out of `apps/`, platform code out of portable crates, and generated release evidence out of normative source/spec directories.
