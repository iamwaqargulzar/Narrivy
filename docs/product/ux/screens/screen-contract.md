# Screen Contract Template

Every Linux 1.0 screen must document and test:

1. **Primary user job** — why this page exists.
2. **Hierarchy** — page title, primary action, secondary actions, main information order.
3. **Data/capabilities** — typed backend state required; no OS/runtime inference in UI.
4. **States** — loading, empty, ready, degraded/permission-needed, error, offline/remote where relevant.
5. **Keyboard/focus** — initial focus, tab order, shortcuts, dialog return focus.
6. **Accessibility** — roles/names/live regions and scaling/contrast concerns.
7. **Privacy** — content/retention/remote boundary shown where relevant.
8. **Responsive behavior** — compact/standard/wide layout.
9. **Recovery** — actionable user-facing error and safe fallback.
10. **Tests** — component/state/a11y/E2E identifiers.

A screen is not complete if only its happy-state screenshot exists.
