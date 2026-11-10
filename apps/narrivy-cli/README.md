# narrivy-cli

Command-line client for narrivyd (toggle, status, support report).

- **Role:** Client: renders state and sends commands to `narrivyd`; it never duplicates policy or domain decisions.
- **Crate docs:** `cargo doc -p narrivy-cli --no-deps --open`
- **Tests:** `cargo test -p narrivy-cli --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
