# narrivy-ipc

Portable, versioned and bounded local-IPC wire protocol: envelopes, commands, events and framing.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-ipc --no-deps --open`
- **Tests:** `cargo test -p narrivy-ipc --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
