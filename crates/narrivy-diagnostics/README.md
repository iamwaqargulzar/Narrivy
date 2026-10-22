# narrivy-diagnostics

Content-safe diagnostics primitives: redaction, health facts and latency summaries.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-diagnostics --no-deps --open`
- **Tests:** `cargo test -p narrivy-diagnostics --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
