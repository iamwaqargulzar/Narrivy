# narrivy-asr-api

Speech-recognition engine contracts: streaming sessions, typed requests, capabilities and cancellation.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-asr-api --no-deps --open`
- **Tests:** `cargo test -p narrivy-asr-api --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
