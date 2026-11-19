# narrivy-platform-null

Null platform adapter for portable integration tests.

- **Role:** Adapter crate: OS-specific mechanics live here, behind the portable contracts. It owns no product policy.
- **Crate docs:** `cargo doc -p narrivy-platform-null --no-deps --open`
- **Tests:** `cargo test -p narrivy-platform-null --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
