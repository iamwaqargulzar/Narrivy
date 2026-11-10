# Dictation Controls + Overlay Contract

## Job
Configure and understand daily dictation behavior without exposing unnecessary runtime details.

There is no Dictation page: it was removed in rc12. Start, Stop and Cancel
are on Home (`HomeScreen`). The Raw/Clean mode and the dictation language
are on Pipelines (Transcribe stage) and in the tray. The start and cancel
shortcuts and the overlay position are in Settings. The microphone is
chosen with `narrivy-cli mic`.

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

The IDs `UI-DICT-BIND`, `UI-DICT-PTT-CAPABILITY`, `UI-OVERLAY-NO-FOCUS`,
`UI-OVERLAY-STALE-EVENT` and `E2E-DICT-CANCEL` are not real tests (WP4 T4.7).
The real frontend tests for these controls are in
`apps/narrivy-desktop/src/__tests__/`: `DictationCancellationRace`,
`DictationLatchShared`, `DictationStartsAdvertisedPipelineId`,
`DictationStateFromEvents` and `PerfDictationStartToListening`.
