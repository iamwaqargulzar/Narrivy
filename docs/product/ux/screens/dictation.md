# Dictation Screen + Overlay Contract

## Job
Configure and understand daily dictation behavior without exposing unnecessary runtime details.

Page shows trigger mode/binding, default Raw/Clean mode, microphone, audio policy, overlay preference, delivery behavior summary, and links to advanced/profile-specific configuration.

## Overlay

The transient overlay is a client view of daemon session events. It never becomes the session owner and never steals focus during normal dictation.

Use minimal states:
- Listening — immediate acknowledgement, optional restrained level indicator;
- Processing — only when finalization is perceptible;
- Saved to Inbox / Degraded — concise recovery cue;
- Error — actionable, persistent long enough to understand.

Do not continuously show raw transcript partials by default if doing so creates distraction/privacy risk; if a live preview option exists, make its content/privacy behavior explicit.

## Keybinding recorder

Clearly distinguishes PTT-capable vs toggle-only backend. Detect conflicts where platform APIs allow. Escape cancels recording; the previous binding remains until a valid new binding is committed.

## Tests

`UI-DICT-BIND`, `UI-DICT-PTT-CAPABILITY`, `UI-OVERLAY-NO-FOCUS`, `UI-OVERLAY-STALE-EVENT`, `E2E-DICT-CANCEL`.
