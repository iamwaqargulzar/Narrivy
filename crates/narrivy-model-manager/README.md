# narrivy-model-manager

Model registry, verified acquisition and install, residency tracking and cache cleanup for local ASR models.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-model-manager --no-deps --open`
- **Tests:** `cargo test -p narrivy-model-manager --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
