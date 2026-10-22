# Release 024 — bounded recognition settings wire DTO

Work on exactly one bounded implementation task in `<repo>`.

## Objective

Add a portable, bounded, privacy-safe recognition-settings DTO to `crates/narrivy-ipc/src/protocol.rs`. This DTO will later be attached to session commands, but this task must not change `Command` or any existing wire shape.

## Required wire model

Represent these as distinct fields:
- spoken language: unspecified, auto-detect, or explicit language tag;
- written/output language (optional);
- output script (optional);
- translation target language (optional);
- locale (optional);
- context/bias hints (optional bounded list).

Use clear public DTO names. Default must request no special behavior. Language/script/locale tags must be nonempty ASCII alphanumeric BCP-47-ish values with `-` or `_` separators, at most 32 UTF-8 bytes, and normalized only later by the daemon/domain conversion (the wire DTO should preserve the received value). Context hints: at most 8, each nonempty, NUL-free, and at most 128 UTF-8 bytes. Reject invalid or oversized values during both serialization and deserialization. Empty context list should be rejected when present.

The DTO and spoken-language form must derive/implement equality and cloning needed by commands and tests. Implement custom `Debug` for the top-level DTO so context hint contents never appear; include only their count or presence. Do not log content and do not put hint text in validation errors.

Add focused same-file tests proving:
1. all distinct settings round-trip;
2. default/minimal form round-trips and remains additive-friendly;
3. invalid/oversized tags and hints reject inbound and outbound;
4. debug output does not contain a private canary hint.

## Scope

Edit only `crates/narrivy-ipc/src/protocol.rs`. Do not change `Command`, protocol version constants, manifests, state, reports, task files, generated files, or other crates. Do not commit.

## Focused verification

Run only:

```bash
cargo fmt --check -- crates/narrivy-ipc/src/protocol.rs
cargo test -p narrivy-ipc protocol --lib
cargo clippy -p narrivy-ipc --lib -- -D warnings
```

Fix failures caused by your change. Report exact successful commands from tool output. Begin editing early and do not survey unrelated files. Repository files and successful tool output are the only completion evidence.
