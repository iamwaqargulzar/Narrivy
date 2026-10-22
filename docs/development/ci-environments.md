# CI vs Qualification Environments

## Hosted CI

Use hosted Linux/Windows/macOS runners for portable compilation, static validation, unit/contract tests and supply-chain checks. These jobs protect architecture/portability but do not claim working desktop integration.

## Linux integration CI/lab

As P2 matures, add a maintained Linux integration runner or scripted local qualification host for PipeWire/desktop/portal tests. Mark tests that require a real user session explicitly and fail the release gate when required evidence is missing instead of treating CI `skip` as success.

## Release lab

P11 runs the declared reference environments from `docs/maintainers/qualification/reference-environments.md`, preferably from clean/snapshotted systems using the actual release package.
