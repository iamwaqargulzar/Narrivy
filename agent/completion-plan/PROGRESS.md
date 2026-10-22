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
  like a paste. **Workaround in place:** the owner's delivery is set to
  Type (`narrivy-cli delivery type`). A uinput keyboard (`ydotool key
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

## Where to start next (handoff, 2026-10-19)

1. **Owner question still open:** ship `1.0.0~beta1` for Pop!_OS before
   the full gate? (Recommended: yes. Freeze features, re-check F-C1, F-C4,
   F-C5, F-C6 against today's code, build with
   `scripts/build_deb_installable.py`, test on a clean user.)
2. **Paste on COSMIC:** a uinput paste chord (ydotool or our own uinput
   keyboard), so Paste works again; then switch the owner back from Type.
3. **Windows (D36):** SQ2 step 1, compile proof of the portable crates for
   `x86_64-pc-windows-*`, then the adapter plan.
4. The review findings in `00-review-findings.md` need a re-check against
   today's code: only F-C2, F-H10 and F-M15 are logged as fixed here.
