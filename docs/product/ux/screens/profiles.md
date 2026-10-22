# Profiles Screen Contract

## Job
Apply predictable dictation defaults to applications/contexts without surprising the user.

List view shows profile name, match summary, selected mode/mic/delivery overrides, priority and enabled status. Detail view exposes typed match conditions, resulting settings and a `Why this profile?` preview/test using current safe application metadata.

Conflict/precedence is deterministic and explained. Privacy Zones always override profile requests for remote processing or retention. Unknown target metadata must not trigger a broad wildcard profile accidentally.

Tests: `PROFILE-MATCH`, `PROFILE-PRECEDENCE`, `PROFILE-PRIVACY-OVERRIDE`, `UI-PROFILE-EXPLAIN`.
