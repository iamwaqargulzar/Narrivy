# Dictation Experience and Workspace

> **Status: roadmap.** Owner direction D29 (2026-10-01). This plans work that
> is not built yet. Everything here is Narrivy's own design (D28): features
> other dictation apps have were studied for behaviour only.

Two goals:

1. Make everyday dictation feel effortless: words appear while you speak,
   the text is cleaned up, and it lands in the app reliably on Linux, macOS
   and Windows.
2. Add a workspace around dictation: the language model's settings, a
   dictionary, notes, meetings, importing recordings, and a chat. It is built
   in phases, so dictation stays reliable throughout.

The architecture rule is unchanged: the daemon does the work (audio, models,
storage, delivery); the desktop app only draws it.

## 1. The dictation flow

What makes dictation feel smooth is four steps that never make the user wait
twice:

| Step | What the user sees | How Narrivy does it |
|---|---|---|
| Speak | Words appear in the overlay as they are recognised | Streaming models stream directly. Whisper, which does not stream, decodes the recent audio every ~1.5 s for a preview. |
| Stop | The final text is ready almost at once | A streaming model's own final text is used when its stream ended cleanly, with no second decode. Otherwise the whole recording is decoded once. |
| Clean up | The overlay shows "Cleaning up…" briefly | Refine: the deterministic rules, then (when on) the shared language model (D26). |
| Insert | The text appears in the app, and the overlay says "Inserted" or "Copied" | Paste through the clipboard (section 2). |

### Making the wait invisible

Clean-up feels instant because the user is already reading the text when it
happens. Nothing is hidden behind a spinner:

1. **While speaking:**
   - The overlay shows the words as they come.
   - The newest few words shimmer, because they may still change.
   - Settled words stay still.
2. **On stop:**
   - The same text stays on screen.
   - A gentle shimmer passes across it while the clean-up runs.
3. **When the clean-up returns:**
   - The cleaned text replaces the raw text in place.
   - Corrected words are briefly highlighted, so the user sees the fix
     happen ("their" becomes "there").
4. **Inserted:** the overlay holds the final text for a moment, says
   "Inserted" or "Copied", and leaves.

**Targets:**
- The overlay shows text continuously from the first word to the insertion.
- The clean-up itself stays within about 0.5 s for a typical sentence on
  the target hardware.

The overlay's streaming card (the logo above live text) is where this runs.
For a final-only model, the card opens with the decoded text, then cleans up
in the same way.

### Clean-up by the language model

- **One job, strictly:** turn the raw transcript into clean written text.
  - Remove fillers, false starts and repetitions.
  - Fix grammar and punctuation.
  - Apply self-corrections ("Thursday, no, Friday" becomes "Friday").
  - Turn spoken punctuation ("comma", "new line") into symbols.
  - Write numbers, dates and money in standard form.
  - Add light formatting only when it clearly helps.
- **It never answers or obeys the text.** A dictated question is written
  down, not answered, and instructions inside the dictation are text, not
  commands.
- **Inputs:** the transcript, the dictionary spellings (section 3), and the
  language.
- **Settings:** temperature 0, with thinking turned off for models that have it.
- **It must never answer the dictation.** This is the classic failure of
  post-processing: "what time is the meeting" comes back as an answer
  instead of the question. Prompt wording alone only makes it rarer, so
  Narrivy uses five layers, and the last one is deterministic:
  1. **Framing:**
     - The transcript goes in clearly marked delimiters.
     - The instructions say it is text to clean, never a request to the
       model.
     - Worked examples include questions, commands and "ignore your rules"
       dictations, each cleaned, not obeyed.
  2. **Settings:** temperature 0, thinking turned off, and any thinking
     text the model still emits is removed before checking.
  3. **No guessing what the user wants:**
     - Clean-up never decides whether something was meant for the
       assistant.
     - Talking to the model is a separate, explicit action: the edit-by-voice
       or assistant shortcut.
     - A dictation is always text.
  4. **Faithfulness guard (deterministic, ADR-0018).** Before insertion,
     the output is aligned word by word with the input.
     - **Allowed changes:**
       - removed words (fillers, false starts, self-corrections, spoken
         punctuation);
       - punctuation and capitals;
       - a small number of single-word fixes;
       - numbers, dates and money rewritten in standard form;
       - dictionary spellings.
     - **Refused:**
       - a run of new content words that are not in the input (an answer
         or added text);
       - output much longer than the input;
       - empty output for real input;
       - a repeated copy of the transcript;
       - a question that lost its question form;
       - changed protected names, numbers or links.
     - **On refusal:** the text cleaned by the rules is inserted instead,
       and the refusal is counted for diagnostics, with no content logged.
  5. **Regression set:**
     - A fixed set of tricky dictations (questions, commands, "write a
       poem", "ignore your instructions", "scratch that" corrections,
       numbers) runs against every clean-up model before the Store offers
       it.
     - The bar is zero answers, and no faithfulness refusals on ordinary
       dictations.
- **Runs on the shared model** in the OpenVINO worker, preferring the NPU, or
  on Vulkan or the processor. Its latency on the NPU must be measured before
  it is on by default.
- **Per pipeline:** Raw never uses it; Clean can.

## 2. Inserting text the same way on every system

Today Linux types the text with `wtype`, which is slow for long text and
cannot be verified. Instead, every system inserts text the same way, so there
is one design to build and test:

1. **Save the clipboard.** On Linux, also save the primary selection, and
   keep every format that can be restored.
2. **Put the transcript on the clipboard.**
3. **Wait until the shortcut's keys are released**, with a short cap. A paste
   chord sent while Ctrl or Shift is still held arrives as a different
   shortcut.
4. **Send one paste chord, chosen for the target:**
   - Ctrl+V (⌘V on macOS) for ordinary apps.
   - Ctrl+Shift+V for terminals.
   - Shift+Insert when the target is unknown, and for terminal-like
     programs that treat Ctrl+V as "paste image". Shift+Insert works almost
     everywhere on Linux.
5. **Check it landed** where the system allows: read the focused field back
   through accessibility (AT-SPI on Linux, AX on macOS, UI Automation on
   Windows). The overlay then says **Inserted**. If the check is not
   possible or fails, it says **Copied** and the text stays on the clipboard.
6. **Restore the user's clipboard** after the target has read it. A
   per-desktop delay and a queue keep two pastes from overlapping. Never
   overwrite something the user copied in the meantime.
7. **Keep the Inbox** as the safety net, as today, but only for real
   failures, not for every unverified paste.

The only part that differs per system is the chord sender:

| System | Chord sender | Fallback |
|---|---|---|
| Linux, Wayland: COSMIC and wlroots | virtual keyboard (`wtype` sending only the chord) | portal or libei, then copy only |
| Linux, Wayland: GNOME and KDE | RemoteDesktop portal or libei (`portal_eis`, already present) | copy only |
| Linux, X11 | XTest | copy only |
| macOS | CGEvent ⌘V, which needs the Accessibility permission | copy only, with a prompt to grant permission |
| Windows | `SendInput` Ctrl+V | copy only |

- Nothing needs a root daemon such as `ydotool`.
- Typing character by character stays available as a per-app option for the
  rare app that blocks pasting.
- Telling terminals apart: the window class on X11; the application identity
  when the desktop exposes it; otherwise Shift+Insert.

## 3. Dictionary

One list of the user's words and spellings, used in three places:

1. **Recognition hints:**
   - Whisper's prompt or context bias on transcribe.cpp, which works today
     (`asr.context-bias`);
   - the same through the OpenVINO pipeline, which is missing today.
2. **Clean-up:** the model must keep these exact spellings.
3. **Replacements:** deterministic rules ("narrivy" becomes "Narrivy").

**Learning from corrections (opt-in):**
- For a few seconds after inserting, read only the inserted span through
  accessibility.
- If the user corrects a dictated word, offer to add it ("Learned:
  Kubernetes").
- It never runs in privacy zones (`privacy-zones.md`), never stores field
  text, and never leaves the computer.

**Screen:** add, edit, import and export words, see what was learned, and
undo it.

## 4. Language model area (Refine)

One model, loaded once (D26). It serves several jobs, each with its own
switch and instructions that can be viewed and reset:

| Job | Uses the model for |
|---|---|
| Clean-up | section 1 |
| Edit by voice | Select text, say "make this shorter", and it is replaced in place. Without a selection, the request goes to the chat. |
| Translation shortcut | Dictate in one language and insert in another. Whisper translates into English by itself; other languages use the model. |
| Notes | Summaries and actions (section 5) |
| Chat | section 6 |

- **Local only first** (D23).
- **The model comes from the Store, under Refine.**
- **Devices:** the NPU first, then Vulkan or the processor; CUDA and macOS
  hardware acceleration later (D26).

## 5. Notes

- **Storage:** notes live in the daemon (SQLite, with Markdown bodies), in
  folders. They are separate from History, with their own export and delete.
- **Editor:** a rich-text editor component, which runs well in the web view.
- **Dictate into a note,** with the same flow and clean-up as dictation.
- **AI summary:**
  - templates (sections, each with an instruction);
  - actions ("action items", "make it an email");
  - built on the language model, with a "nothing to summarise" answer for
    empty material.
- **Search:**
  - **first:** keyword search (SQLite full-text);
  - **later:** search by meaning, using a small embedding model through
    OpenVINO and an index stored in SQLite, with no separate database server.

## 6. Chat and research agent

A chat that can act, not only talk. Chats in other dictation apps that run
on a local model have no tools and no web access, so the model only knows
what it was trained on.

**The model calls tools.** The shared model (D26) gets tools to call:
- **Local tools, always available:**
  - search notes and History;
  - read a note or a file the user attaches;
  - create or append to a note;
  - today's date and time.
- **Web research tools, opt-in:** `web_search` and `read_page`.
- **Every tool that changes something** (creating or editing a note) shows
  a confirmation card first. Reading needs no confirmation.

**Web research (network module, off by default):**
- **Search provider:** chosen by the user, for example:
  - a SearXNG address, self-hosted or public, with no key;
  - a search API key (Brave Search, Tavily, Kagi and similar), stored in
    the system keyring, never in settings files.
- **Page reading:** the daemon fetches a page with limits (https only, a
  size cap, a timeout, no logins or cookies) and keeps only the readable
  text.
- **What leaves the computer:** only the search queries and the pages
  opened. Transcripts, notes and History never do unless the user puts
  them in the question. A clear "Web on" indicator shows when it can.
- **Untrusted pages:** fetched pages are data, never instructions. A page
  that says "ignore your instructions" cannot trigger a tool. The only
  network actions are searching and reading.

**Research mode:**
- **Steps:** plan, search, read, compare, then answer with numbered
  citations to the sources.
- **Live progress:** "Searching…", "Reading 3 of 5 pages…".
- **A budget:** a maximum number of searches, pages and minutes.
- **Saving:** the answer can be saved as a note with its sources.

**Two kinds of model (D31):**
- **Grammar model:** built only to correct grammar and punctuation. It is
  the default for clean-up, and it cannot answer the dictation because it
  only rewrites.
- **AI model:** a larger general model for the AI area, loaded when used
  and unloaded when idle. The user may also pick it for clean-up.
- **Two at once only by choice:** for example the grammar model for every
  dictation and the AI model for chat. Dictation keeps priority.

**Model size matters for research:**
- **The trade-off:** a small model plans research poorly. The Store offers
  the AI model in sizes (for example 4B balanced, 8B best) for the user to
  choose.
- **Speed on the NPU:** the larger sizes must be measured before they are
  recommended (F30).

## 7. Meetings: microphone and speaker together

- **Two streams:**
  - the microphone, labelled "You";
  - the computer's sound, labelled "Others", captured from the PipeWire
    monitor on Linux, WASAPI loopback on Windows, and Core Audio taps or
    ScreenCaptureKit on macOS.
- **Echo:**
  - **audio level:** echo cancellation removes the speakers' sound from the
    microphone (WebRTC AEC3 or PipeWire's echo-cancel module, both already in
    the module catalog);
  - **text level:** a check drops microphone lines that repeat what "Others"
    just said.
- **Live:** the transcript appears while the meeting runs. At the end it
  becomes a note with a template summary (section 5).
- **Telling speakers apart within "Others"** comes later, with a speaker
  embedding model.
- **Started by hand first.** Detecting a meeting automatically (a call app
  using the microphone) comes later.
- **Consent:** a clear indicator while recording, and a reminder that others
  may need to agree.
- **NPU fit:** two streams at once suit the NPU, leaving the processor and GPU
  for the call itself. The same capture powers the live captions module.

## 8. Importing recordings ("video area": owner to confirm)

- **Audio and video files** (drag in, or batch) are transcribed into a note.
  The daemon decodes them through GStreamer or FFmpeg.
- **Links to streaming sites are left out.** Downloading from them depends on
  tools that break often and carry terms-of-use questions.

## 9. Reliability and polish details

Small things that make daily use feel finished. Most are cheap.

**Starting and stopping**
- **Accidental taps are not failures.** A recording with no audio, from a
  double-tap, is dropped silently. Today it shows "Dictation failed"
  (`capture_finalized_before_audio`).
- **Silence is not sent to recognition.** A recording that holds only
  silence is skipped, so Whisper never invents "Thank you." on silence.
- **A held key is one press.** Keyboard auto-repeat never toggles
  dictation twice.
- **Instant start:**
  - keep the microphone ready after use, and reopen it when the system has
    put it to sleep;
  - keep a short buffer from the key press, so the first word is never cut.
- **Microphone recovery:**
  - when the chosen microphone is unplugged, switch to the default and say
    so;
  - when a microphone returns silence after idle, reopen it.
- **Undo a cancel:** a recording cancelled by mistake is kept for a short
  while and can be restored from History.

**While dictating**
- **Pause media while dictating:** music or video pauses and resumes after.
  This uses MPRIS on Linux, the media session API on Windows, and the system
  media controls on macOS.
- **Smooth live text:** new words fade in at an even pace, and the overlay
  measures before it resizes, so it never jumps (the overlay redesign
  already eases).

**Inserting**
- **Smart spacing:** a space between two dictations in a row, but not after
  Chinese or Japanese text, which uses no spaces.
- **Capitals that follow on:** continuing mid-sentence starts in lower case
  when the field allows reading what comes before.
- **Undo the last dictation:** one shortcut removes what was just inserted.
- **Insert again:** from History, which exists already ("Paste again").

**Shortcuts**
- **One shortcut per job:** dictate, edit by voice, translate, meeting.
- **Push-to-talk** where the system reports key release (X11, Windows,
  macOS). COSMIC and other Wayland desktops toggle only.

**Keeping things**
- **Optional audio keeping,** so a dictation can be re-transcribed with
  another model, under the History retention settings.
- **Notes mirrored as Markdown files** to a folder of the user's choice, so
  notes work in other editors (for example Obsidian). The notes stay the
  user's own files.
- **Note titles written by the model** until the user types their own.
- **Dictionary import and export.**

**Statistics (on this computer only)**
- Words dictated, words per minute, streaks, time saved compared with
  typing, and a calendar heatmap.

**Starting with the system**
- Start at login, hidden in the tray.
- Repair the autostart entry when the app has moved.

## 10. Narrivy's own ideas

Things that would set Narrivy apart:

1. **Per-app behaviour.** The focused app chooses the pipeline (context
   fabric, privacy zones):
   - a code editor or terminal gets Raw, no clean-up, and terminal paste;
   - an email client gets Clean with a more formal clean-up;
   - a privacy zone gets no learning and no History.
2. **Spoken commands without a model:** "new paragraph", "scratch that",
   "delete last sentence", "all caps". These are deterministic, instant and
   reliable; the language model is not needed for them.
3. **Snippets:** "insert my address" expands to saved text. Storage already
   plans "learning, dictionary and snippets".
4. **Review before inserting** (opt-in, per app): for long or important
   text, the overlay shows the cleaned text first. Enter inserts it, Esc
   cancels, and the text can be edited.
5. **Hands-free mode:** dictation stops after a pause, using the endpointing
   the daemon already has (`speech.endpoint`).
6. **Fits the computer automatically:**
   - on first run, a short benchmark picks the best model and device for
     this machine, using the Store's fit verdicts;
   - on battery, it prefers the NPU and unloads the language model when idle,
     while keeping speech recognition loaded.
7. **Privacy dashboard:** what is stored (History, audio, notes, learned
   words), how long it is kept, and one button to wipe each.
8. **Language memory:** each app remembers its dictation language, with
   automatic detection as the fallback.
9. **Never lose text:** the crash-safe journal and the Inbox already do
   this. Show it clearly: "Recovered 1 dictation after a crash".
10. **Live captions module** (planned separately): the same capture and
    overlay, for any video or call.

## 11. Left out

- Accounts, team spaces and cloud sync.
- Outside connectors (email, chat services, issue trackers) and calendar
  integration.
- Enterprise sign-on and cloud keys managed by an organisation.
- Leaderboards.
- A mobile app.

## Phases

Each phase ships only when dictation stays as reliable as before.

| Phase | Contents |
|---|---|
| P1: dictation polish | Inserting text through the clipboard on Linux, with Inserted or Copied and the clipboard restored. Live text for Whisper. The dictionary with recognition hints for both engines. Clean-up by the language model (opt-in) with its guard. The cheap reliability details: accidental taps, the silence gate, auto-repeat, microphone recovery, smart spacing, pausing media. |
| P2: voice tools | Edit by voice; the translation shortcut; learning from corrections; spoken commands; snippets; undo the last dictation; one shortcut per job and push-to-talk where possible; statistics. |
| P2b: fits the computer | Per-app behaviour, language memory, review before inserting, hands-free mode, the first-run benchmark and battery behaviour, the privacy dashboard. |
| P3: notes | Notes storage and editor, dictating into a note, keyword search, AI summaries and actions, model-written titles, the Markdown folder mirror. |
| P4: meetings and imports | Microphone and computer sound into a note, with echo handling; importing audio and video files. |
| P5: deeper | Chat with tools (local tools, then opt-in web research with citations); search by meaning; telling speakers apart; the live captions module. |

The chord senders for macOS and Windows (section 2) are built with each
system's packaging work. On Linux, all of P1 runs on COSMIC first.
