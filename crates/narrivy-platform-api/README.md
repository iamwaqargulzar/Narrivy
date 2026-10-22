# narrivy-platform-api

Platform capability contracts (delivery, shortcuts, context, lifecycle) with no concrete OS code.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-platform-api --no-deps --open`
- **Tests:** `cargo test -p narrivy-platform-api --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
