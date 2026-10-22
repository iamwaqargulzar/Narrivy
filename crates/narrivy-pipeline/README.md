# narrivy-pipeline

Declarative, versioned pipeline definitions and resource identity (ADR-0016, ADR-0017).

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-pipeline --no-deps --open`
- **Tests:** `cargo test -p narrivy-pipeline --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
