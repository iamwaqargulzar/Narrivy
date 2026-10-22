# Browser writing extension: research, feature map and build plan

> **Status (2026-10-05):** research done, and phase W1 is built: the checker,
> IPC minor 24, the host and installer, and the extension, tested in
> Chromium. W2–W4 are planned. It extends the D26 design in `writing-assistant.md`, which
> remains the source of the product rules (native messaging only, one
> shared language model, privacy). Owner request: map every feature and
> option, then build it and install it in Brave.

## 1. What was studied

The rule (D28) is ideas only: we study how others solve a problem and write
our own code. Libraries used as dependencies are fine.

| Source | Licence | What we took from it (ideas) |
|---|---|---|
| Harper's Chrome extension and lint framework (Automattic) | Apache-2.0 | <ul><li>Mirror element for underlines in `<textarea>`/`<input>`.</li><li>The CSS Custom Highlight API for `contenteditable`.</li><li>Walking up to known editor roots (ProseMirror, Lexical, Slate, Draft, CKEditor, Quill, Trix, CodeMirror).</li><li>Replacing text through `beforeinput`/`execCommand('insertText')` so the editor and its undo stack see a normal edit.</li><li>Watching every scrollable ancestor.</li><li>Google Docs through its accessibility SVG rects.</li><li>Per-domain switches, a user dictionary, ignored lints, dialect and a delay setting.</li></ul> |
| `harper-core` crate | Apache-2.0 | Used as a **library** in the daemon: about 200 rules, five English dialects, suggestions, under 10 ms per check. |
| LanguageTool | Server LGPL; browser add-on "All Rights Reserved" since 2019 | Product ideas only. Its add-on is not open source, so no code was read: <ul><li>coloured underlines by category;</li><li>a per-field badge with a count;</li><li>a full-text panel;</li><li>auto-detected language;</li><li>"picky" mode.</li></ul> |
| Grammarly (public help pages and visible behaviour only) | Proprietary | The feature list in §2: <ul><li>underline colours;</li><li>suggestion cards with Accept, Dismiss and Add to dictionary;</li><li>a field badge with a count and a spinner while checking;</li><li>a generative-AI button;</li><li>tone detection;</li><li>synonyms on double-click;</li><li>per-site disable;</li><li>category toggles;</li><li>dialect;</li><li>editable shortcuts.</li></ul> Its minified code was not studied. |
| Local-LLM extensions (ai-grammar, GrammarJo, Vaayu) | MIT/OSS | <ul><li>Selected text, then an action, then a preview, then Replace.</li><li>They reach Ollama over a local HTTP port, which D26 forbids for Narrivy (any web page could reach the port).</li></ul> |

## 2. Feature map

Legend: **W1** is being built now, **W2–W4** are later phases, and
**—** means deliberately not planned.

### Checking while typing (rule checker, harper-core in the daemon)

| Feature | Grammarly | LanguageTool | Harper | Narrivy |
|---|---|---|---|---|
| Underlines in textarea/input | ✓ | ✓ | ✓ | W1 (mirror overlay) |
| Underlines in contenteditable / rich editors | ✓ | ✓ | ✓ | W1 (CSS Custom Highlight API, overlay fallback) |
| Colour by category (spelling, grammar, style…) | ✓ | ✓ | ✓ | W1: red spelling, amber grammar/punctuation, blue clarity/style |
| Suggestion card on hover/click | ✓ | ✓ | ✓ | W1 |
| Apply a suggestion in one click | ✓ | ✓ | ✓ | W1 (native edit, so Ctrl+Z works) |
| Ignore once | ✓ | ✓ | ✓ | W1 |
| Add to personal dictionary | ✓ | ✓ | ✓ | W1 |
| Turn a rule off from the card | ✓ (category) | ✓ | ✓ | W1 |
| Field badge with issue count and checking spinner | ✓ | ✓ | — | W1 |
| Panel listing all issues + "Fix all" | ✓ (sidebar) | ✓ | — | W1 |
| Dialect (US/UK/CA/AU/IN) | ✓ | ✓ | ✓ | W1 |
| Check delay setting | — | — | ✓ | W1 |
| Rule list with on/off per rule | partly | ✓ | ✓ | W1 (options page) |
| Readability / document score | ✓ | — | — | W3 |
| Synonyms on double-click | ✓ | ✓ | — | W3 (harper-core thesaurus) |
| Definitions | ✓ | — | — | — (needs a dictionary dataset) |
| Other languages | ✓ | ✓ (30+) | — | W4 (LanguageTool server as an optional module) |
| Plagiarism / AI detection | ✓ | — | — | — (needs the network) |

### AI writing actions (the assistant model, NPU/GPU)

| Feature | Grammarly | Narrivy |
|---|---|---|
| Action bar on selected text | ✓ (lightbulb) | W1 |
| Fix grammar (faithful) | ✓ | W1: **Fix**, guarded by the dictation faithfulness check |
| Improve / clearer | ✓ | W1: **Improve** |
| Rewrite | ✓ | W1: **Rewrite** |
| Shorter / longer | ✓ | W1: **Shorter**, **Longer** |
| Tone: formal, friendly, confident | ✓ | W1: **Formal**, **Friendly**, **Confident** |
| Bullet points / summary | ✓ | W1: **Bullets**, **Summarise** |
| Custom instruction ("make it sound like…") | ✓ | W1: free-text prompt box |
| Streaming preview before replace | ✓ | W1 |
| Replace / Insert below / Copy / Try again / Stop | ✓ | W1 |
| Warning when numbers, links or names changed | — | W1 (ADR-0018 guard) |
| Undo after replace | ✓ | W1 (native undo, plus an "Undo" link on the toast) |
| Whole field "Improve all" | ✓ | W1: the action bar on the badge acts on the whole field |
| Tone detection of what you wrote | ✓ | W3 (one short model call, shown on the badge) |
| Reply/compose from context (email thread) | ✓ | W4 (needs explicit per-site context permission) |
| Translate | ✓ | W3 |
| Keyboard shortcut for the action bar | ✓ | W1: `Alt+Shift+G` (changeable at `brave://extensions/shortcuts`) |
| Right-click menu "Narrivy: Fix / Improve / Rewrite…" | — | W1 |

### Control, privacy and status

| Feature | Narrivy |
|---|---|
| Popup: on/off for this site, global on/off, connection state, assistant model state | W1 |
| Never read password, credit-card, one-time-code, `autocomplete=off` secret or hidden fields | W1 |
| Skip code editors (CodeMirror, Monaco, Ace) and `spellcheck=false` code fields | W1 |
| Sensitive sites (banking, health, government login) off by default | W1 (a pattern list; the user can turn a site on) |
| No network permission, no host permission beyond content scripts | W1 |
| Text never logged, never in History | W1 (daemon and host both) |
| Clear message when Narrivy is not running or no assistant model is chosen | W1 |
| Google Docs | W2 (accessibility-rect reader, like Harper) |
| Shadow-DOM editors (Reddit, some web components) | W2 |
| Same-origin iframes (Gmail compose body, many CMSs) | W1 (`all_frames`) |
| Firefox (same native messaging) | W3 |
| Dictionary synced with the desktop app's custom words | W3 |
| Writing actions outside the browser (select text anywhere, then a shortcut) | W4 (D26 "later") |

## 3. How it is built

```
Brave page ─ content script (underlines, cards, action bar)
               │ chrome.runtime messages
             service worker (settings, one native port)
               │ native messaging: stdin/stdout, 4-byte length + JSON
             narrivy-browser-host (Rust, started by the browser)
               │ same-user Unix socket, existing IPC (role "browser-host")
             narrivyd ─ harper-core checker (CPU, in process)
                      └ assistant model worker (NPU/GPU), shared with the Assistant tab
```

- **The host is the boundary.** The daemon does not trust handshake roles
  (ADR-0031/0061). So the host is the only thing the browser can start,
  and it maps its small protocol onto exactly these daemon commands:
  - `GetStatus`;
  - `Subscribe`, whose events are filtered to the host's own turns;
  - `CheckWriting`;
  - `ListWritingRules`;
  - `WritingAction`;
  - `AssistantCancel`.

  A unit test pins that list.
- The host manifest's `allowed_origins` lists only Narrivy's extension
  ID. The ID is fixed by a public key in `manifest.json`.
- **Rule checking in the daemon** (not WASM in the page), so the same checker
  serves "writing actions outside the browser" later and the extension stays
  small. A check is one round trip of a few milliseconds.
- **Writing actions reuse the assistant model and its queue**, so no
  second model is loaded (D26). The daemon owns the prompts. The final text
  is tidied (quotes and "Here is…" openers removed) and guarded:
  - **Fix** must pass the dictation faithfulness check;
  - every action must keep the original's numbers, links and email
    addresses.

  A failed guard does not block the reply. It is shown as a warning on the
  preview.
- **Offsets are UTF-16** on the wire (what the browser uses); the checker
  converts from Rust char offsets.
- **Edits** go through the editor:
  - **textarea/input:** select the span, then `execCommand('insertText')`, so
    undo works; fall back to `setRangeText` plus an `input` event;
  - **contenteditable:** a DOM Range on the span, then a `beforeinput` with
    `insertReplacementText`, then `execCommand('insertText')`.

## 4. IPC additions (protocol minor 24)

- `CheckWriting { text, dialect, disabled_rules, words }` returns a list of
  `WritingFinding { start, end, kind, rule, message, suggestions[] }`.
- `ListWritingRules` returns `{ name, description, enabled_by_default }[]`
  for the options page.
- `WritingAction { turn_id, action, text, instruction? }` is accepted at
  once. The reply streams as `AssistantDelta`, and `AssistantDone` gains an
  optional `warning`.

## 5. Phases

| Phase | Content |
|---|---|
| **W1 (now)** | <ul><li>Checker crate and IPC.</li><li>Host binary and installer for Brave/Chrome/Chromium.</li><li>Extension with:<ul><li>underlines;</li><li>cards;</li><li>badge and panel;</li><li>action bar and preview;</li><li>popup;</li><li>options;</li><li>context menu;</li><li>shortcut;</li><li>privacy exclusions.</li></ul></li></ul> |
| W2 | <ul><li>Google Docs.</li><li>Shadow-DOM editors.</li><li>A site-compatibility test page set.</li></ul> |
| W3 | <ul><li>Synonyms.</li><li>Tone detection.</li><li>Readability score.</li><li>Translate.</li><li>Firefox.</li><li>Dictionary shared with the desktop app.</li><li>Store listing as an Extension (D33).</li></ul> |
| W4 | <ul><li>Other languages.</li><li>Reply/compose with context.</li><li>Writing actions in any application.</li></ul> |

## 6. Tests

- Checker unit tests: offsets with emoji and accents, dictionary words,
  disabled rules.
- Guard unit tests.
- Host tests:
  - framing;
  - the command allow-list;
  - turns from other clients are not forwarded.
- IPC round-trip tests.
- Manual: the extension on plain textareas, Gmail, GitHub comments, Reddit
  and a contenteditable test page in Brave.
