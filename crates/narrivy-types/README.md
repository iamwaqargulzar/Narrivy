# narrivy-types

Portable domain value types shared by every Narrivy crate.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-types --no-deps --open`
- **Tests:** `cargo test -p narrivy-types --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
