# narrivyd

The Narrivy per-user dictation daemon: composes capture, ASR, correction, delivery and recovery.

- **Role:** Service: composes the portable core with the Linux adapters and engines, and owns the daemon lifecycle.
- **Crate docs:** `cargo doc -p narrivyd --no-deps --open`
- **Tests:** `cargo test -p narrivyd --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
