# Narrivy documentation

Pick the row that matches what you want to do.

| I want to… | Start here |
|---|---|
| Understand what Narrivy is and what 1.0 covers | [Vision and scope](product/vision-and-scope.md), [Roadmap](roadmap/README.md) |
| Read the product requirements | [Requirements](product/requirements.md) (machine-readable: `spec/requirements-traceability.csv`) |
| Understand how it works | [Architecture overview](architecture/README.md), [System overview](architecture/system-overview.md), [Component boundaries](architecture/component-boundaries.md) |
| Work on the code | [Code style](development/code-style.md), [Testing strategy](development/testing/strategy.md), [CI environments](development/ci-environments.md), [`scripts/README.md`](../scripts/README.md) |
| Understand privacy and security | [Threat model](security/threat-model.md), [Data retention](security/data-retention.md), [Supply chain](security/supply-chain.md), [`SECURITY.md`](../SECURITY.md) |
| Know which desktops are supported | [Bounded support](platforms/bounded-support.md), [COSMIC](platforms/cosmic.md), [Capability matrix](platforms/capability-matrix.md) |
| Design or review UI | [Experience spec](product/ux/experience-spec.md), [Design system](product/ux/design-system.md), [Accessibility](product/ux/accessibility.md), [Screens](product/ux/screens/) |
| Look up a past decision | [ADRs](adr/) (architecture decision records) |
| Build, qualify or release (maintainers) | [Release gates](maintainers/qualification/release-gates.md), [Release packaging](maintainers/operations/release-packaging.md), [Linux 1.0 qualification](maintainers/release/linux-1.0-qualification.md), [Phases](maintainers/phases/) |

## Sections

- **`product/`:** what Narrivy is for: scope, requirements, personas, UX and
  screen contracts.
- **`architecture/`:** how the code is organised and why: daemon and IPC,
  audio capture, speech recognition (`speech/`), text delivery, storage and
  recovery.
- **`platforms/`:** Linux platform support. 1.0 supports Pop!_OS 24.04
  COSMIC (Wayland) only.
- **`security/`:** threat model, retention, secrets and supply chain.
- **`development/`:** contributor guides: code style, testing, i18n and CI.
- **`maintainers/`:** release engineering, qualification evidence, phase
  specs and the AI-agent process. Contributors don't need this section.
- **`roadmap/`:** designs for features and platforms **after 1.0**. Nothing
  here describes current behaviour.
- **`adr/`:** architecture decision records, numbered and dated.

Historical material (old plans, audits, management registers) lives in
[`archive/`](../archive/README.md) and is not current guidance.

> Documentation is being revised for 1.0. Where a document and the code
> disagree, the code and `agent/completion-plan/00-review-findings.md` are
> more current. Please open an issue.
