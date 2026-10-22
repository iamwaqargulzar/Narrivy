# narrivy-history

Retained user History store, dictionary aliases and privacy-zone evaluation.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-history --no-deps --open`
- **Tests:** `cargo test -p narrivy-history --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
