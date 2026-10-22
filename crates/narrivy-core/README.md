# narrivy-core

Portable application core: session machine, recovery tail, worker supervision and correction orchestration.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-core --no-deps --open`
- **Tests:** `cargo test -p narrivy-core --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
