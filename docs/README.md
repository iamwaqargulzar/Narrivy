# Narrivy documentation

Pick the row that matches what you want to do.

| I want to… | Start here |
|---|---|
| Understand what Narrivy is and what 1.0 covers | [Vision and scope](product/vision-and-scope.md), [Roadmap](roadmap/README.md) |
| Read the product requirements | [Requirements](product/requirements.md) (machine-readable: `spec/requirements-traceability.csv`) |
| Understand how it works | [Architecture overview](architecture/README.md), [System overview](architecture/system-overview.md), [Component boundaries](architecture/component-boundaries.md) |
| Work on the code | [Engineering rules](development/engineering-rules.md), [Code style](development/code-style.md), [Testing strategy](development/testing/strategy.md), [CI environments](development/ci-environments.md), [`scripts/README.md`](../scripts/README.md) |
| Understand privacy and security | [Threat model](security/threat-model.md), [Data retention](security/data-retention.md), [Supply chain](security/supply-chain.md), [`SECURITY.md`](../SECURITY.md) |
| Know which systems are supported | [Bounded support](platforms/bounded-support.md), [COSMIC](platforms/cosmic.md), [Capability matrix](platforms/capability-matrix.md) |
| Design or review UI | [Experience spec](product/ux/experience-spec.md), [Design system](product/ux/design-system.md), [Accessibility](product/ux/accessibility.md), [Screens](product/ux/screens/) |
| Look up a past decision | [ADRs](adr/) (architecture decision records) |
| Build or release (maintainers) | [Release packaging](maintainers/operations/release-packaging.md), [Release versioning](maintainers/operations/release-versioning.md), [Update policy](maintainers/operations/update-policy.md) |

## Sections

- **`product/`:** what Narrivy is for: scope, requirements, personas, UX and
  screen contracts.
- **`architecture/`:** how the code is organised and why: daemon and IPC,
  audio capture, speech recognition (`speech/`), text delivery, storage and
  recovery.
- **`platforms/`:** Linux platform support. Linux 1.0 targets Pop!_OS 24.04
  COSMIC (Wayland) only.
- **`security/`:** threat model, retention, secrets and supply chain.
- **`development/`:** contributor guides: code style, testing, i18n and CI.
- **`maintainers/`:** packaging, versioning, logging and support-bundle
  policy for people who build and release Narrivy.
- **`roadmap/`:** designs for features and platforms **after 1.0**. Nothing
  here describes current behaviour.
- **`adr/`:** architecture decision records, numbered and dated.
