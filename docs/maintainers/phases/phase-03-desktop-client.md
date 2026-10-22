# P3 — Desktop Client


## Outcome
Build the GTK4/WebKitGTK 6 + React/TypeScript desktop client **on top of the already-proven service path (P2)**. The UI is a client, not the owner of product policy. Hide or clearly label experimental/post-1.0 capabilities; do not expose screens for deferred features as if complete.

> Runtime note (2026-09-04, ADR-0064): P3 was originally specified against Tauri 2. The maintained Linux shell is now `narrivy-desktop-gtk`; the Tauri shell is excluded from the workspace (`Cargo.toml` `exclude`) and is retained only as a migration parity oracle.

## Primary requirements
`UX-001`–`UX-008`, `UX-010` (final accessibility requalification in P9).

## Required work
- Information architecture per the legacy planning amendment §25/§30: **Home, Dictation, Models, Applications, History, Settings, Diagnostics** + About/Updates. Keep top-level navigation approachable (legacy planning §25). Settings sections: General, Automation, Compute, ASR, Text Delivery, Audio, Formatting, Privacy, Advanced.
- Home screen shows computed current configuration (Compute Auto→device, Model, Mode, Language, Microphone, Text Delivery Auto→backend), last transcript actions (Copy/Paste Again/Edit), and status summary from real state — never hard-coded (legacy planning §26).
- Design tokens, typography/spacing/component primitives, focus states, high-DPI/responsive minimum sizes, loading/empty/error/degraded states, dialogs/sheets/toasts and notification hierarchy.
- Overlay is non-focus-stealing and suppresses transient visual churn; floating dictation overlay respects Light/Dark/System theme (legacy planning §40).
- Capability-driven disabled/unsupported/remediation states; no frontend `if linux/gnome/cosmic` product logic (capability-driven UI).
- Onboarding that reaches a real first dictation without runtime jargon (UX-001).
- Keyboard navigation and accessible semantic structure from first components; reduced-motion and contrast tokens exist now even though P9 certifies them.
- Diagnostics can launch isolated capability checks through typed APIs (UX-010).
- i18n/message-ID architecture and layout resilience.
- Theme support mandatory: System / Light / Dark, synchronized quick switch and Settings control, persisted, with accessibility contrast checks (legacy planning §37–§39).
- Compute/ASR/Text Delivery advanced controls are visible and overridable but progressive-disclosure — every automatic decision must show resolved value (e.g., `Auto → Intel NPU`) and allow Reset to Auto (legacy planning §11–§13, §31–§34).

## Tests
Component/navigation/state tests, keyboard/focus smoke, token/schema checks and screenshot/visual-regression where deterministic.

## Exit evidence
Desktop shell launches against **real daemon** (not only mock) and can complete onboarding/direct walkthrough; capability-driven UI correctly hides/labels deferred features. Schema-valid evidence for UX verification IDs. `python3 scripts/phase_gate.py --phase P3` exits 0.
