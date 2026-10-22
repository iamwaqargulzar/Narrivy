You are the implementation worker. Codex has already isolated one bounded defect and will independently review and verify your work.

Task: fix the IPC-to-domain recognition-tag conversion in `services/narrivyd/src/daemon/session.rs`. `RecognitionTagDto` intentionally accepts `-` and `_`, while domain `AsrTag::parse` accepts the canonical hyphenated form. The current `recognition_request_from_dto` passes the wire string directly, so a valid wire value such as `EN_us` is rejected. Normalize underscores to hyphens at this conversion boundary before calling `AsrTag::parse`. Preserve lowercasing in the domain parser and preserve existing error redaction.

Scope and constraints:
- Edit only `services/narrivyd/src/daemon/session.rs`.
- Make the smallest correct change; do not redesign the DTO or domain type.
- Extend the existing `recognition_wire_settings_map_to_distinct_domain_fields` regression to prove an underscore-form wire tag becomes the canonical lowercase hyphenated domain tag. Avoid a duplicate test if the existing test can cover it clearly.
- Do not commit, push, modify evidence/state, install anything, or change unrelated formatting.
- Do not weaken or remove assertions.

Focused verification:
1. `cargo fmt --all -- --check`
2. `cargo test -p narrivyd recognition_wire_settings_map_to_distinct_domain_fields -- --exact`
3. `cargo clippy -p narrivyd --all-targets -- -D warnings`

A command counts as passing only when its tool result exits zero. Stop immediately after the three checks pass and report the actual edit and exact command results. If blocked, report the blocker truthfully rather than expanding scope.
