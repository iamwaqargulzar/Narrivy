# Narrivy 1.0 Experience Specification

## Experience promise

Narrivy should feel like a dependable system capability, not a model laboratory. The default path is: launch → ready → trigger → speak → text appears. Technical depth exists for users who want it, but never obstructs first dictation.

## Window architecture

Desktop app uses a stable application shell:

- primary navigation rail/sidebar;
- page header with title, optional scoped actions, and status only when relevant;
- scrollable page body with a readable maximum content width for settings and wider layouts only for tables/diagnostics;
- transient overlay/pill separate from the main window and never focus-stealing in normal dictation;
- command/search palette accessible by keyboard.

### Responsive behavior

Define three layout ranges in tokens, not device-name checks:

- compact: narrow single-column, navigation collapses to icon/drawer pattern while preserving labels via accessible names;
- standard: sidebar + content;
- wide: content may use two-column cards/inspector where it reduces scanning.

The desktop window must remain usable at the minimum supported size defined in tokens and at high UI scale. No primary action can be clipped behind fixed pixel layouts.

## Information architecture

Linux 1.0 navigation:

1. Home
2. Dictation
3. Models
4. Applications
5. History
6. Settings
7. Diagnostics

Modes/profiles/dictionary are surfaced within the relevant Dictation/Applications/Settings workflows. Privacy is a first-class Settings section and processing-boundary state remains visible where relevant. Deferred Snippets/Integrations are not displayed as working top-level placeholders. Avoid a giant undifferentiated Settings page; use clear Settings sections and deep links.

## State language

Use the same semantic state vocabulary across UI:

- Ready
- Listening
- Processing
- Delivering (shown only if perceptible/useful)
- Degraded
- Permission needed
- Offline/local
- Remote (only when intentionally active)
- Error

Critical state is conveyed by text/icon/semantics in addition to color/animation.

## Progressive disclosure

Default surface shows user intent: microphone name, mode, trigger, readiness, local/remote boundary, model quality/resource choice. Runtime names, device IDs, compiler caches, driver errors, thread counts and engine internals live in advanced panels/Diagnostics unless needed for recovery.

## Feedback timing

- acknowledge trigger immediately;
- listening feedback appears within the performance target and does not wait for ASR;
- do not flash a “Processing” state for operations so short that it creates visual noise;
- persistent operations (model download/compile) show determinate progress when measurable and cancellable state;
- background completion uses a nonintrusive notification/toast when the user needs to know.

## Error hierarchy

1. **Inline validation** for a field/action the user can fix immediately.
2. **Page/banner degraded state** when a capability is unavailable but the page remains usable.
3. **Toast** for transient non-destructive completion/failure with undo/retry if relevant.
4. **Dialog** only for consequential choices, permission explanation before system UI, or destructive irreversible operations.
5. **Diagnostics detail** contains technical codes/copyable metadata, separate from the user-facing message.

Every user-facing error answers: what happened, whether text/data is safe, what Narrivy did automatically, and the next useful action.

## Loading/empty/disabled policy

- use skeletons only when layout is known and loading is perceptible;
- use progress for downloads/compilation;
- empty states explain why the collection matters and provide one primary action;
- disabled controls expose the capability/permission reason via adjacent text/tooltip accessible to keyboard/screen reader;
- do not render future roadmap controls as enabled placeholders.

## Motion

Motion communicates continuity/status, not decoration. Respect reduced motion. No blinking/flashing listening state. Overlay transitions must not delay feedback or steal focus.

## Keyboard

Every action has a reachable focus target. Page-level shortcuts do not override text-editing conventions. Command palette is keyboard-accessible. Escape has consistent behavior: close transient surface/cancel safe UI operation, but does not accidentally discard active dictation unless explicitly bound/documented.

## Copy/tone

Short, concrete, nontechnical by default. Use model/runtime names only in advanced context. Prefer “Microphone access is blocked — open system permissions” to an opaque backend error. Never imply text was inserted if Narrivy cannot verify that outcome; say it was saved to Inbox.

## Acceptance

Every screen contract defines: primary job, hierarchy, states, keyboard/focus, capability/privacy behavior, error recovery, responsive behavior, and test IDs. P2/P7–P9 UI gates cannot pass from screenshots alone; automated component/a11y tests plus manual interaction checks are required.
