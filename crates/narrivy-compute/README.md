# narrivy-compute

Compute-device discovery, per-tuple qualification records and explainable device planning.

- **Role:** Portable crate: no OS APIs, no concrete ASR runtime, no UI framework (see `AGENTS.md`, "Architecture boundaries").
- **Crate docs:** `cargo doc -p narrivy-compute --no-deps --open`
- **Tests:** `cargo test -p narrivy-compute --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
