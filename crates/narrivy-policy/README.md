# narrivy-policy

Deterministic privacy and processing policy decisions.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-policy --no-deps --open`
- **Tests:** `cargo test -p narrivy-policy --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
