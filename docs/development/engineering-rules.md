# Engineering rules

These rules apply to every change in this repository. Code comments and crate
READMEs refer to them by section name.

## Architecture boundaries

- `crates/narrivy-core` owns domain orchestration but no OS, UI framework, ASR
  runtime, network provider, or device-specific implementation.
- Portable contracts live in `crates/narrivy-*-api` and the portable domain
  crates.
- OS-specific product behaviour lives in `adapters/platform-*`. Narrow,
  reusable OS mechanics may live only in the infrastructure adapter families
  listed in [component boundaries](../architecture/component-boundaries.md)
  (currently `ipc-*`, `journal-*` and `process-*`); those adapters own no
  product policy or portable contract.
- ASR and runtime-specific implementation lives in `engines/asr-*`.
- `services/narrivyd` composes dependencies and owns the long-running service
  lifecycle.
- `apps/narrivy-desktop` and the CLI are clients; they do not duplicate policy
  or domain decisions.
- Privacy policy runs before data leaves its allowed boundary.
- The active-session recovery journal is separate from optional user history.
- The frontend renders capabilities; it does not use OS names, engine names or
  model names as feature flags.

Before changing pipelines, engines, modules, runtimes, models or providers,
read the [architecture overview](../architecture/README.md) and
`spec/catalogs/`.

## API stability

Once an API is marked stable in
[API stability](../architecture/api-stability.md), a breaking change needs an
ADR and a migration.

## Rust rules

- Rust edition 2024. The project toolchain is pinned in `rust-toolchain.toml`;
  the portable crates' MSRV is tested separately in CI.
- Portable and core crates use `unsafe_code = forbid`.
- Unavoidable FFI `unsafe` stays inside the specific adapter or engine crate,
  with its invariants documented.
- Production paths use typed errors; no panic-driven control flow.
- No blocking operations on UI or real-time audio threads.
- Long operations expose cancellation and bounded timeouts.
- Background tasks have an explicit owner, lifetime, shutdown and failure
  policy.
- Warnings are errors in CI.

## Security and privacy rules

- Ordinary logs never include transcript text, audio, clipboard, selection or
  context content, tokens, secrets, or environment dumps.
- IPC is same-user, authenticated and authorized only.
- Untrusted archives, model packs and plugin packs reject path traversal and
  verify integrity metadata.
- Secrets use an OS credential facility or a documented secure abstraction,
  never ordinary configuration.
- Extension and plugin network access is deny-by-default.
- Security and dependency checks are never disabled to make CI pass.

## Quality rules

A feature is complete only with its tests, failure UX, diagnostics,
accessibility behaviour and documentation. Mock-only success does not satisfy a
requirement that claims real platform or hardware behaviour.

Pick the smallest check tier in [`scripts/README.md`](../../scripts/README.md)
that covers the change; there is no need to run every tier after every edit.
