# Progress log

One line per finished task: `date | task | commit | check | result`.

| Date | Task | Commit | Check | Result |
|---|---|---|---|---|
| 2026-09-29 | Record scope amendment and owner decisions | cb32866 | verify_traceability, verify_phase_model, verify_owner_gates, validate_pack, amendment_gate | pass |
| 2026-09-29 | Add completion plan | 08f1074 | validate_pack | pass |
| 2026-09-29 | WP1 T1.3 item 5 (P-6/B26): quarantine 9 refresh scripts + 7 legacy runners | 4a558ee | pytest tests (303) | pass |
| 2026-09-29 | WP1b T-OSS.1: root 33 → 16 files; archive build pack | 585d6bc | validate_pack, amendment_gate, pytest | pass (kill_mid_migration flaky 1/10, recorded) |
| 2026-09-29 | WP1b T-OSS.3/T-OSS.5: archive agent history, quarantined evidence, placeholders | 8adb86c | validate_pack, amendment_gate, verify_traceability, pytest | pass |
| 2026-09-29 | WP1 T1.1/T1.2/T1.3 item 4: truthful STATE, BLOCKERS, PHASES, templates | 3f79e7f | release_gate (exit 1: 273 lines, 0/93 accepted), gate validators, pytest | pass |
| 2026-09-29 | WP1b T-OSS.6: crate descriptions, licence, repository, 24 READMEs | e26a8a9 | cargo check, cargo deny, boundary validators, pytest | pass |
| 2026-09-29 | WP1b T-OSS.2: docs by audience; 28 docs archived; links rewritten; check_docs.py | b6392c1 | cargo test (1631), npm test (113), pytest, check_docs, all gate validators | pass |
| 2026-09-29 | WP1b T-OSS.2: docs/README.md index; roadmap status banners | 4134983 | check_docs, validators, pytest | pass |
| 2026-09-29 | WP1 T1.3 item 4 / P-7: Pop!_OS-only scope in live platform/qualification docs | e5ec9da | check_docs, validators, pytest | pass |

## 2026-09-29: T1.4 step 6, switch to Tauri everywhere (ADR-0075, D22)

- The workspace builds `apps/narrivy-desktop/src-tauri`. `src-gtk` and
  `adapters/desktop-parent-linux` are removed (they stay in git history).
- `cargo deny check` passes: plist 1.10.1, quick-xml 0.42.0 and time 0.3.55
  are pinned, and 8 unmaintained notices are ignored by ID with reasons.
- CI, recorders (25 runner pins), `.deb` Depends (WebKitGTK 4.1, GTK3,
  gtk-layer-shell) and the governance tests are updated. Docs, AGENTS.md,
  B-DESKTOP-008 and B-SUPPLY-010 are updated.
- Verified: fmt, clippy `-D warnings`, 1619 Rust tests, 114 frontend tests,
  304 Python tests (the flaky kill-mid-migration test is deselected), cargo
  deny, validate_pack, amendment_gate, owner gates, boundaries and check_docs.

## 2026-09-29: first native run fixes

- The owner's first launch hit a month-old installed 0.2.3 package (protocol
  1, the app needs 9) and an old state-folder symlink. Both are removed; the
  owner purged the package.
- F-H10 fixed: the real-engine (`asr-transcribe-resident`) daemon tests
  compile and pass, its two lint errors are fixed, and CI now tests and lints
  that build.
- `scripts/dev-daemon.sh` runs real dictation in development (PipeWire,
  resident Whisper, wtype then Inbox); `scripts/dev-desktop-entry.sh` gives
  the dev build a dock icon.
- Desktop: system tray (open, start/stop dictation, quit), close-to-tray,
  and a single-instance guard. Startup no longer blocks the UI thread on
  daemon probes outside qualification lanes.
- Daemon: the stale `marker-x7q` probe is removed, and "serving on" now
  prints only once the daemon can answer.

## 2026-09-29: desktop UI revamp (owner: full creative freedom, then direction)

- New design language: dark-first "quiet instrument" tokens, bundled Geist and
  Geist Mono (OFL-1.1), one coherent icon set, a rail with Home, History,
  Models and Settings, plus Diagnostics and About in the footer.
- Home is a live overview (owner direction): session state, active pipeline
  flow and mode, latency (shown only when the daemon reports it), model,
  compute ladder (only when reported), microphone, text insertion, shortcut
  command, History and health, plus a test-dictation control and the last
  result. The Dictation screen merged into Home; the Applications
  placeholder was removed.
- Settings is a complete feature map (`src/lib/featureMap.ts`): 48 settings in
  11 sections with search. Each row is Live, From service, Coming (with its
  plan task) or Proposed. The old localStorage-only rows that never reached
  the daemon are no longer presented as working.
- `npm run dev` renders the app in a browser through a dev-only preview bridge.
- THIRD_PARTY_NOTICES updated (stale GTK3 text, Tauri host, frontend, fonts):
  owner review needed (D9).
- Follow-up: the Proposed items (start hidden, sounds, spoken punctuation,
  trailing space) need owner decisions.

## 2026-09-29: UI rebuilt to the approved mockup

- The owner rejected the free-form revamp ("not what I have envisioned") and
  pointed to `spec/ui/reference-images/`. The shell and Home now follow
  `narrivy-home-dark-approved.png`; Settings follows the Compute reference
  (top tabs, rich cards, compute priority, insertion fallback chain).
- Kept from the revamp: the honest feature map, the preview bridge, live
  status plumbing. Fonts: Inter and JetBrains Mono (OFL-1.1), bundled.
- Restored Dictation and Applications; added Pipelines.

## 2026-09-29: Home reshaped to three sections (owner direction)

- Home: (1) dictation button plus live/last text, (2) read-only Active
  pipeline status (microphone, noise suppression, transcription, refine,
  deliver; changes happen in Pipelines), (3) system status: latency,
  resources, services with an overall pipeline verdict. Refreshes every 3 s
  while visible. Mode selection moved to Dictation.
- Top bar: Tips pop-up next to Search; app version chip (from
  tauri.conf.json) with the service build in its tooltip.
- Daemon: new content-free `diagnostics.resources.*` tokens (RSS, process
  count, CPU share) for the service's own process tree, sampled from /proc by
  `narrivy-process-linux::usage` (bounded walk).
- Open: product versions disagree (daemon 0.0.0, desktop crate and .deb
  control 0.1.0, tauri.conf 0.2.3); single-source them in T3.2.

## 2026-09-29: premium monochrome redesign and History delete

- Owner direction: modern, Wispr Flow-like, "not AI slop"; sketch for the
  first section (mic + Ready, text box, Recent list with Copy/Retry/Delete).
- Palette: near-black and grays with an accent picker (Mono, Blue, Gold) in
  the top bar and Settings; serif display greeting (Instrument Serif, OFL).
- Home: greeting, dictation card with Recent, hairline Active pipeline line,
  System stats (latency, memory, CPU) and service list.
- New IPC command `DeleteHistoryEntry` (protocol minor 10): History store,
  daemon handler (Destructive class), desktop bridge, tests.
- Retry is shown only for dictations that were not inserted and stays
  disabled: re-inserting text and retrying a failed transcription both need
  an owner decision (see the question raised on 2026-09-29).

## 2026-09-29: one type scale, appearance settings

- Typography: Inter everywhere and Instrument Serif only for the greeting;
  JetBrains Mono removed (technical values use Inter tabular figures). Every
  size and weight comes from tokens: sizes 12/13/14/16/20/28/44px, weights
  400/500/600 (was 20 sizes and 6 weights).
- The accent picker lives only in Settings → General (removed from the top
  bar). New "Show greeting" switch there (client preference,
  `localStorage['narrivy.greeting']`); tighter spacing around the greeting.
- Switch thumbs use the accent contrast colour, so an "on" switch stays
  readable with the Mono accent.

## 2026-09-29: About and the version button

- "About / Updates" is now "About". The version left the header and sits
  under About in the sidebar footer as a button that opens About, with its
  update state.
- The update state is "Not checked": under the locked update policy
  (`docs/maintainers/operations/update-policy.md`) the app has no network
  access and a check may only run on request with the user's opt-in, which
  is not built yet. About shows an Updates panel with a disabled "Check for
  updates" (marked Coming), the reason, and the apt command. The UI never
  says "Up to date" without a real check; a test pins this.

## 2026-09-29: live status bar, compact About/version

- The bottom bar is live: the device recognition runs on, the model and the
  mode on the left (a missing mic or text delivery appears only as a
  warning); memory, CPU share and typical latency of the Narrivy service on
  the right. Unreported values show a dash.
- Status freshness moved from Home into the status provider: a quiet read
  every 3 s while the window is visible, paused during an active model
  provision (which keeps its own deadline-bounded poll).
- Daemon fix: the strict production capability path returned before the
  compute plan was appended, so `compute.device*` never reached clients.
  It is now reported (test `strict_capabilities_report_the_compute_plan`).
- Pipeline gap found (for the pipeline pass): the compute plan is advisory
  only; nothing routes recognition by it and the transcribe.cpp worker runs
  on the CPU. With no `qualifications.json` the planner selects nothing. The
  bar therefore names the device from engine tokens and never claims a
  planned NPU/GPU as running.
- Sidebar footer: About and the version button side by side, 50/50.

## 2026-09-30: Home Active pipeline card, History and Pipelines polish

- Home's Active pipeline is a card like the dictation section: five stage
  tiles in one row (icon, active value, read-only facts, state), each fact
  from a daemon-reported value: capture limit, device, language, live vs
  after-stop text, the mode's description, the Inbox backup.
- History: "committed" reads "Not inserted" (same rule as Recent), unmapped
  states read as words, raw transaction ids are no longer shown, rows can be
  deleted (protocol minor 10).
- Pipelines: redundant id chip removed; import/export labels in sentence case.

## 2026-09-30: Pipelines screen redesign (first pass)

- Each pipeline is a card: drag handle (and up/down buttons for keyboard),
  click-to-rename, version, In use / Use this pipeline, five stage buttons
  with icons (Microphone, Noise suppression, Transcribe, Refine, Deliver).
  Clicking a stage expands its options; every option carries its standing:
  Live, From service, Coming (task) or Proposed.
- Live now: rename and order (this app's preferences,
  `localStorage['narrivy.pipelines.prefs.v1']`), choosing the pipeline in
  use, a per-pipeline shortcut command and per-pipeline language shortcuts
  (`narrivy-cli toggle --pipeline ID [--language TAG] [--output-language
  TAG]`, flags the CLI already accepts), and the recognition languages/
  context hints (existing RecognitionSettingsCard).
- Needs backend work before the planned/proposed controls can go live:
  daemon-side pipeline names/order and a pipeline editor (rename in the tray
  and CLI too); noise suppression stage (no plan task yet; catalog has a
  built-in light DSP for 1.0); model identity tokens (name, quantization,
  size, backend) in status; per-pipeline model and device choice (T2.17,
  T2.15, T2.16); mic device choice and number formatting (T2.12);
  vocabulary/replacements (T2.14); delivery method and clipboard restore
  (T2.4); AI polish, meaning check, trailing space, gain and echo
  cancellation (proposed, owner decision).

## 2026-09-30: Module Store and shortcut capture designs (proposed)

- Owner asked for a module store (per-hardware add-ons with an update
  path), a Refine screen with rules plus an optional language model, and
  press-a-button shortcut capture as the first option.
- Proposals, built on ADR-0014/0015/0016/0017/0018, with edge cases and the
  owner decisions each needs: `docs/roadmap/design/module-store.md` and
  `docs/roadmap/design/shortcut-capture.md`. Nothing is implemented; both
  wait for the owner's answers.
- Fact recorded: COSMIC keeps custom shortcuts in a per-user text file (one
  entry per binding, press only), so capture-then-register is feasible
  through a platform adapter; press-and-hold still needs T2.13.

## 2026-09-30: shortcut capture on COSMIC (D24)

- Adapter `adapters/platform-linux/src/desktop_shortcuts.rs`: reads and edits
  the desktop's custom-shortcut file by text splice (other entries stay
  byte-for-byte, an unrecognised file is never written, atomic writes).
- Service `services/narrivyd/src/daemon/shortcuts.rs`: builds the command
  only from the CLI path, a canonical pipeline id and validated language
  tags; lists, updates and removes only Narrivy's entries; reports a
  conflict with a content-free name; keeps a replaced custom entry and
  restores it on removal. Status token `activation.shortcuts=`.
- IPC protocol minor 11: `ListShortcuts`, `SetShortcut`, `RemoveShortcut`
  (spec/protocols/ipc.md). Desktop host commands and frontend bridge.
- UI: "Add shortcut" on each pipeline opens a press-the-keys dialog; saved
  shortcuts show as key chips with remove; language shortcuts use the same
  dialog; the copy-a-command route is the second option. Onboarding wording
  changed per D24.
- Known limit: the desktop handles its own shortcuts before any window sees
  the keys, so a combination the desktop already uses usually cannot be
  captured at all; the conflict path mostly covers other custom shortcuts
  added since the dialog opened, and requests from other clients.
- Still open: remove Narrivy's bindings on package purge (packaging, WP3);
  a post-save check that the key triggers; other desktops (after 1.0).
- For T2.5 (start at login): the daemon detects the desktop from
  `XDG_CURRENT_DESKTOP`. A systemd user service does not inherit it unless
  the session imports it, so the unit must get the session environment or
  shortcuts will report `manual:desktop_not_supported`.

## 2026-09-30: History shows the pipeline and the transcription time

- The retained History store gains a nullable `transcribe_us` column
  (additive in-place migration; older rows stay unmeasured). The daemon
  records capture-end to raw-text for each completed dictation and reports
  it on `GetHistory` rows (protocol minor 12, optional field).
- History rows show the pipeline (with the user's own name for it) and
  "Transcribed in …" when measured; nothing is shown for unmeasured rows.
- Owner confirmed shortcut capture works on the reference machine: a
  captured combination started dictation with its pipeline.

## 2026-09-30: desktop quit crash found and fixed

- Symptom: `free(): corrupted unsorted chunks` / `corrupted double-linked
  list` in the terminal after quitting. Apport's report
  (`/var/crash/..._WebKitWebProcess.1000.crash`, SIGABRT, cwd the project)
  showed the crashing process is the Linux web view's content process, not
  the shell: it aborts inside its own exit cleanup.
- Reproduced by clicking the tray's Quit over D-Bus (`com.canonical.dbusmenu`
  `Event`) in a loop after two start/cancel dictation cycles: 3 crashes in
  72 quits before the fix. Closing the web views first and waiting 400 ms
  did not help (1 in 30).
- Fix: on Quit (and on window close when there is no tray) the shell ends
  each web view's content process through `terminate_web_process` before it
  exits, so that cleanup never runs. 0 crashes in 94 quits after the fix.
- Also fixed on the way: `App::run` never returns, so the session-event
  worker was never joined and the overlay's widgets were left to process
  teardown. Both are now released in `RunEvent::Exit`.
- Not done: a stack trace of the WebKit abort (needs a debugger and debug
  symbols; WebKitGTK 2.52.6) and an upstream report.

## 2026-09-30: Refine screen (read-only first version) and D23 tasks

- New plan tasks for the approved D23 work: T2.19 (Module Store, 1.0
  slice), T2.20 (Refine: rules and a local language model), T2.21 (built-in
  light noise suppression).
- Refine sidebar screen: the five rules the deterministic corrector applies
  today (read from which pipeline is in use), per-rule settings and
  vocabulary as Coming, context hints as Live; the language model section
  (model, device, level of change, meaning check, local-only) as Coming with
  its task. Nothing on the screen is selectable until the daemon serves it.
- Pipelines: noise suppression, AI polish and the meaning check moved from
  Proposed to Coming with their tasks; the cloud polish choice was removed
  (D23: local only first).
- Shared option/badge components moved to
  `app/components/StandingOptions.tsx`.

## 2026-09-30: first model packs staged for the narrivy Hugging Face org (T2.17 step 1)

- Owner chose the first packs: Whisper large-v3 and large-v3 turbo, each as
  GGUF (transcribe.cpp / Vulkan lane) and OpenVINO IR, plus Parakeet Unified
  EN 0.6B (streaming). The files already on the owner's computer are used
  as they are; each pack states its quantization (owner direction).
- Verified by SHA-256 against the upstream repositories: all 41 files are
  byte-identical to `handy-computer/*-gguf` at the cached revisions and to
  `OpenVINO/whisper-large-v3{,-turbo}-int8-ov` at their current revisions.
  Licences: Apache-2.0, MIT, and CC-BY-4.0 (Parakeet, attribution required).
- `tools/reference-models/packs.json` + `build_model_packs.py` verify and
  stage the packs (manifest + model card each); manifests are recorded in
  `tools/reference-models/manifests/` and schema-checked by validate_pack.
- Quantizations: turbo GGUF Q8_0, large-v3 GGUF Q5_K_M (the 8-bit file is
  not on this computer), Parakeet GGUF Q8_0, both OpenVINO packs INT8.
- Published by the owner on 2026-09-30; revisions recorded in
  `tools/reference-models/published.json` and verified against the pins by
  `verify_published.py` (all five packs OK).
- Not done: the download transport (T2.17 step 2). The OpenVINO
  packs cannot run in Narrivy until the OpenVINO engine exists (T2.16), and
  Parakeet/large Whisper through transcribe.cpp are not yet qualified here.

## 2026-09-30: Models page wired to a real model listing

- Parakeet Unified EN 0.6B (Q8_0) loads in the resident worker on the
  reference machine: the daemon reports `model-streaming=supported` and the
  streaming, VAD and endpoint paths as composed (unqualified); ~1.1 GB RSS.
  The trust gate refused the staged hard-linked, group-writable file, so a
  private 0600 copy in the 0700 model cache is used.
- Fixed: the mode listing could stay "No dictation mode is currently
  available" after a model change (read once per readiness change, possibly
  just before the worker was ready). A status that disagrees with the held
  listing now triggers a re-read.
- Registry: the three published GGUF packs were added to the seeded
  registry (sources: the narrivy org at the published revisions, pinned
  SHA-256, unqualified). `narrivy-model-manager::model_catalog()` lists the
  registry plus the published multi-file (OpenVINO) packs.
- IPC protocol minor 13: `ListModels` returns each catalog model with its
  quantization, size, licence, engine, languages, streaming, source and
  qualification, and the daemon's observed state (`active`, `installed`,
  `available`, `needs_engine`; an unknown loaded model is `unlisted`).
  `narrivy-cli models` prints it.
- Models page: "On this computer", "Available to download" and "Waiting
  for their engine", each model with its quantization and details. "Use
  this model" and "Download" are shown as Coming (T2.17): switching the
  loaded model and the verified download transport are not built yet.

## 2026-09-30: Models page redesign (owner reference: Handy's model list)

- `ListModels` rows carry `devices` (the device classes a model may be
  qualified on; eligibility, not a claim about this computer).
- Models page: search, filters (language, runs on, live text) and sort
  (recommended, name, size); rows without the square icon, the quantization
  beside the name with its bit width, facts with icons, size on the right.
- Deliberately not copied from the reference: accuracy/speed bars (no
  measured data yet; they belong to qualification results), per-model
  hotkeys and Delete (need model switching and management, T2.17).

## 2026-09-30: streaming capture bug found with the first streaming model

- Symptom on the reference machine with Parakeet Unified (streaming): every
  dictation ended within seconds, the app showed "No dictation mode is
  currently available", and the journal recorded
  `capture_measurement_invalid`.
- Cause (`adapters/platform-linux/src/recovery.rs`, stream reader): the
  first-frame time was measured whenever its slot was empty. Sending the
  first chunk empties the slot, so the next sample measured it again and
  the second chunk carried a first-frame time, which the portable slice
  rejects. Any stream longer than one chunk failed. It had never run with a
  real streaming model; the existing tests used one chunk.
- Fix: measure once per capture (`first_frame_seen`). Regression test
  `only_the_first_stream_chunk_carries_the_first_frame_time` (three chunks).
- Verified live: a started session now captures until the endpoint detector
  ends it; in a silent room it records `empty_transcript` with
  `capture_end=endpoint_detected` and the worker stays ready.
- `narrivy-cli pipelines` added (prints the mode listing).
- To watch: the endpoint detector ends a session after a short silence even
  before the user has spoken; owner feedback needed on real speech.

## 2026-09-30: T2.6 and repeated dictations (owner test of the streaming model)

- **T2.6 done.** The daemon passes `endpointing: None`: a dictation ends on
  Stop, cancel or the capture limit, never on a pause. A ready streaming lane
  reports `speech.endpoint=disabled:stop_on_request`. ADR-0039 is amended; the
  VAD and endpointer stay in `narrivy-core`, unused. New core test
  `stream_without_endpointing_keeps_listening_through_silence`.
- **Repeated dictations.** The third dictation in a row failed with
  `capture_backpressure`: the live capture queue held 8 chunks (640 ms), less
  than one slow recognition step. It now holds 64 (5.12 s, 320 KiB). ADR-0037
  is amended.
- The Pipelines noise panel and the feature map say a pause never ends a
  dictation.

## 2026-09-30: overlay follows the app's theme and accent

- The native overlay card takes its colours from the theme and accent chosen
  in Settings (`overlay_theme.rs`, checked against `tokens.css` by a test).
  The app window reports them through the new `overlay_appearance` command.
- New look: an animated activity mark, a listening timer, the newest words of
  live text at full strength, a state-coloured edge glow, a fade-in and an
  eased width change. The card is now click-through.
- Checked from off-screen renders of every theme, accent and state
  (`overlay_preview`, ignored by default). Not yet seen on the real desktop:
  owner to confirm on COSMIC.

## 2026-09-30: Vulkan acceleration in the worker (T2.15 first part, ADR-0076, D25)

- The worker builds with the Vulkan backend (`vulkan` feature). Asked for
  `auto`, it loads the model on the GPU, decodes the known recording there
  as a self-test, and falls back to strict CPU if anything fails. It attests
  the device; status reports `asr.transcribe-cpp.device`.
- A GPU worker that fails is replaced by a CPU worker. A one-shot dictation
  that fails on the GPU is recognised again on the CPU from the same audio.
- Verified on the dev daemon (real hardened launch, sealed worker): status
  reports `igpu` with the Parakeet model; service memory fell from about
  1.1 GB to about 170 MB because the weights live on the GPU.
- Development timings on the reference computer (11 s fixture, release
  build): Whisper large-v3-turbo Q8 10.1 s on CPU, 2.6 s on Vulkan; Parakeet
  1.05 s and 0.30 s; streaming Parakeet's slowest 80 ms feed 877 ms on CPU
  (this is what overflowed the old 640 ms capture queue) and 387 ms on
  Vulkan. Not release evidence.
- D25 recorded: Vulkan in the base install; CUDA and OpenVINO are Store
  add-ons.
- Open in T2.15: stored qualification records, the Settings control, CPU
  replay of a failed stream, the packaged Vulkan build and its evidence.

## 2026-09-30: overlay position (owner request)

- The overlay sits at the bottom centre by default. Settings → Overlay has a
  six-place picker (top or bottom; left, centre or right); the choice is
  kept and sent to the host with the theme and accent.

## 2026-09-30: choose the model in the pipeline (owner request; part of T2.17)

- Owner direction: the Models page is for downloading; the model is chosen,
  with its options, in the pipeline's Transcribe stage.
- IPC minor 14: `SelectModel`. The daemon loads a downloaded catalog model
  in place of the one in use (verified against its pinned SHA-256, new model
  first so a failure keeps the old one), reports `loading` in the listing
  and `model.selection` in status, and remembers the choice for the next
  start. `narrivy-cli use-model ID` does the same.
- The Transcribe panel lists the downloaded models as a chooser and shows
  the loaded model's real facts (device, languages, quantization, size). The
  Models page no longer offers "Use".
- Verified on the dev daemon: Parakeet → Whisper large-v3-turbo in 5.5 s,
  → Whisper tiny.en in 2.5 s, both on the GPU; the choice survives a restart.
- Found and fixed on the way: multilingual Whisper models could not start in
  the worker at all, because they attest about 100 languages and the bound
  was 64 (now 128). Development builds now optimise SHA-256, which was
  taking most of a minute per model.
- **Limits:** one model for all pipelines (not one per pipeline); only in a
  daemon started with an explicit model (the packaged composition still has
  its single bundled model); downloading is still T2.17.

## 2026-09-30: a model and a device per pipeline (owner request)

- Each pipeline keeps its own model and device (Automatic, GPU, CPU). IPC
  minor 15: `SetPipelineEngine`; `ListPipelines` rows carry the choice. The
  NPU is shown as needing an add-on (T2.16).
- One engine is loaded at a time. A dictation reloads it only when the
  pipeline's model or device differs from what is loaded; the overlay says
  a model is loading until listening begins. A failed load keeps the
  previous model and the dictation proceeds.
- The worker accepts `--compute gpu` (a GPU or a failure, no fallback).
- Verified on the dev daemon with start/cancel: Raw (Whisper tiny.en, CPU)
  then Clean (Parakeet, automatic → GPU): the first start of each switched
  (about 2.2 s and 3.4 s extra), the second start of the same pipeline did
  not reload.
- Limit: a switch re-verifies the whole model file each time; caching the
  verified copy would shorten it.

## 2026-09-30: model downloads (T2.17, ADR-0077)

- The Models page downloads a model published in the Narrivy organisation:
  Download, a progress bar, Stop (keeps what arrived), Resume, and the
  reason when one did not finish. The downloaded model then appears in each
  pipeline's model chooser.
- The daemon has no network code: it runs the system `curl` as a revalidated
  root-owned helper with an empty environment, `https` only, Hugging Face
  hosts only (every redirect checked first). The file is accepted only when
  its size and SHA-256 match the pins. IPC minor 16: `DownloadModel`,
  `CancelModelDownload`; status `model.download`.
- Verified for real: Whisper large-v3-turbo (886 MB) downloaded from
  `narrivy/whisper-large-v3-turbo-gguf-q8-0`, stopped at 33 MB, resumed,
  finished in 138 s, SHA-256 matched the pin, file mode 0600.
- The package depends on `curl`.

## 2026-09-30: D26 recorded (one OpenVINO worker, shared language model, writing assistant)

- Owner direction recorded as D26. Design:
  `docs/roadmap/design/writing-assistant.md`.
- T2.16 and T2.20 are extended.
- The writing assistant is after the release, as SQ-WRITE. It is a global
  feature: the language model runs on OpenVINO, Vulkan, CUDA or macOS
  acceleration, with English by default.
- Nothing is implemented.

## 2026-10-01: OpenVINO add-on and the Store (T2.16, T2.19 first slices, ADR-0078)

- **OpenVINO engine:**
  - new `engines/openvino` with `narrivy-openvino-worker`, which loads
    OpenVINO GenAI 2026.4 from a runtime pack through its C API;
  - the daemon never loads OpenVINO;
  - the resident protocol and self-test moved to `narrivy-asr-api`, so both
    workers share them.
- **Runtime pack:** `scripts/build_openvino_runtime_pack.py` assembles 31
  files (292 MB) with a pinned manifest, including Intel's NPU user-space
  driver. Staged in `~/.cache/narrivy/hf-staging/store`. **The owner
  publishes it** with every model pack in one repository, `narrivy/store`
  (see the Store hosting entry below).
- **Store (IPC minor 17):**
  - `ListStore` and `RemoveStoreItem`;
  - `DownloadModel` installs multi-file packs, verified file by file;
  - a Store screen and an NPU device choice in Pipelines.
- **Verified for real:**
  - installed the runtime and Whisper large-v3-turbo INT8 through the
    daemon;
  - the Raw pipeline was set to it on the NPU: the first compile took
    106 s, then it reported `device=npu`;
  - reloads take 3.5 s on the CPU and 5.3 s on the NPU (cached);
  - decode takes 0.45 s per 11 s clip on the NPU against 3.0 s on the CPU.
- **OpenVINO GPU is not yet usable here:** Intel's GPU compute runtime is
  not in the archives.
- **Store hosting (2026-10-01):** one repository, `narrivy/store`, laid out
  as `models/gguf/`, `models/openvino/`, `runtimes/openvino/` and `modules/`.
  - The catalog names each pack's folder (`source_path`); manifests did not
    change, so the pins hold.
  - Staged in `~/.cache/narrivy/hf-staging/store`; `upload.sh` beside it.
  - Published by the owner at revision
    `1fa31d318fe6a7c36d3b71134e26dd9c14c11d0b`, pinned for every model and
    the runtime; the GGUF registry entries moved to `narrivy/store`.
    `verify_published.py` and a manifest check confirmed every file's size
    and SHA-256 at that revision.
  - The five per-model repositories are superseded; deleting them is the
    owner's step.

## 2026-10-01: overlay redesign

- The overlay is drawn natively as one animated card (`linux_overlay.rs`):
  - a compact pill with the logo's waveform at its centre when there is no
    live text;
  - a streaming card that widens first, then grows line by line, with the
    logo centred above the text.
- The waves move while listening; a light runs along them while loading or
  processing. Fades, size changes, scrolling and new words all ease, and
  every animation stops when the desktop turns animations off.
- The surface is a fixed transparent canvas, so growing never resizes the
  layer surface. The previews come from `overlay_preview` (ignored test),
  which also writes the opening animation frame by frame.

## Remaining in WP1 / WP1b (Phase A)

- **T1.2 blocker re-verification:** run each open blocker's unblock check
  (only the counts and scope notes were updated so far).
- **T1.4 step 7 (owner present):** launch the Tauri app on COSMIC. Check that
  the UI loads under the CSP, events arrive, dictation starts and stops, the
  overlay never takes focus and appears on the right monitor, and the app
  closes cleanly.
- **T-OSS.4 dead code:** the legacy router (F-L2/F-M15), `transcribe-sys`
  FFI (F-L1), and the probe-only audio code per D14. These belong with the
  WP2 product fixes because they touch production code.
- **T-OSS.2 content merges:** the architecture overviews and a user-facing
  install/getting-started guide. Phase B, once the product works.
- **T4.7 content-truth items** inside individual docs (NAR-* registry, doctor
  fields, and so on).

## 2026-10-01: WP2c F1, clipboard paste (first slice)

- **Clipboard access:** the clipboard and primary selection through the
  Wayland data-control protocol, with no helper from PATH
  (`wayland_selection.rs`). Snapshots capture every format and are bounded;
  the user's values are restored only where Narrivy still owns the
  selection, after the target has read ours.
- **Paste chord:** Shift+Insert through the trusted wtype
  (`SystemWtypeChord`).
- **Ladder:** the rung runs ahead of typing in the routed ladder when the
  clipboard is authorized. A clipboard that cannot be captured is never
  replaced.
- **Dev daemon:** now uses the routed ladder with clipboard authorization.
- **Checks:**
  - adapter tests pass, plus the round trip on the real COSMIC clipboard;
  - daemon tests and clippy pass;
  - `cargo deny` passes.
- **Remaining for F1:**
  - a live dictation into a GUI app and a terminal;
  - telling terminals apart (Ctrl+Shift+V), if Shift+Insert falls short
    anywhere;
  - waiting for the shortcut's keys to be released;
  - the overlay saying "Pasted" or "Copied";
  - a user setting in place of the environment flag.

## 2026-10-02: F1 live check, T2.1, and the delivery result in the overlay

- **Owner live check:** dictation pasted into a GUI app (the Claude Code
  message box) and into a terminal on COSMIC.
- **T2.1 (F-C2, Critical) done:** at most one rung dispatches per
  transaction. An unverified paste or typing, a copy, or a failure after
  dispatch ends the ladder; a failure before dispatch (`NotDispatched`)
  continues. Before this, a paste could be followed by typing the same text.
  Tests: `routed_ladder_stops_after_unverified_submission`,
  `routed_ladder_stops_after_dispatch_failure`,
  `routed_ladder_continues_after_a_failure_before_dispatch`. ADR-0072
  amended.
- **Delivery result (F1 step 5):** the `committed` event carries `inserted`,
  `pasted`, `typed`, `copied` or `inbox` (protocol minor 18). The overlay
  shows "Pasted"/"Typed" briefly, and "Copied — paste with Ctrl+V" or
  "Not inserted — saved to Inbox" for longer.
- **Checks:** adapter, daemon (all features), IPC, desktop and front-end
  tests; clippy on the workspace.
- **Remaining for F1:**
  - the clipboard paste switch as a user setting (with T2.4, which moves the
    installed unit to the routed lane);
  - telling terminals apart, only if Shift+Insert falls short somewhere;
  - waiting for the shortcut's keys to be released.

## 2026-10-02: clean-up on the NPU, the invisible wait, and the assistant (D31, D33)

- **Clean-up model:** S1-mini by Superwhisper (Qwen3-0.6B fine-tuned only to
  clean dictation, Apache-2.0 with a naming term), converted with
  `scripts/build_llm_pack.py` to OpenVINO INT8 per channel. On the Lunar Lake
  NPU: about 250-470 ms per sentence; it cleaned question- and
  instruction-shaped dictations instead of answering them. INT4 repeated
  itself on one such dictation; FP16 matched INT8 at twice the size.
- **Daemon:** `narrivy-openvino-llm-worker` (new) loads in the background;
  in the Clean pipeline each dictation is cleaned within 8 s and kept only
  if the deterministic faithfulness check passes, else the rule-based text
  is delivered. Fix found on the way: a worker spawned from a short-lived
  thread is killed by its parent-death signal; owners now stay alive.
- **Overlay:** "Refining…" with the recognised text, then the cleaned text
  in place with the changed words marked, kept with "Pasted" until it fades.
- **Assistant extension:** Qwen3.5-4B (OpenVINO INT4) on the CPU, streamed
  to a new Assistant screen; IPC minor 19. Load about 8 s, about 8 tokens/s.
  The GPU needs Intel's compute runtime 24.x for Lunar Lake (the archive has
  23.43); Qwen3.5 did not compile for the NPU within 30 minutes.
- **Research reviewed (owner-supplied):** edit-tagging models (GECToR,
  Typurr, Quire) cannot generate an answer at all; they are added to the F30
  benchmark as a second clean-up engine.
- **Remaining:** owner live test of clean-up; Models → Language models
  section (D32) instead of the dev environment variables; idle unload
  verified; assistant tools and web research (F24, F25).

## 2026-10-02 to 2026-10-18: v1 features (WP2c), summary

This log was not kept per task in this stretch (about 100 commits). The
commit messages are the detailed record; this is the summary.

- **Clean-up and language models:** S1-mini clean-up on the NPU, Narrivy's
  own fixed-shape NPU runner (Qwen3 and Qwen3.5), Models → Language models
  (D32), idle unload.
- **Assistant (D33–D35):** Qwen3.5-4B chat with a library kept by the
  service; voice mode (English and Urdu voices); MCP tools with approval;
  web research (Brave, SearXNG, DuckDuckGo/Wikipedia, public addresses
  only); computer tools with a working folder; screen share and screen
  recording kept as sessions; `view_page` in headless Chromium (optional
  Store pack, Firefox fallback); `append_file` and an 8192-token reply
  limit; per-chat thinking level; an Activity panel showing stages, the
  call being written, and each tool call and result as JSON.
- **Meetings (F22):** record, tell speakers apart, live transcript (pieces
  of at most 8 s, the current piece every 1.5 s, protocol minor 53),
  subtitles, notes, import of recordings, a quick raw save on stop with
  processing on request (minor 52), mic and computer sound switched
  separately.
- **Dictation polish:** voice filter (Silero VAD) and "Only my voice"
  (CAM++, Relaxed/Normal/Strict), echo cancellation, noise suppression,
  vocabulary, learn from corrections, press-and-hold, cancel shortcut,
  start/stop sounds, History retention (T2.8), Paste or Type, numbers as
  digits, spoken line breaks, start at login.
- **Status bar:** CPU/GPU/NPU ten-minute graphs. GPU use is DRM fdinfo
  engine cycles over all apps, with Narrivy's share (2ddba1c).
- **Browser writing assistant** (Brave extension and native host; SQ-WRITE
  scope, D26).

## 2026-10-18 to 2026-10-19: live problems found by the owner

- **Paste broken by COSMIC** (cosmic-comp updates of 15 and 17 Oct):
  modifier keys sent through the Wayland virtual keyboard (`wtype -M`, and
  `-P Shift_L`) are ignored, so Shift+Insert and Ctrl+V never paste. Plain
  typing still works. The paste rung still reports `SubmittedUnverified`,
  because the clipboard manager reads the clipboard at once, which looks
  like a paste. **Fixed for the owner:** the opt-in `paste-keyboard`
  method (uinput Shift+Insert, 0ddf1fc); it replaced the Type workaround. A uinput keyboard (`ydotool key
  ctrl+a`, tested) does pass Ctrl; moving the paste chord to uinput is
  proposed, not built.
- **First words dropped:** "Only my voice" on Strict (0.42) rejected the
  owner's own speech (scores 0.35–0.43 on the Samson mic; profile from
  2026-10-06). Set to Normal (0.30); retraining the voice is advised. The
  echo canceller was ruled out (measured).
- **Overlay words lag:** the live preview ran every 1.5 s; it now decodes
  as soon as the last decode ends (482375c).

## 2026-10-19: release gate re-measured (at 482375c, plus status edits)

`python3 scripts/release_gate.py --target linux-1.0`: **exit 1, 614 lines,
0 of 93 release requirements accepted.**

- Requirement status in `spec/requirements-traceability.csv`: 39 passed
  and 54 blocked.
  - Blocked by area: FUN 17, PERF 9, REL 6, UX 6, TEST 5, OPS 4, PRIV 3,
    ACC 2, PORT 2.
- Kinds of error:
  - 140 assertions with no measurement binding (14 requirements);
  - 56 artifact hash mismatches (33 requirements; the code moved under
    the evidence);
  - 54 requirements blocked;
  - 39 passed rows with evidence from an older commit;
  - 5 missing or unsafe artifact paths.
- Manifest: 102 unlisted files and 101 changed files (all the new
  features); `scripts/build_manifest.py` is a reviewed packaging step.
- The P11 phase report is not passing and names an older commit.

## Where to start next (handoff, updated 2026-10-27)

Read this section first; the dated entries at the end of this file have the
detail. Rules learned the hard way are in the owner's memory notes and in
`~/.claude/CLAUDE.md` (git identity), summarised here.

**Working rules (owner):**
- One question per message, short replies. Commands for the owner each in
  their own ```bash block with FULL absolute paths; `sudo` goes in the
  Terminal tab (the chat's bash box cannot sudo).
- **Before asking the owner to test**, check what is actually running and
  that it has the fix: `readlink /proc/$(pgrep -x narrivy-desktop)/exe`
  (`/usr/bin/...` = installed package, opened from the menu),
  `dpkg-query -W narrivy`, `narrivy-cli status` (`daemon_build`), and the
  package's commit in `artifacts/candidates/<version>/provenance.json`
  (`source_commit`). App fixes reach the owner only through a new package.
- Never commit while `scripts/build_release_candidate.sh` runs (it refuses
  a moved HEAD at the end).
- Never `kill` a negative pid without `--` (ac5fbda: `kill -KILL -<pid>`
  killed every process of the user and logged the owner out twice).
- Git: everything is committed and pushed as Waqar Gulzar
  <96654715+iamwaqargulzar@users.noreply.github.com> (global config; no
  per-repo identity, no agent names, no Co-authored-by). GitHub `main` is
  `4dcedc1` (the squashed public commit, re-authored and force-pushed
  2026-10-23). Push only when the owner says so, via the `publish` branch.
- Host builds: cargo `-j 2`–`4`, one heavy build at a time (memory).

**Owner's machine now:**
- Installed package: **rc11** (`1.0.0~rc11`, commit `ae64891`, installed by
  the owner 2026-10-26). **rc12 is built (2026-10-27, commit `98a22ea`,
  sha256 `b628524b…727b`) but not installed yet**: the owner installs it,
  then continue with the checks in item 1 below.
- The owner's account runs the **development service** (user unit
  `~/.config/systemd/user/narrivy-daemon.service` → `scripts/dev-daemon.sh`,
  rebuilds from this checkout on restart; it reports `daemon_build=1.0.0-rc.11`).
  Drop-in `narrivy-daemon.service.d/openvino-runtime.conf` points it at
  `~/.cache/narrivy/dev-runtimes/dev/openvino-runtime-2026.4.0.npu1.38-linux-x86_64`
  (OpenVINO 2026.4.0 + Intel NPU user-space driver 1.38). Speech: Whisper
  large-v3-turbo int8 on the **NPU** (status shows `igpu` for a few seconds
  after a restart, then `npu`). The package itself is the slim 1.0 build
  (tiny.en on CPU through transcribe.cpp, no NPU/assistant).
- System: Intel NPU packages 1.38.0 installed (intel-fw-npu,
  intel-level-zero-npu, intel-driver-compiler-npu); NPU40xx firmware
  "Aug 20 2026" loads. **Whisper on this NPU hangs with OpenVINO 2026.4.1
  and the 2026.5 nightly** (any compiler/driver); stay on 2026.4.0 for the
  NPU. Upstream report is the owner's call.

**Done since rc11, waiting for rc12 (all committed, nothing pushed):**
- Side panels like the Assistant's chat list on Settings (32d09b6), Store
  (9c1c151: sections with counts, search, trust note and Models link at the
  foot), Models and Meetings (e84d897).
- Models sections: Speech to text, Correction (the clean-up model), Language
  models (the assistant), Text to speech (voice mode's Kokoro + Piper Urdu
  voices, read from `assistantLibrary('get','voice')`). Pointers to the
  clean-up model now say "Models → Correction".
- The Dictation page is gone (8598014): everything on it was on Home,
  Pipelines or Settings. The start command and tray note are the first card
  in Settings → Shortcuts.
- Meetings: New meeting / search / list in the panel; a compact recorder
  card (32d8cf4): name + Record, then chips for microphone and call audio,
  pill switches (Screen, Go over on stop, Auto notes) and "Transcribe a
  file…", which uses a new `pick_recording` command (portal file chooser
  for audio/video) in place of the path box. Not yet tried in the real app.
- Tray "Start dictation in…" submenu (c7cdae2), one entry per distinct
  language choice in the daemon's language shortcuts ("Urdu · Clean",
  "Urdu → English · Raw"); starts that pipeline with those languages. Read
  at start, every minute, and after a shortcut is set/removed through the
  app. Unit-tested only; not seen in the real tray yet. Language shortcuts
  themselves already existed: Pipelines → a pipeline → Transcribe →
  Language shortcuts (the owner wants Urdu spoken and pasted).

**Next, in order:**
0. **Dependency refresh: done 2026-10-26** (entry at the end). Left: the
   owner's call on the new `cargo deny` advisories (B-SUPPLY-010 note), and
   the portable MSRV job, which fails on our own let-chains (needs 1.88).
1. **Build rc12.** Bump `1.0.0-rc.11` → `1.0.0-rc.12` in
   `services/narrivyd/Cargo.toml`, `apps/narrivy-cli/Cargo.toml`,
   `apps/narrivy-desktop/src-tauri/Cargo.toml`,
   `apps/narrivy-desktop/src-tauri/tauri.conf.json` (+ `Cargo.lock` via
   `cargo check`), commit like 1a56ec8, then run
   `CARGO_BUILD_JOBS=4 scripts/build_release_candidate.sh 1.0.0~rc12` in the
   background (no commits while it runs). Owner installs with
   `sudo apt install /home/waqar/Documents/projects/mine/narrivy/artifacts/releases/narrivy_1.0.0~rc12_amd64.deb`.
   Then verify what runs (rules above) and have the owner check one item at
   a time: the side panels (Settings, Store, Models, Meetings); no Dictation
   page; Settings → Shortcuts start card; Meetings recorder and "Transcribe
   a file…" chooser; an Urdu language shortcut, then the tray's "Start
   dictation in…" entry pastes Urdu. Still unchecked from rc11: Saved
   sessions per chat, live share pictures, Pipelines Import/Export and
   title-row shortcuts, Home without Services, dictation types.
2. **Owner reports open:** a wrong transcription ("install the latest
   version" → "install early decision"; tonight "release candidate 10" →
   "ReleaseCandondent10") and dictation latency on the dev service (p50
   ~11 s, p95 ~43 s, measured 2026-10-22, before the NPU driver update) —
   re-measure with `narrivy-cli status` latency after some dictation.
3. A screen share left running froze when the portal crashed; if the owner
   sees COSMIC's portal crash again, check the capture restarted
   (`journalctl --user | grep "wayland thread crash"`).
4. **Linux smoke test, round 2** on the test user `narrivytest` (paused at
   C2 on 2026-10-21): steps file and remarks file are on its Desktop (copy
   new versions with `sudo install -o narrivytest -g narrivytest -m 644 …`).
   Left: C2 log, C3 special characters via a vocabulary replacement
   ("test phrase" → "café — it's done"), C4–C6 remove and reinstall
   (F-H15), D optional USB unplug (F-H2). Use the newest package.
5. **Windows (D36), branch `windows/port`** (checkout
   `/home/waqar/Documents/projects/mine/narrivy-windows`, head `630a034`;
   nothing pushed). Merge cleanup/wp1b into it before more Windows work.
   Left: the desktop app connected to the Windows service, overlay and
   tray, microphone choice, clean Ctrl+C stop, packaging (owner decides).
   VM: `~/VMs/narrivy-win11` (`run-vm.sh`, `vm-ssh.sh`, `sync-to-vm.sh`,
   `vmctl.py`; password in `vm-password.txt`). transcribe.cpp 0.4.1 has a
   Windows build fix. Notes: `engines/transcribe-cpp/BUILD-NOTES.md`
   ("Windows") and `~/.config/Claude/side-session-notes/local_09dee21b-0a43-47f4-97cd-5c0196969abb/`.
6. **Older open items:** F-M18 (SIGKILL survival test), F-M16 builder
   success-path test, F-M3/F-M4, F-M7 (D4), F-M20 onboarding, F-M12 app-ID
   check (live), the manifest regeneration (`verify_manifest.py`), F-C7
   probe-only audio code; then the 1.0 release-gate campaign
   (`python3 scripts/release_gate.py --target linux-1.0`).

**Cleanup the owner may do:** the session worktrees under
`.claude/worktrees/` (windows-*, win-*) are all merged into `windows/port`
and can be removed with their sessions.

## 2026-10-20: review findings re-checked against today's code

Each finding in `00-review-findings.md` was checked against the code at
`5f63f39`. "Fixed tonight" items have a commit and a test.

**Fixed tonight**
- **F-H7** (4b2c651): the compute probe read `ldconfig -p` (96 KB here,
  more than a pipe holds) only after the child exited, so it blocked until
  the deadline before the daemon served. Output is now read while the
  child runs. The dev daemon answered 1.7 s after a restart.
- **F-H16** (844d886): daemon tests composed the real clipboard-paste and
  wtype rungs, so `cargo test` from a desktop terminal could write the
  clipboard and send keys to the focused window. Under `cfg(test)` those
  rungs are unavailable stand-ins with the same ids. (These tests ran once
  on 2026-10-19 before the fix; they took 0.5 s then, 0.05 s now.)
- **F-C6** (49cc533): only the sidecar engine requires a model path, so
  installed mode no longer fails every dictation with `model_unavailable`.
  **Not yet proven:** needs an installed-mode dictation (WP5 native).
- **F-H17** (2299cd4): the count cap deletes in one statement (no
  clear-and-reinsert), `secure_delete` is on, the WAL is truncated after
  deletes.
- **F-M10** (c373091): the live status poll is single-flight.
- **F-M17** (a01aeca): invalid environment values are reported by name only.
- **F-M1, F-M2** (5f63f39): the VAD refuses multi-channel audio and checks
  a chunk's size before changing any state.
- **F-H8:** the typing helper (`wtype`) now always gets `LC_ALL=C.UTF-8`.
  Reproduced first with a C program on this machine (glibc 2.39):
  `mbstowcs("café — it’s")` is -1 in the C locale and 11 with C.UTF-8.
  Not yet checked by typing such a phrase natively.
- **F-M16, one item** (49cc533): the support-report tests waited on
  `committed` and then raced the History write (failed 1 in 5); they now
  wait for the row (15 of 15 pass).

**Already fixed before tonight (code confirms)**
- F-C2 (one dispatch per transaction), F-H3 (no silence endpointing in
  dictation), F-H10, F-H11 (layer-shell overlay, `KeyboardMode::None`),
  F-H12 (event numbering keeps rising across reconnects, plus a
  resubscribe marker), F-H14 (no status pill any more; the unused
  `usePerfMarks` hook still says `recording`), F-M19 (History "Keep for"
  and "Clear").
- F-H1 partly: the queue is 64 chunks (5.1 s), not the 10 s suggested.
- F-C8 settled by use: typing works on COSMIC; the modifier problem since
  15 Oct is handled by the opt-in `paste-keyboard` method.

**Still open (confirmed in today's code)**
- **F-C1:** all three unit files still set `NARRIVY_RECOVERY_MODE=
  linux-mic-inbox` and `NARRIVY_DELIVERY_BACKEND=inbox`: the package only
  saves to the Inbox. (They are still byte-identical.)
- **F-C4:** `wtype` is in neither `Depends` nor `Recommends`.
- **F-C5:** `model_import.rs` still requires the source file's owner to be
  the daemon's user, and the package still ships a root-owned
  `/usr/share/narrivy/models/whisper-tiny.en-Q8_0.gguf`, which the desktop
  still offers (`bridge.rs:134`).
- **F-H2:** a microphone lost mid-dictation still discards the dictation
  (`capture_source_ended`), although D6 asked to keep it.
- **F-H4:** `narrivy-cli` does not start the daemon or notify when it is
  not running.
- **F-H5:** the units still use `WantedBy=default.target` without
  `PartOf=graphical-session.target`.
- **F-H6:** the packaged unit has still not been run natively (the owner
  runs `scripts/dev-daemon.sh`).
- **F-H15:** `prerm` still does nothing on remove/upgrade.
- **F-H18:** no `PR_SET_DUMPABLE`, no `LimitCORE=0`. Left for the owner:
  `PR_SET_DUMPABLE=0` also makes `/proc/<pid>` of the daemon root-owned,
  which the native checks above read.
- **F-M3:** Inbox files are still on tmpfs; undelivered text is kept
  durably in History (D6) unless History keeps 0.
- **F-M6:** `Build-Using`, `narrivy.local` homepage and maintainer remain.
- **F-M12:** the CLI is still version 0.0.0.
- **F-M13:** `DEFAULT_PIPELINE_ID` is still `mock-dictation`.
- **F-M15 / F-L2:** `NARRIVY_DELIVERY_BACKEND=platform` still reaches the
  legacy router (`session.rs`, `PlatformRouter`); earlier notes listed
  F-M15 as fixed, which is not so.
- **F-C7:** D14 (C) answered; the microphone picker exists
  (`narrivy-cli mic`), but the probe-only audio code
  (`audio_policy.rs` and the rest) is still in the tree.
- **F-L1:** `engines/transcribe-cpp/transcribe-sys/` still exists.
- **F-L3:** `session.rs` is now 11k lines (post-1.0).

**Not re-checked** (UI or native behaviour this pass did not cover): F-M4
disclosure, F-M7 (D4), F-M8 on the Tauri overlay, F-M9 (GTK shell
retired; the Tauri event bridge not checked for the same issue), F-M11,
F-M14, F-M18, F-M20, the remaining F-M16 items, and the Low list.

## 2026-10-20: installed .deb path (F-C1, F-C4, F-C5, F-H5, F-H15)

All on `cleanup/wp1b`, one local commit. Checked with the packaging tests
(117 pass), `cargo test -p narrivy-platform-linux` (309 pass), the new
`narrivyd` policy test (with `asr-transcribe-resident`) and clippy on both
crates. **None of it has run from an installed package yet** (WP5 native).

- **F-C1:** the three unit files (still byte-identical), the canonical copy
  in `build_deb_candidate.py` and the qualifier's mirrored environment now
  select `linux-mic-routed-inbox` / `routed-inbox`. Startup validation
  accepts that lane with the installed runtime; a `policy.rs` test reads the
  shipped unit and checks it. The routed lane does not require `wtype` to
  be ready; without it, text goes to the Inbox.
  **Owner decision still open:** the package ships typing only. The unit
  keeps `UnsetEnvironment=NARRIVY_CLIPBOARD_AUTHORIZED`, so clipboard paste
  (and `paste-keyboard`) is off in the package, unlike `dev-daemon.sh`.
  Turning it on means `Environment=NARRIVY_CLIPBOARD_AUTHORIZED=1` in all
  copies; better as a user setting (T2.4).
- **F-C4:** `wtype` is in `Recommends` (`build_deb_installable.py` and both
  `control` templates), with a test. The Diagnostics/Home text for
  `wtype_missing` is not done.
- **F-C5:** `model_import.rs` has a packaged-source rule for exactly
  `/usr/share/narrivy/models/whisper-tiny.en-Q8_0.gguf` imported as
  `whisper-tiny.en-Q8_0`. Each directory from `/` down is opened through the
  one before it (no symlinks) and must be root-owned and not group- or
  other-writable; the file must be root-owned, mode exactly 0644, and the
  registry size; the digest is checked during the copy as before. Every
  other path keeps the same-user rule. Tests inject the trusted owner and
  base: a full import, and rejection of a wrong owner, wrong size, file
  modes other than 0644, a writable ancestor and symlinks.
- **F-H5:** the units use `PartOf=` and `WantedBy=graphical-session.target`.
  Nothing enables the unit for all users (D5).
- **F-H15:** `prerm` (remove, deconfigure, upgrade) stops
  `narrivy-daemon.service` in each running user manager
  (`systemctl --user -M <uid>@`, 10 s each, never fails the package step).
  `postinst configure` reloads each user manager and runs `try-restart`;
  `postrm remove` reloads. The call to the nonexistent `narrivy-migrate` is
  gone. The purge message now includes `~/.local/state/narrivy`. The
  `-M <uid>@` form was checked as the user on this machine, not as root
  under dpkg.
- **Not done:** `scripts/verify_manifest.py` was already failing on many
  files before this change, and these files are now among them. It was not
  regenerated: per the script, that is a separately reviewed packaging step.

## 2026-10-20: paste as a user setting (D37)

- The unit stays typing-only (`UnsetEnvironment=NARRIVY_CLIPBOARD_AUTHORIZED`).
  The per-dictation ladder already took the user's Pipelines → Deliver
  method as the authorization (`text_options::paste`); the status tokens
  did not, so an installed daemon reported `clipboard_not_authorized` while
  it pasted. GetStatus now renders the clipboard tokens from the same
  effective authorization.
- `paste_for` is a pure rule with a test (package default types; `paste` and
  `paste-keyboard` authorize; `type` withdraws a composed authorization).
- ADR-0072 amendment and `spec/protocols/ipc.md` updated. Without
  `/dev/uinput` access, `paste-keyboard`'s rung is unqualified and the ladder
  types.
- Also fixed: an unused-variable lint on the default-feature `narrivyd`
  build (`job_uinput_paste`).
- Checked: `cargo clippy -p narrivyd --all-targets -D warnings` (default and
  `asr-transcribe-resident`), `cargo test -p narrivyd --features
  asr-transcribe-resident` (all pass). Not yet run from an installed package.

## 2026-10-20: crash dumps blocked (F-H18, D37)

- `LimitCORE=0` in the three unit copies (still byte-identical) and the
  canonical copy in `build_deb_candidate.py`; the packaging test requires
  it and rejects a unit without it.
- `narrivy_process_linux::forbid_core_dumps()` (rustix, no new `unsafe`)
  runs first in `narrivyd` (exit 1 if it fails) and in
  `narrivy-transcribe-worker`; both OpenVINO workers make the same rustix
  call. Each worker sets it itself because `execve` makes an ordinary binary
  dumpable again. `pw-record` and `wtype` get only `LimitCORE=0`.
- Test: a child copy of the test binary turns itself non-dumpable; the
  parent (same user) gets `EACCES` on its `/proc/<pid>/environ` while
  `status` (what the resource sampler reads) stays readable.
- No client or script reads the daemon's `/proc/<pid>` (only `/proc/self`).
- Docs: `docs/security/data-retention.md` (new "Process memory" section;
  `docs/user/privacy.md` named in D37 does not exist), a line in the threat
  model, and the wording in ADR-0035/0036/0037 ("core dumps: contained;
  swap: documented residual").
- Checked: clippy `-D warnings` on narrivyd (both feature sets),
  process-linux, transcribe-cpp (`resident-worker`) and openvino
  (`worker`); their tests and the packaging tests pass. Not yet run from an
  installed package, and the dev daemon was not restarted.

## 2026-10-20: package identity (F-M6, D3)

- `build_deb_installable.py` `_control`: `Maintainer: Waqar Gulzar
  <iamwaqargulzar@gmail.com>`, `Homepage:
  https://github.com/iamwaqargulzar/narrivy`. Both `control` templates match,
  without `Build-Using`, at `Version: 1.0.0~rc1`.
- `--version` now accepts only D3 versions: `X.Y.Z` or `X.Y.Z~rcN`. The old
  `+g<hash>` suffix (rc57) is refused; the source commit is in the build
  info already. Test: `test_package_identity_is_d3`.
- Left: `narrivy.local` URIs in `build_candidate_supply_chain.py` (SPDX
  namespace, provenance `predicateType`) are identifiers, not package
  fields; change them with the supply-chain evidence, not here. The product
  versions still disagree (F-M12, CLI 0.0.0; T3.2 single-sourcing).
- Checked: `pytest tests/packaging` (118 pass).

## 2026-10-20: release build (T3.1) and the first rc1

- `scripts/build_release_candidate.sh VERSION` built `1.0.0~rc1`. Fields
  checked with `dpkg-deb`: D3 maintainer and homepage, `wtype` in
  Recommends, no `Build-Using`, sorts above 0.2.3; the unit has the routed
  lane, no clipboard authorization and `LimitCORE=0`.
- Fixes found by building: `umask 022` (cargo's 0775 binaries were refused);
  `CONST_RANDOM_SEED` from the commit (`const-random` through
  burn-ndarray/Harper baked a random 256-bit constant into `narrivyd`); the
  fixed-path package moves to `previous` only when the version changes.
  The second fix came too late for the 2026-09-22 package at the previous
  path: it was overwritten (it was stale; no copy kept).
- **Reproducible:** two fresh builds of `aba3de0` both give
  `094852ede040322e3b7b6c2d8d63cb7cfa876e484d079d968223808c65d61978`.
- The owner installed the first rc1 (`775f74cc…`, built at `653f425`, same
  code) with apt. On the owner's account the dev unit
  (`~/.config/systemd/user/narrivy-daemon.service`, from
  `install-dev-service.sh`) overrides the packaged one, so the smoke test
  runs on a clean user `narrivytest` (script and steps put on its Desktop;
  results in `/tmp/narrivy-smoke-narrivytest-<step>.log`).

## 2026-10-20: first native run of the packaged unit (F-H6)

- On the clean user `narrivytest`, rc1's daemon exited in 40 ms on every
  start: `installed resident runtime failed verification`
  (`ManifestUntrusted`). Cause: `PrivateTmp`, `ProtectSystem=strict` and
  `ProtectHome=read-only` in a *user* unit run the service in a user
  namespace where root is unmapped, so `/usr` and every packaged file look
  owned by 65534 and each root-ownership check fails. Reproduced on the
  owner's account with `systemd-run --user -p ...` (each of the three alone
  breaks it).
- Owner decision: drop the mount sandbox. `CapabilityBoundingSet=` went too:
  outside the namespace it fails exec with status 218 (and an ordinary user
  holds no capabilities). Kept: `LimitCORE=0`, `NoNewPrivileges`,
  `UMask=0077`, `RestrictAddressFamilies=AF_UNIX`, non-dumpable daemon.
  A packaging test rejects the namespace directives.
- Checked: the installed `/usr/bin/narrivyd` under exactly the new settings
  (private runtime/state dirs) serves, verifies the worker
  (`ready:installed-runtime-manifest-bound`), trusts the system `wtype` and
  PipeWire helpers, and refuses `/proc/<pid>/environ`.
- Also seen: while the daemon failed, something requested a start every
  3 s (`Start request repeated too quickly`), probably the desktop app;
  `RestartPreventExitStatus=1` stops systemd itself from restarting. To
  check with the owner's screenshots.
- Owner's screenshots (rc1, test user): onboarding and Models sat on
  "Connecting…"/"Loading…" with no error, and the app showed v0.2.3. Cause
  of the silence: the first status read waits out the 3 s on-demand start,
  the 3 s quiet poll supersedes it, and quiet failures were hidden; each
  read also asked systemd to start the unit again. Fixed: the desktop host
  starts the unit at most once per 30 s and answers `daemon_failed` when
  systemd reports it failed; a quiet failure before any status is shown;
  the top bar says "Service failed" and the bottom bar gives the journalctl
  hint. The app version is `1.0.0-rc.2` and the release script refuses a
  `tauri.conf.json` version that is not the package version.

## 2026-10-20 (night): F-H4, the shortcut starts the daemon

- `narrivy-cli start|toggle` without `--socket`: when the daemon is not
  running it runs `/usr/bin/systemctl --user start narrivy-daemon.service`
  once (bounded), waits up to 10 s for the socket, then sends the one
  request. If the start or the wait fails it shows one content-free desktop
  notification ("Narrivy could not start", through `/usr/bin/gdbus` and
  `org.freedesktop.Notifications`; checked live) and exits 1. Other
  commands never start the daemon. `libglib2.0-bin` (gdbus) is in
  Recommends.
- Tests (`start_tests`): running daemon never started; stopped daemon
  started once and the retry connects; a daemon that never answers notifies
  once; a failed start notifies; other IPC errors start nothing.
- Not yet in a package: needs rc3 and the owner's native check (smoke step 4).

## 2026-10-20 (night): F-M13, no silent mock

- `narrivyd` refuses to start (exit 1) when `NARRIVY_ASR_ENGINE` is unset:
  a hand-started daemon no longer serves the mock engine's fake
  transcripts; the mock needs `NARRIVY_ASR_ENGINE=mock`. The unit,
  `dev-daemon.sh` and the qualification scripts already set
  `transcribe-cpp`; the one test that starts the binary passes `mock`. Test
  `a_daemon_without_an_engine_choice_refuses_to_start`.
  `EngineConfig::default()` (library tests) is unchanged.
- Fixed a clock-dependent test: `routines_are_due_once_per_slot` failed
  every evening after 20:00 (its 08:00 slot was over the 12 h window).

## 2026-10-20 (night): F-M12, one version

- `narrivyd`, `narrivy-cli` and the desktop crate are `1.0.0-rc.2`, like
  `tauri.conf.json` (daemon status and support reports showed 0.0.0). The
  release script checks all four against the package version and names
  each one that differs; test updated.
- Not changed: the app ID / `StartupWMClass` part of F-M12. The owner's
  rc1 screenshots show the Narrivy icon in the COSMIC dock with no generic
  duplicate; check the window's app ID live before changing anything.

## 2026-10-20 (night): F-H2, a lost microphone keeps what was said (D6)

- Core (`recovery.rs`, batch and streaming): when the capture source ends
  mid-dictation after at least 0.5 s (`SOURCE_ENDED_MIN_SAMPLES`), the audio
  is recognized (the stream is finished like a Stop), corrected and
  journaled, and the transaction ends `FailedRecoverable` with
  `capture_source_ended`. Nothing is delivered (the user may have left the
  window) and no delivery intent is written, so startup replay never sends
  it. Under 0.5 s it fails terminal as before (the two existing tests now
  pin that case).
- History keeps that one recoverable failure as an undelivered entry (Copy);
  other recoverable failures are unchanged.
- The Dictation notice now reads "The microphone disconnected before you
  stopped. What you had said is saved in History, not typed into the app".
- Tests: `batch_source_ended_after_enough_audio_keeps_the_text_undelivered`,
  `controlled_streaming_source_ended_keeps_the_text_undelivered`,
  `a_dictation_whose_microphone_disappeared_is_kept_undelivered`, the
  rendering test.
- Also: `committed_dictation_is_retained_and_served_over_the_socket` raced
  the History write after `committed` (failed about 1 in 20); it now waits
  for the row. Once tonight, a parallel run of that test binary (which also
  holds every daemon unit test) hung after all tests finished, with daemon
  threads blocked on a pipe read; 15 later parallel runs and 3 serial runs
  passed. Not chased further.
- Not yet checked natively (unplug a USB mic mid-dictation).

## 2026-10-20 (night): F-C4, the missing-wtype message

- When the daemon reports `delivery.wtype-helper=unavailable:wtype_missing`,
  the delivery card, Settings and the Home detail say "wtype is not
  installed (sudo apt install wtype)" instead of "typing helper
  unavailable"; other reasons keep the generic text. The Home chip still
  reads "Saves to Inbox" (its text match was widened; without that it would
  have claimed "Types text"). Test in `ResolvedCardTruth`.

## 2026-10-20 (night): F-M14, only a resident daemon is packaged

- `narrivyd --build-features` prints its compiled features and exits
  (after the non-dumpable call, before anything else). The builder runs it
  with an empty environment and a 5 s bound just before copying the
  binaries and refuses a daemon without `asr-transcribe-resident`. Tests:
  resident accepted; plain, sidecar-only, failing, hanging and missing
  binaries refused. The real debug resident daemon is accepted.
- F-C5 dry run (2026-10-20 night): the installed rc1 `narrivyd` under the
  new unit settings (private runtime/state/cache dirs, `systemd-run --user`)
  imported `/usr/share/narrivy/models/whisper-tiny.en-Q8_0.gguf` through
  `StartOfflineModelProvision` (Importing to Ready) and its resident worker
  became ready (Vulkan). Not yet done by a user through the app.

## 2026-10-20 (night): F-M15 and F-L2, the legacy router is gone from the daemon

- `NARRIVY_DELIVERY_BACKEND=platform` is now an invalid value; the
  `PlatformRouter` backend, its session arm (which reported every
  non-verified outcome as a failure) and its `platform.delivery.router`
  token are removed from `narrivyd`. The desktop still maps that token,
  harmlessly; left as is.
- `adapters/platform-linux/src/delivery/router.rs` (PATH-resolved helpers,
  rejected by ADR-0072) is `#[cfg(test)]` only, so nothing can wire it back.
- `cargo clippy --workspace --all-targets -D warnings` was failing on
  `adapters/ipc-unix/examples/assistant_probe.rs` (`expect` in a hand-run
  probe); it allows `expect_used` now and the workspace lint passes.
- Checked: narrivyd tests (resident), platform-linux tests (309), workspace
  clippy.
- rc3 (`b0fc59c6…`, built at `395a699`) predates these three changes; they
  touch no installed behaviour.

## 2026-10-20 (night): F-L1, transcribe-sys deleted

- `engines/transcribe-cpp/transcribe-sys/` (an uncompiled FFI stub with
  wrong declarations; nothing built it) is deleted; BUILD-NOTES and the two
  crate docs say so. Governance tests: 182 pass; the one failure is the
  known `verify_manifest.py` drift (unlisted scripts, now also
  `build_release_candidate.sh`), which waits for the reviewed manifest
  regeneration.

## 2026-10-20 (night): F-M16, the no-OS-branching guard checks something

- `capability_ui_no_os_branching` read only the top of `dist/` (just
  `index.html`), so it never saw the bundle in `dist/assets/`. It now walks
  `dist/` recursively and fails on a string literal that is exactly an OS
  name (`"linux"`, `'cosmic'`, …), which is what a branch compares against;
  help text naming COSMIC is allowed. Checked that a probe file with
  `os==="linux"` makes it fail.

## 2026-10-20 (night): F-M16, the kill-mid-migration test is deterministic

- The worker slept a random 80–400 ms in its transaction while the parent
  killed after a random 20–180 ms, so it sometimes committed first and
  exited 0 (the 1-in-10 flake). The worker now prints `ready` inside the
  transaction and waits; the parent kills only after reading it. 20 of 20
  runs pass.

## 2026-10-21 (night): F-M18 investigated, not done

- A subprocess test (mock lane, `NARRIVY_DELIVERY_FAIL=1`, SIGKILL,
  restart) cannot prove replay: the mock (non-strict) lane commits the text
  with `delivery_intent: null`, and startup replay skips entries without an
  intent, so nothing is replayed. Its journal also records an `inbox`
  attempt with `InboxFallback` although no Inbox file is written
  (misleading, test-only lane). The strict lanes write the intent. F-M18
  needs a strict-lane subprocess test with fixture audio and a fake engine.
- Seen on the way: a test daemon started by `daemon_shutdown_signals`
  writes `echo-cancel.conf` into its runtime dir and initialises the Mesa
  shader cache (GPU probing) even with the mock engine.

## 2026-10-21 (night): F-M11, the first-run wizard installs the model

- On a clean install the wizard's last step could never enable Start (no
  model). When the daemon reports `installed_model` awaiting, provisioning
  or activation-failed, that step now shows the bundled-model install (the
  Models page's texts and `installBundledModel`), with the install state;
  the status provider already follows the operation. Tests: offered on a
  clean install and calls the install; absent when a model is ready.
- Other F-M11 items are still open (typed failure reason on Models, Home
  "Ready" after a stop, Save racing a newer transcript, palette Cancel,
  palette focus trap).
- F-M11, second item: the typed install failure (`model_provisioning.failure`)
  is shown in words on the Models page and in the wizard (one sentence per
  reason, the code for an unknown one). Test in `BundledModelInstall`.
- F-M11, third item: a daemon that stops is no longer shown as Ready. One
  failed background read still keeps the last status (no flapping); two in
  a row (about 6 s) clear it and the app shows Offline (or Service failed).
  Test in `LiveStatusStrip`.
- F-M11, fourth item: Save on Home no longer overwrites a newer dictation
  that arrived during the edit; it says the edit was not saved. Test
  `HomeEditStale` (fails on the old code).
- F-M11, fifth item: the Start/Cancel latch and the pending-start flag live
  in the one SessionProvider, so a Cancel from the palette stops a Start
  Home is still resolving (and the palette can see it). Test
  `DictationLatchShared` (fails on the old hook); hook-level tests use a
  pinned session with the same latch (`src/test/sessionHookMock.ts`).
- F-M11, last item: the command palette keeps Tab and Shift+Tab in its
  search box (its only focusable element) and gives focus back to where it
  was when it closes. Test in `PaletteOpensAndExecutes`. F-M11 is done.

## 2026-10-21 (night): F-M20, set the shortcut in the first-run wizard

- When the daemon reports `activation.shortcuts=managed`, the wizard's
  welcome step offers "Set a shortcut", which opens the Pipelines capture
  dialog for the selected pipeline; when saved it says so. With manual
  shortcuts it shows only the suggested command, as before. Tests in
  `OnboardingCompletes`.
- `PerfDictationStartToListening` (a <100 ms timing test) failed once under
  the full parallel vitest run and passes alone; timing-sensitive, noted.

## 2026-10-21 (night): the hanging test found; F-M4 Inbox expiry

- **The hang** (twice tonight, `history_retention_composition` and the
  daemon test run): `assistant_builtin`'s page picture ran headless Firefox,
  Firefox crashed, and its crash reporter (reparented to init, outside the
  process group) kept the console pipes open, so joining the pipe readers
  waited forever. Production code had the same hang. Fixed: Firefox runs
  with `MOZ_CRASHREPORTER_DISABLE=1`; the browser's process group is killed
  whenever it ends; the console is collected through channels with a 2 s
  bound. Test `a_helper_holding_the_console_open_does_not_hang_the_picture`
  (a fake browser leaves a `setsid` helper holding the pipe; returns in
  about 4 s).
- **F-M4:** `InboxStore::prune_older_than` removes recovered-text files
  (regular `<txid>.txt` only, never through a symlink) older than the
  journal TTL (15 min); the daemon prunes at startup (after the journal
  replay) and after each strict-lane session. History is the durable copy
  (D6). Test `only_old_recovered_text_files_are_removed`. The Privacy-screen
  disclosure is still to do.
- F-M4 disclosure: Settings (Inbox backup, Privacy → Crash recovery) and
  `docs/security/data-retention.md` say each dictation also gets a private
  Inbox copy for 15 minutes, cleared at logout, with History as the lasting
  copy. F-M4 is done.

## 2026-10-21 (night): checkpoint

- `cargo test --workspace --locked`: all pass (63 test binaries).
  `pytest tests`: 312 pass; the one failure is the known
  `verify_manifest.py` drift (reviewed regeneration pending). Vitest: 205
  pass. Workspace clippy `-D warnings`: clean.
- Not done tonight, needs care or the owner: F-C7 (archive the probe-only
  audio code and amend FUN-006/012, REL-005/009 per D14; entangled with
  `spec/evidence-verification-profiles.json`), F-M18 (strict-lane SIGKILL
  test), the builder success-path test (F-M16), the manifest regeneration.
- Changes since rc4 (`4cd179b`): F-M20 wizard shortcut, the browser-hang fix,
  F-M4 Inbox expiry and disclosure. Build rc5 when the owner wants them in
  the smoke test.

## 2026-10-21: native smoke of rc4 on the clean user `narrivytest` (F-H6)

Owner upgraded rc1 to rc4 with apt (no maintainer-script errors) and ran
the smoke steps 05:17–05:22; logs `/tmp/narrivy-smoke-narrivytest-*.log`.
- **First start (F-H6):** the packaged unit is active and serving,
  `daemon_build=1.0.0-rc.4`.
- **Bundled model (F-C5):** installed from the app; `model.gguf=available`,
  worker ready, status bar "Speech model · IGPU".
- **Dictation (F-C6):** completed through the installed unit; end of
  capture to raw text about 150 ms, delivery about 120 ms. Delivery mode
  `type` (D37), `wtype` trusted, clipboard not authorized.
- **F-H18:** `LimitCORE=0`; the same user is refused
  `/proc/<pid>/environ`.
- **Stop:** about 2 s. **Log out/in (F-H5):** the daemon stopped with the
  session and served again after login; a dictation followed.
- **Found:** Models kept the "Offline speech model" panel and its Install
  button after the install was ready. Fixed (hidden once the model is in
  use); test in `BundledModelInstall`.
- **Still to confirm with the owner:** what text appeared where (the "café
  — it's done" check, F-H8, in an editor, a terminal and Firefox; no
  double insertion), and whether the shortcut alone started the stopped
  daemon (F-H4): the daemon restarted 2 s after the stop, which may have
  been the open app rather than the shortcut.
- Noise: each start logs "qualification registry unreadable … no
  accelerator may be selected" on a fresh user (yet the worker reported
  the iGPU); worth checking.

## 2026-10-21: bundled-model setup card redesigned (owner request)

- One "Set up speech recognition" card (`components/BundledModelSetup.tsx`)
  on Models and in the first-run wizard: names the included model and size,
  says nothing is downloaded, one Set up button; while running, the step in
  words ("Checking and copying the model…", "Starting the speech
  engine…") over a progress bar, with Cancel; on failure the reason and Try
  again; gone once a model is in use. Checked in the dev preview with
  `?model=awaiting` (new, preview only).
- Models with no published download were labelled "Included with Narrivy"
  (false); they are listed apart as "Not available yet".
- Owner asked why Refine was still in the side menu: an earlier request was
  never recorded. Decision D38 recorded; the move is item 0 of the handoff.
- Committed `ac25030`; vitest 207 pass. Not yet in a package (rc4 lacks
  these and the post-rc4 changes; build rc5 when wanted).

## 2026-10-21: D38, Refine moved into Pipelines

- The Refine screen and its side-menu entry are gone (`screens/Refine.tsx`
  deleted; `refine` removed from `SCREENS`, `PRIMARY_SCREENS`,
  `NAV_LABEL_KEYS` and `AppShell`). The Store keeps its Refine module
  category (D30).
- The pipeline's Refine stage now has two parts. **Rules:** spacing,
  punctuation, capitals and ending (From service, on in Clean, off in Raw),
  then the live numbers and spoken-breaks options, per-rule toggles
  (Coming · T2.20) and context hints. **Language model:** the clean-up
  model in use, read from the service with its state ("S1-mini · Ready on
  NPU", or "None: rules only"), with a link to Models; the device and the
  level (faithful/polish/rewrite) stay Coming · T2.20; the meaning check and
  "Stays on this computer" are Live.
- Found on the way: the old Refine screen still said "None installed" and
  showed the meaning check as Coming although S1-mini clean-up and its
  faithfulness check (`daemon/refine.rs` `faithful`) have been live since
  2026-10-02; the guard's copy now says what the check really does. The
  handoff's "planned vocabulary" option was dropped: vocabulary is live in
  the Transcribe stage. The duplicate "AI polish" choice and the old
  "Punctuation and capitals" line are removed, with 21 unused strings. Fixed
  a missing string id (`pipe.model.auto`) carried over from the old screen.
- Tests: a Refine stage test replaces the Refine screen test; tsc clean,
  vitest 207 pass; checked in the dev preview. Not yet in a package.

## 2026-10-21: rc5 built; smoke round 2 handed to the owner

- `1.0.0~rc5` built at `02c7a41` (`7ba1815b…`), with everything since rc4
  (D38 Refine move, setup card, F-M20 wizard shortcut, browser-hang fix,
  F-M4). rc4 is now `narrivy-previous-amd64.deb`.
- Round 2 steps on `narrivytest`'s Desktop (owner copies them with sudo):
  A what appeared in round 1 (F-H8), B shortcut with the daemon stopped and
  the app quit (F-H4), C upgrade rc4 to rc5, remove, reinstall (F-H15),
  D optional USB unplug (F-H2). The owner's notes go in
  `~/Desktop/NARRIVY-REMARKS.txt`, which the script appends to each
  `/tmp/narrivy-smoke-narrivytest-<step>.log`.
- Round 2 results so far: **A (F-H8):** "Cafe. It's done." once in Text
  Editor, terminal and Firefox (no double insertion), but the model wrote
  no é or dash, so non-ASCII delivery is untested; C3 retests it with a
  vocabulary replacement. **B (F-H4):** with the app quit and the daemon
  stopped, the shortcut started the daemon (29 s after the stop) and
  dictation; the second press stopped and delivered. **Found:** no overlay
  and no start/stop sound while the desktop app is not running.
  **C1:** rc4 to rc5 with apt, done.

## 2026-10-21: the owner's login was killed twice (fixed, ac5fbda)

- At 18:27 and 18:50 the owner's whole desktop session was SIGKILLed while
  a session ran `cargo test -p narrivyd --test e2e_mock_dictation`. Cause:
  the assistant page viewer stopped the browser with `kill -KILL -<pid>`;
  procps `kill` reads a group starting with 1 (`-14`, `-1028059`) as the
  option `-1` and signals every process of the user. Reproduced in a user +
  PID namespace with strace (`kill(-1, SIGKILL)`), fixed with `--`, and
  re-checked there (group kills only). Guard test added.
- **rc5 has this bug** (the assistant's page view would end the user's
  session when the browser's pid starts with 1). The next package must
  include ac5fbda.
- Not memory: the first logout coincided with high memory use (VM plus
  parallel builds), so the VM now runs with 4 vCPU / 7 GB and builds use
  fewer jobs anyway.

## 2026-10-22: owner report, a wrong transcription

- The owner said "install the latest version" and Narrivy wrote "install
  early decision" (dev daemon on the owner's account, `asr.transcribe-cpp.device=npu`,
  multilingual model). The same account's status shows dictation latency
  p50 about 11 s and p95 about 43 s over 40 sessions, which is also worth a
  look. Not investigated yet.

## 2026-10-25: assistant screen share fixes, and a testing mistake

- Fixed (9bddd49, 40ffdb5, a05230a): the live transcript follows within
  seconds, stopping (Stop sharing, or the last source switched off in the
  composer) ends the portal screen cast and the app's share bar, and a
  stopped share is saved raw (no transcription or screen descriptions until
  the user asks: "Go over it", or giving it to a chat).
- **Mistake:** the owner tested these several times on the installed rc6
  app (`/usr/bin/narrivy-desktop`, opened from the menu) while the app fix
  was only in a dev build, so it kept looking broken and cost the owner
  time and tokens. Rule from now on: before asking for a test, check which
  app and daemon binaries are running and that they contain the fix, and
  say so. rc7 (`de8534b`) packages the fixed app.

## 2026-10-25: rc9 assistant checks passed

- The owner checked rc9 (installed app `/usr/bin/narrivy-desktop`, service
  `daemon_build=1.0.0-rc.9`, both verified before asking): model switches
  back in the bottom bar, Saved sessions with Show / Give to chat, the
  privacy note gone, and screen share stops cleanly both from Stop sharing
  and from the composer's Screen button, with the session saved.

## 2026-10-25: screen share live pictures every second (rc10)

- Owner: during a share the live pictures lagged. The capture took a frame
  every 2 s and wrote the small live picture at most every 3 s (in practice
  4 s); the app read it every 3 s, so it was 3-9 s old. Now
  (414b72c): frames every second, `live-N.png` every second, `now-N.png`
  (for the assistant) every 3 s, the settle/keep check still every
  interval; the app reads the pictures every second and keeps the last one
  of a source that could not be read. The capture script runs from the
  checkout on the dev service; the app part needs the package.
- rc10 built from 247b08e (sha256 bb0c8167...). Not yet installed.

## 2026-10-26: owner round of changes for rc11 (not built yet)

- Owner (rc10): Saved sessions showed other chats' sessions; the live
  picture froze after a few seconds. The freeze is COSMIC's portal backend
  crashing (`create_frame sent before the previous frame was destroyed`,
  `wayland thread crash`, also four times on 2026-10-25 at the old rate);
  the capture now starts sharing again when the backend's D-Bus name goes
  away (756ae1d; detection tested with a fake name, the restart itself not
  yet seen live). Saved sessions are now per chat (`Chat.sessions`).
- Also asked by the owner and done: Home Services cards removed (6ea28c5);
  Pipelines Import/Export as buttons beside New pipeline with the desktop
  file chooser (`pick_file`), shortcuts on each pipeline's title row
  (df1ee43); Models "Where it runs" removed (467d6b4).
- The rc11 build was stopped at the owner's request (more changes first);
  the version is already 1.0.0-rc.11 (1a56ec8). Build and test together.
- transcribe.cpp upgraded 0.2.3 -> 0.4.1 (8889e91, owner). Worker: the
  model's capabilities are a Result now; weights memory-mapped; a decode
  stopped for repeating itself delivers the cleaned partial text (0.2.3 gave
  the loop). Engine tests 80 pass; the JFK clip decodes to the reference on
  CPU and Vulkan. Measured old vs new (same machine, one at a time, JFK 11 s):
  speed unchanged (tiny CPU ~0.38 s, turbo Vulkan ~2 s, turbo CPU ~13 s);
  unreclaimable memory: turbo CPU 1015 -> 147 MB, tiny CPU 128 -> 63 MB, GPU
  unchanged. Not yet used: VAD, language id, vocabulary/prompt fields (our
  context bias still goes through Whisper's initial prompt).

## 2026-10-26: OpenVINO 2026.5 nightly tried on the NPU (not adopted)

- Owner asked to try the 2026.5 nightly on the dev service only.
  `openvino_genai_ubuntu24_2026.5.0.0.dev20261023` (sha256 2aa117b8…,
  verified) unpacked to `~/.cache/handy-openvino-build/sdk-2026.5-dev20261023`;
  dev pack `~/.cache/narrivy/dev-runtimes/dev/openvino-runtime-2026.5.0.dev20261023-linux-x86_64`.
  The C headers we use are unchanged (2026.5 adds text-to-speech headers);
  library suffix .2650; GenAI now needs `libopenvino_gguf_frontend`.
- Probe, whisper-large-v3-turbo int8, JFK 11 s, warm: NPU 2026.4 0.45 s;
  NPU 2026.5 fails every run (`zeFenceHostSynchronize`
  ZE_RESULT_ERROR_UNKNOWN; kernel `ivpu_job_timeout_work … Engine reset`).
  GPU 0.24 s vs 0.28 s, CPU 3.2 s vs 3.0 s (no gain). The NPU recovered and
  works on 2026.4. The dev service was not switched.
- Kept (useful for the real 2026.5): workers take the library suffix from
  the pack; `NARRIVY_OPENVINO_RUNTIME_DIR` (dev only) points the service at
  a local pack checked against its own manifest, with its own cache; the
  pack builder takes `--version` (2026.4 pack rebuilds byte-identical).
- Untried: the pack's NPU user-space driver is the old one from Handy;
  linux-npu-driver v1.38.0 (2026-09-11) is newest and may be what 2026.5
  needs.

## 2026-10-26: Intel NPU driver 1.38.0 (in progress, owner asked)

- Before: kernel 7.2.11-popos `intel_vpu`; NPU40xx (Lunar Lake) firmware
  from Pop's linux-firmware, "Jun 30 2026 … 26ww25"; no Intel NPU packages;
  the runtime pack's user-space driver came from Handy.
- linux-npu-driver v1.38.0 (2026-09-11, Ubuntu 24.04 bundle, sha256
  1efcd4b6… verified) in `~/.cache/handy-openvino-build/npu-driver-1.38.0`
  (`x/` the .debs, `umd/` libze_intel_npu 1.38 + system libze_loader 1.32).
  Intel's tested firmware for Lunar Lake: "Aug 20 2026 … 26ww32".
- Dev packs in `~/.cache/narrivy/dev-runtimes/dev/`:
  `openvino-runtime-2026.4.0.npu1.38-…` and
  `openvino-runtime-2026.5.0.dev20261023.npu1.38-…`.
- With the new user-space driver only (old firmware): 2026.4 NPU works
  (0.46 s JFK, right words); 2026.5 still hangs the NPU.
- Done: owner installed intel-fw-npu, intel-level-zero-npu and
  intel-driver-compiler-npu 1.38.0 (+ libtbb12) and rebooted; the kernel
  loads firmware "Aug 20 2026 … 26ww32" (Intel's tested build).
- Results, turbo int8, JFK 11 s, warm, new firmware:
  2026.4.0 NPU 0.45 s (old or 1.38 user-space driver, same);
  2026.4.1 (GenAI 2026.4.1.0, sha256 b8ba7b6e… verified) NPU FAILS like the
  2026.5 nightly (zeFenceHostSynchronize, kernel job timeout + engine
  reset), with the plugin compiler and with the driver compiler; GPU 0.23 s,
  CPU 2.9 s. So Whisper on this NPU breaks from 2026.4.1 on, not a driver,
  firmware or compiler matter. Stay on 2026.4.0 for the NPU; possible
  upstream report (owner's call).
- Dev service: drop-in
  `~/.config/systemd/user/narrivy-daemon.service.d/openvino-runtime.conf`
  sets `NARRIVY_OPENVINO_RUNTIME_DIR` to the 2026.4.0 + driver 1.38 pack
  (`~/.cache/narrivy/dev-runtimes/dev/openvino-runtime-2026.4.0.npu1.38-linux-x86_64`);
  the worker reports `npu` (status shows `igpu` for a few seconds after a
  restart, then `npu`). Remove the drop-in to go back to the installed pack.
- Packs kept for later: 2026.4.1 and 2026.5 nightly (+1.38) in the same
  folder. Not updated yet: Rust crates (75 compatible updates, 42 behind a
  major), npm (React 18->19, Vite 5->8, Vitest 1->5, TypeScript 5->7, …).

## 2026-10-26: dependency refresh (handoff task 0)

- Rust (4747bc6, then the majors commit): every compatible update, taken
  with `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=allow` (the workspace's
  `rust-version = 1.85` held back tokio, Tauri and harper). Tauri 2.12.5,
  wry 0.57, tokio 1.54.1 (fixes RUSTSEC-2026-0386), zbus 5.19, harper-core
  2.13.0, rustix 1.1.5, ashpd 0.12.3; majors rusqlite 0.41, base64 0.23,
  rand 0.10 (`OsRng` → `SysRng`). The `unic-*` crates left with Tauri 2.12,
  so their five deny ignores are removed. fmt, clippy `-D warnings` and all
  2205 Rust tests pass (one run flaked in
  `latency_histogram_p99_under_load_stays_under_50us`: a shared global
  histogram counted 1001 of 1000 while another test ran; pre-existing).
- Formatting had drifted in 38 files on HEAD; fixed separately (521efd9).
- `cargo deny check` fails on advisories new since September, none ignored
  (owner's call, noted in B-SUPPLY-010): zip 8.6 extraction CVEs (fixed only
  in zip 9) plus unmaintained bincode/paste, all via harper-core 2.13.0 (the
  newest; harper and candle open zips in memory and never call the
  vulnerable `extract`), and unmaintained zbus_names/zvariant (merged into
  zbus 6; ashpd 0.13 and tauri-plugin-single-instance 2.5 still need zbus
  5, so moving to zbus 6 would not clear them).
- Portable MSRV job (Rust 1.85): the dependencies build, but
  `narrivy-asr-api` uses let-chains (stable in 1.88), so the job already
  failed on HEAD. Either raise `RUST_PORTABLE_MSRV`/`rust-version` to 1.88
  or rewrite those `if let … &&` chains (owner's call).
- Desktop npm: compatible updates (@tauri-apps/api 2.12.4 matches Tauri
  2.12), then React 19 + Testing Library 16 (8769b20; fixed a real race
  React 19 exposed: a late status reply after a reconnect reset a session
  the events were following), Vite 8 + plugin-react 6 + Vitest 5 + jsdom 30
  (acdaff3; lock regenerated; `npm audit` 0, was 5), jest-dom 7, TypeScript
  7.0.2 (native compiler). 213 tests and the build pass after each; the dev
  preview renders Home, Pipelines and Assistant with no console errors.
- Not moved: zbus 6 / ashpd 0.13 (no advisory gain, API work); gtk 0.19,
  libloading, toml 0.8 are inside Tauri/other crates.
- rc11 built at `ae64891` (build exit 0): `narrivy_1.0.0~rc11_amd64.deb`,
  sha256 9e07c4d734edb40c24f524232560fb6f5afe07bdecd89be60a6795d83492e564.
  Not installed or smoke-tested yet.

## 2026-10-27: desktop navigation tidy-up and tray languages (for rc12)

Owner requests, one at a time, each verified in the dev preview
(`desktop-ui`) with the frontend tests (217 passing) and committed:
- Settings, Store, Models and Meetings got the Assistant-style side panel
  (shared `.settings-page/.settings-side/.settings-tab` styles in
  `SettingsCompute.css`; `main.is-flush` for those screens in AppShell).
- Dictation page removed; its tests now drive Home (Start/Stop/Cancel,
  notices, selected id) and Pipelines ("Use this pipeline").
- Models has four kinds; `LanguageModelsPanel` takes `roles` and
  `listRole`; new `TextToSpeech.tsx`; Kokoro voice list moved to
  `lib/voices.ts`. No grammar-checker status is shown: the service does not
  report one.
- Meetings recorder compacted; new Tauri command `pick_recording`; the
  recorder's columns wrap by container width.
- Tray language submenu in `src-tauri/src/tray.rs` (`language_entries`,
  `entry_label`, `refresh_languages`; a named background thread refreshes
  every 60 s; `daemon_shortcut_set/remove` refresh it). 3 unit tests.


## 2026-10-27: dictation language (after rc12, for rc13)

- The owner's language shortcuts (COSMIC Alt+B `--output-language ur`,
  Alt+C `--output-language en`) and the tray's "Start dictation in…" failed
  after recording: a written language alone reached the engine, which takes
  only the spoken language. Fixed in the service (12474b5): a written
  language with no spoken language and no translation is dictated as that
  spoken language. `asr.translation=unavailable` on large-v3-turbo.
- Owner's design (1ee6b67): one current dictation language, chosen from the
  tray ("Language: …", check marks; choosing never starts) or Pipelines →
  Transcribe → Dictation language; the plain toggle, Start and press and hold
  use it; language shortcut keys are optional. Kept in `text.json`
  (`language`, `languages`), protocol minor 54; `narrivy-cli language
  [auto|TAG]`. The dev service has it (restarted 19:04); the tray and page
  need a new desktop build (rc13). Owner's list is `ur`, current automatic.
- Windows: `windows/port` re-authored as the owner (backup ref
  `backup/windows-port-before-reauthor`) and merged with rc12 (fa599aa);
  continued in its own session.
- **rc13 built** 2026-10-27 (commit `1b70fe0`, sha256 `d15bd9ee…8c19`), not
  installed yet. Owner checks, one at a time: tray "Language: Automatic"
  submenu (Urdu listed; choosing sets it, no dictation starts); the usual
  toggle then writes Urdu; Pipelines → Transcribe → Dictation language
  (choose/add/remove); Alt+B/Alt+C no longer fail. rc12 checks above still
  open.

## 2026-10-29: every review finding re-checked (read-only)

- Each F-C, F-H, F-M and F-L finding (plus a quick look at P-1 to P-10) was
  checked against `9d5a085` (rc13). The table is in
  `review-findings-status-2026-10.md`. No product code changed.
- **Open:** F-C7 (D14 rows not amended, probe code still in the tree),
  F-H9 (Clean still writes `3. 14`, `example. Com`), F-M7 (D4 base.en
  never measured), F-M18 (strict-lane SIGKILL test), P-2, P-3, P-8.
- **Partly fixed:** F-H1, F-H10 (CI skips most governance tests), F-H13
  (pipeline import/export writes a tmpfs dir no daemon code reads), F-H17
  (PRIV-003 privacy zones still a gated `must`), F-M12 (app ID, native),
  F-M16, F-M20, P-7.
- **Code fixed, native check pending:** F-H2 (unplug), F-H8 (non-ASCII),
  F-H15 (remove/reinstall).
- Release gate today: exit 1, 848 lines, 54 requirements blocked, manifest
  out of date (129 unlisted). Not edited, listed in the status file: the
  D14 rows in `requirements-traceability.csv`, PRIV-003, and the dated
  `last_gate_result` in `agent/STATE.json`.

## 2026-10-29: advisories after the 2026-10-26 refresh (B-SUPPLY-010)

- `cargo deny check advisories bans licenses sources`: advisories fail with
  8 errors; bans, licenses and sources pass. `npm audit --omit=dev` in the
  desktop app: 0 vulnerabilities.
- No compatible or small upgrade clears any of them, so nothing changed:
  - zip 8.6 (RUSTSEC-2026-0367/0368/0369): harper-core 3.0.0 still needs
    zip 8.6.
  - bincode and paste: required by harper and burn.
  - zvariant, zbus_names, zvariant_derive (RUSTSEC-2026-0370/0371/0372):
    ashpd 0.13.13 and every tauri-plugin-single-instance release still need
    zbus 5.
- Baseline tests pass: grammar-harper 7, platform-linux 310.
- Is our code exposed? The zip extraction functions are never called:
  harper only reads in-memory Weirpacks, and Narrivy never uses them. paste
  runs only at compile time. bincode is compiled in but never used (harper's
  model is MessagePack). The zvariant crates are used in production by the
  Linux adapter, but those notices are about maintenance only.
- Report with the owner's choices:
  `docs/maintainers/supply-chain/advisories-2026-10.md`. Recommended: a new,
  time-boxed owner acceptance listing each ID in `deny.toml`, plus an
  upstream harper zip-9 bump. Nothing is ignored until the owner decides.

## 2026-10-29: portable crates build at MSRV 1.85 again

- The CI job `portable-msrv` failed. `narrivy-asr-api` used let-chains (Rust
  1.88 and newer). `narrivy-core` used rusqlite 0.41 (needs 1.89, and its
  libsqlite3-sys 0.39 needs 1.88) and sha2 0.11 (needs 1.88).
- The let-chains in `crates/narrivy-asr-api/src/request.rs` are now nested
  `if` statements and behave the same. With the owner's approval, the
  workspace now uses rusqlite 0.40 (libsqlite3-sys 0.38.2) and sha2 0.10.9.
  No source code changed for the downgrade. `rust-version` stays 1.85.
- Guard: the CI job `portable-msrv` already runs. A comment by the
  `Cargo.toml` pins explains why they must not go past 0.40 and 0.10.
- Verified: the CI MSRV command on 1.85.0 passes. On the pinned 1.97.1
  toolchain, clippy `--all-targets -D warnings` is clean and `cargo test`
  passes for asr-api, core, history, model-manager, process-linux,
  platform-linux, asr-openvino and narrivyd (1814 passed). One test fails
  here and on the base commit too: `capability_check_mic_never_promotes_
  helper_presence_to_signal`, built into 5 test targets. It depends on this
  machine's microphone, and this cloud machine has no audio. `cargo deny`
  was not run here.
