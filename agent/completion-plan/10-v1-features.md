# WP2c: v1 feature scope (D29, D30, D31, D32, D33)

The owner set the full v1 scope on 2026-10-01: the dictation experience
first, then a workspace around it. The design is
`docs/roadmap/design/dictation-experience-and-workspace.md`; this file
turns it into ordered tasks. It runs alongside WP2 and **must finish before
the WP5 freeze**. The first release is still Pop!_OS COSMIC only (the
macOS and Windows parts of each task follow the side quests in
`07-post-release-side-quests.md`).

**Rules for every task:**
- Same as WP2: tests, failure UX, accessibility, documentation and a check
  you ran yourself.
- The daemon does the work. The desktop app only draws it.
- Our own implementation only (D28).
- Each phase ships only when dictation is as reliable as before it. Run the
  dictation regression checks (daemon tests plus one live dictation) at the
  end of each task.

Tasks marked **→ T2.x** extend an existing WP2 task instead of duplicating it.

## P1: dictation polish

### F1: insert text through the clipboard (design section 2)

1. **Daemon delivery rung "clipboard paste":**
   - save the clipboard and the primary selection;
   - write the text;
   - wait for the shortcut's keys to be released (capped);
   - send one chord;
   - restore the clipboard after a desktop-specific delay;
   - serialise pastes in a queue.
2. **Chord choice:**
   - Ctrl+V for ordinary apps;
   - Ctrl+Shift+V for terminals;
   - Shift+Insert when the target is unknown.
3. **COSMIC chord sender:** `wtype` sending the chord only.
4. **Outcome reported as `inserted` or `copied`:**
   - **Inserted** is only reported once read-back through AT-SPI confirms
     it; until that exists, the outcome is honest `copied_and_pasted`
     (unverified).
   - The Inbox fallback is used only when the paste could not be sent at
     all.
5. **The overlay** shows "Inserted" or "Copied". History records the
   outcome.
6. **Typing character by character** stays available as a per-pipeline
   option.

**Check:**
- daemon tests for the ladder order, restore and queueing;
- a live dictation into a GUI app and into a terminal on COSMIC, with the
  previous clipboard content restored afterwards.

### F2: reliability details (design section 9, starting and stopping)

1. Accidental taps (no audio) are dropped silently, not reported as
   "Dictation failed".
2. A silence gate before recognition.
3. Keyboard auto-repeat never toggles twice.
4. Microphone recovery: reopen after idle, fall back to the default device
   when it is unplugged (**→ T2.7**).
5. Instant start:
   - keep the microphone ready after use;
   - keep a short buffer from the key press, so the first word is never cut.
6. Undo a cancel: a recording cancelled by mistake is kept briefly and can
   be restored from History.
7. Show recovery clearly: "Recovered 1 dictation after a crash", from the
   journal and the Inbox.

**Check:** daemon tests for each rule, plus a live double-tap.

### F3: live text for Whisper (design section 1)

1. While recording, a final-only model decodes the recent audio every
   ~1.5 s for preview partials (these never count as the final text).
2. A streaming model's own final text is used when its stream ended
   cleanly, with no second decode.
3. The overlay's streaming card shows the partials with the newest words
   shimmering.

**Check:** a daemon test with a fake engine; a live Whisper turbo
dictation on the NPU shows text while speaking.

### F4: dictionary (design section 3, **→ T2.14**)

1. T2.14's dictionary (spoken form → written form, plus plain spellings).
2. Recognition hints for transcribe.cpp (exists) and for the OpenVINO
   worker (missing).
3. Spellings passed to clean-up.
4. A Dictionary screen, with import and export.

### F5: clean-up by the language model (design section 1, **→ T2.20**)

1. The grammar model (D31) in the OpenVINO worker, as the recommended
   clean-up model; the AI model can be chosen for clean-up instead. No
   model ships with the app (D32).
2. **Models → Language models (D32):** a section separate from Speech to
   text, listing grammar and AI models with what each does, size, licence
   (with the D31 notice where limited) and a Recommended badge. The Refine
   module in the Store links here for its model.
3. The five layers against answering, including the deterministic
   faithfulness guard.
4. The "invisible wait" overlay behaviour: shimmer, in-place replacement,
   highlighted corrections.
5. Off by default in Raw; opt-in in Clean.
6. The Refine area:
   - a switch for each job (clean-up, edit by voice, translation, notes,
     chat);
   - each job's instructions can be viewed and reset;
   - the grammar and AI models come from Models → Language models (D32),
     and the user picks which one cleans up.

**Check:**
- the guard's unit tests;
- the regression set of tricky dictations (zero answers);
- clean-up latency measured on the NPU for a typical sentence.

### F6: smaller polish (design section 9)

- smart spacing, and capitals that continue the sentence;
- pausing media while dictating (MPRIS);
- undo the last dictation;
- start at login hidden in the tray, and repair the autostart entry
  (**→ T2.5**);
- optional audio keeping, so a dictation can be re-transcribed with another
  model (under the History retention settings).

## P2: voice tools

- **F7:** edit by voice (selection-aware, uses the agent or default model).
- **F8:** the translation shortcut.
- **F9:** learning words from corrections (opt-in, AT-SPI, never in privacy
  zones).
- **F10:** spoken commands and snippets (deterministic).
- **F11:** one shortcut per job; push-to-talk where supported (**→ T2.13**).
- **F12:** statistics, on this computer only.

## P2b: Narrivy's own ideas (design section 10)

- **F13:** per-app behaviour and language memory.
- **F14:** review before inserting.
- **F15:** hands-free mode (endpointing).
- **F16:** fits the computer automatically:
  - a first-run benchmark;
  - battery behaviour;
  - unloading the AI model when idle (D31).
- **F17:** the privacy dashboard.

## P3: notes

- **F18:** notes storage in the daemon (SQLite, Markdown bodies, folders),
  separate from History.
- **F19:** the editor screen; dictating into a note; model-written titles.
- **F20:** AI summary templates and actions.
- **F21:** keyword search; the Markdown folder mirror.

## P4: meetings and imports

- **F22:** meetings:
  - the microphone and the computer's sound (PipeWire monitor);
  - echo cancellation and the duplicate check;
  - a live transcript;
  - the result saved as a note with a summary;
  - a recording indicator, and a reminder that others may need to agree;
  - started by hand first, with automatic meeting detection (a call app
    using the microphone) later in this task.
- **F23:** importing audio and video files into a note (GStreamer or FFmpeg
  decode in the daemon).

## P5: deeper

- **F24:** the chat agent with tool calling, running on the AI model
  (D31):
  - local tools;
  - a confirmation card before any tool that changes something.
- **F25:** opt-in web research with citations (network module, off by
  default).
- **F26:** search by meaning (embedding model, SQLite index).
- **F27:** telling speakers apart in meetings.
- **F28:** the live captions module.

- **F31:** the voice changer extension (D33):
  - microphone in, converted voice out as a virtual microphone (a PipeWire
    source named "Narrivy voice") that calls, games and recorders can pick;
  - voice conversion on the NPU, with the two models from the owner's
    research benchmarked in F30 (latency budget about 100 ms end to end,
    quality, licence);
  - preset voices; a pass-through switch; the original microphone is never
    blocked for other apps;
  - the audio never leaves the computer and is not recorded;
  - clear labelling in the overlay while the virtual microphone is live.

**Extensions in the Store (D33):** the AI assistant (F24, F25), meeting
transcription (F22), the voice changer (F31) and the browser writing
assistant (SQ-WRITE) are listed as Extensions and turned on there.

## Engineering for the whole scope

- **F29:** the same UI on all three web engines:
  - bundle Inter;
  - replace native `<select>`s with our own component;
  - compile CSS for the engine baseline and add a lint rule;
  - Playwright screenshot tests in WebKit and Chromium.
- **F30:** benchmark the clean-up candidates (generative: S1-mini, already
  integrated; edit taggers that cannot generate text at all: GECToR
  RoBERTa, Typurr, Quire), the AI model sizes and the voice-conversion
  models (F31) on the NPU and GPU (latency, quality on the tricky-dictation set, memory),
  including prompt-lookup decoding for clean-up by the AI model, and record
  each licence, before any is recommended (D31). A model with a limited
  licence (for example non-commercial) is offered with a notice the user
  accepts before downloading, and its licence and attribution on its Store
  card.

## Considered and left out

- Outside connectors: email, chat services, issue trackers, calendars.
- Accounts, team spaces, cloud sync, enterprise sign-on, leaderboards, a
  mobile app.
- Sending a screenshot of the screen to the assistant (privacy).
- Calling the assistant by a spoken name ("Hey Narrivy"). Its own shortcut
  is the only entry (design section 1, layer 3).
- Importing from links to streaming sites (design section 8; the "video
  area" is still for the owner to confirm).

## Language models (D31, D32; amends D30)

- **Nothing ships with the app (D32):** every model is downloaded by the
  user; "default" means Recommended.
- **Where they live (D32):** Models → Language models, separate from Speech
  to text. The Store holds the Refine module (the extension), not models.
- **Grammar model (recommended for clean-up):** built only to correct grammar
  and punctuation; loaded when Refine clean-up is on. It rewrites and
  cannot answer.
- **AI model (optional):** a large general model for the AI area (edit by
  voice, translation, notes, chat, research). Loaded on demand and
  unloaded when idle.
- **Clean-up by the AI model:** the user can choose it instead of the
  grammar model.
- **Two models at once** only when the user wants it (grammar model for
  dictation, AI model for chat). Dictation keeps priority on the device.
- **Languages the grammar model does not cover:** the AI model cleans up if
  installed; otherwise only the deterministic clean-up runs.
- **Benchmarking and licence notes (F30)** come before any model is
  recommended. Limited-licence models need the user's acceptance before
  download.
