# narrivy-journal-unix

Secure Unix filesystem adapter for the recovery journal.

- **Role:** Adapter crate: OS-specific mechanics live here, behind the portable contracts. It owns no product policy.
- **Crate docs:** `cargo doc -p narrivy-journal-unix --no-deps --open`
- **Tests:** `cargo test -p narrivy-journal-unix --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
