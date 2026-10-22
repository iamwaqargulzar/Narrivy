# narrivy-journal-api

Recovery-journal records, retention policy and the portable storage port.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-journal-api --no-deps --open`
- **Tests:** `cargo test -p narrivy-journal-api --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
