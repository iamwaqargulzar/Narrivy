# narrivy-asr-mock

Deterministic mock ASR engine for tests and development (never used for release behaviour).

- **Role:** Engine crate: ASR runtime specifics live here, behind `narrivy-asr-api`.
- **Crate docs:** `cargo doc -p narrivy-asr-mock --no-deps --open`
- **Tests:** `cargo test -p narrivy-asr-mock --locked`

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
