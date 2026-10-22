# narrivy-process-linux

Trusted system-helper launching on Linux: identity checks, environment scrubbing and process-group containment.

- **Role:** Adapter crate: OS-specific mechanics live here, behind the portable contracts. It owns no product policy.
- **Crate docs:** `cargo doc -p narrivy-process-linux --no-deps --open`
- **Tests:** `cargo test -p narrivy-process-linux --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
