# narrivy-ipc-unix

Authenticated Unix-domain socket transport for the Narrivy IPC protocol.

- **Role:** Adapter crate: OS-specific mechanics live here, behind the portable contracts. It owns no product policy.
- **Crate docs:** `cargo doc -p narrivy-ipc-unix --no-deps --open`
- **Tests:** `cargo test -p narrivy-ipc-unix --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
