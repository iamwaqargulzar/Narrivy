# Owner decisions needed before (or early in) the build

D1, D7 and D10 are already answered or not needed for this release. Ask D3–D6, D8 and D9.

The coding agent must not decide these alone. Each one changes scope, user
behaviour, or what the release claims.

**Agent instructions:**
1. At the start of work, ask the owner every question below that has no
   recorded answer. Ask them together in one message.
2. Record each answer in `agent/DECISIONS.md` with the date, then fill in the
   "Answer" line here.
3. Until a decision is answered, work only on tasks that do not depend on it.
   The "Blocks" line says which tasks wait.

Each decision has a recommended option, chosen for the fastest release with
the fewest bugs.

---

## D1 — Text insertion on GNOME (answered 2026-09-28)

**Answer:** the first release is **Pop!_OS COSMIC only**. GNOME text insertion
(F-C3) is part of side quest SQ1, done after the Pop!_OS release is properly
tested. Nothing to do before release.

## D2 — Activation modes (answered: already documented)

Two modes, as documented in FUN-001/FUN-031: **toggle** (press, speak, press
to stop) and **press-and-hold** (hold while speaking, release to stop).
Neither stops on silence. Implemented by WP2 T2.6 (remove silence auto-stop)
and T2.13 (hold). Not a question for the owner.

## D3 — Package identity: maintainer and homepage (F-M6)

**What it is:** the fields every `.deb` carries, which users see in
`apt show narrivy` and software centres. Today they are placeholders:
`Maintainer: Narrivy <release@narrivy.local>` and
`Homepage: https://narrivy.local`.

**Already decided by the docs (not a question):**
- **Version scheme:** `docs/maintainers/operations/release-versioning.md` requires
  semantic versioning with alpha/beta/rc/stable channels. In Debian form that
  is `1.0.0~rc1`, `1.0.0~rc2`, … then `1.0.0`. The `~` sorts before the final
  release, and every one sorts above the `0.2.3` already installed.

**Open (owner answers):**
1. **Maintainer:** the name and email shown as the package maintainer (for
   example `Narrivy <you@example.com>`). This email is public inside the
   package.
2. **Homepage:** the URL shown for the project. The only real one today is
   `https://github.com/iamwaqargulzar/narrivy`, which is public but empty
   until your push decision. Options: use it (it fills in when you push), use
   another site, or omit the field for the first release.

Blocks: T3.1/T3.2 (package build). Answer: (owner, 2026-09-29): Maintainer `Waqar Gulzar <iamwaqargulzar@gmail.com>`; Homepage `https://github.com/iamwaqargulzar/narrivy` (D18). Version scheme as documented (`1.0.0~rcN`, then `1.0.0`).

## D4 — Default speech model (F-M7)

- **A (recommended):** measure `base.en` quantized. If release-to-text latency
  stays within target on the reference CPU, make it the default; it is much
  more accurate than tiny.
- **B:** keep `whisper-tiny.en-Q8_0` (smallest package, lowest accuracy).

Model files must have a licence and provenance recorded as for the current
model. Blocks: T2.9. Answer: **A** (owner, 2026-09-29): measure quantized `base.en` on the reference CPU. Make it the default only if release-to-text latency meets target; otherwise keep `tiny.en` and report the numbers. Record licence and provenance for any new model.

## D5 — Start the daemon automatically at login? (F-H4, F-H5)

- **A (recommended):** yes. The package enables the user service for all users
  (`systemctl --global enable`), tied to the graphical session. The hotkey
  then works right after login. The CLI also starts the daemon on demand as a
  fallback.
- **B:** no auto-start. Only the CLI starts it on demand when the hotkey is
  pressed.

Blocks: T2.5. Answer: **Modified** (owner, 2026-09-29):
  - the daemon does **not** start at login by default, and the package never
    enables it globally;
  - the desktop app gets a **"Start Narrivy at login"** setting, off by
    default. It runs `systemctl --user enable|disable narrivy-daemon.service`
    for the current user only (fixed absolute path, bounded timeout), and
    shows the real state from `systemctl --user is-enabled`;
  - the CLI still starts the daemon on demand when the shortcut is pressed
    (F-H4), so the shortcut works after login either way;
  - the unit is tied to `graphical-session.target` (F-H5), so when enabled it
    starts with the COSMIC session and gets `WAYLAND_DISPLAY`.

## D6 — Where recovered text lives, and mic-loss behaviour (F-M3, F-M4, F-H2)

- **A (recommended):**
  - move the Inbox to the durable state directory, with the same TTL as the
    journal;
  - add a small "Recovered text" list with copy and delete;
  - disclose on the Privacy screen that unverified deliveries keep a copy
    there;
  - when the microphone disconnects mid-dictation, save what was already said
    to the Inbox.
- **B:** keep the Inbox on tmpfs (cleared at logout) and document it as a known
  limitation. The mic-loss fix still applies.

Blocks: T2.7, T2.8. Answer: **Modified: one unified History** (owner, 2026-09-29). This also answers D11.
  - **Every dictation is kept in History, delivered or not.** Undelivered or
    unverified ones are flagged, with **Copy** and **Retry**. Each entry can
    be deleted, and there is a "Clear all".
  - **Size limit set by the user, by entry count:** default **10**, range
    **0–1000**, oldest removed first. **0 keeps nothing**: an undelivered
    dictation is then protected only by the crash-recovery journal (up to
    15 minutes). The UI says this when the user picks 0.
  - Stored durably under the private state directory (0700/0600),
    replacing the tmpfs Inbox. Journal replay and Inbox fallback write into
    this History, and a recovered item never disappears at logout.
  - Disclosed in onboarding and on the Privacy screen.
  - A mic lost mid-dictation saves what was already said (F-H2).

## D7 — Windows/macOS and AMD (answered 2026-09-28)

**Answer:** out of Linux 1.0 scope. PORT-001 moved to post-1.0 roadmap phase
R13, the Windows named-pipe clause of PORT-004 moved to PORT-007, and the AMD
NPU blocker B-HW-004 became a post-release side quest. Nothing to do before
release. See `07-post-release-side-quests.md` and `agent/DECISIONS.md`
(2026-09-28).

## D8 — Remove the retained Tauri shell? (WP1 T1.4) — **superseded by D22 (2026-09-29): keep Tauri, retire the GTK4 shell instead**

- **A (recommended):** yes, if T1.2 shows the GTK shell has parity.
- **B:** keep it excluded, as it is today.

Answer: **A** (owner, 2026-09-29): remove the Tauri shell after moving its 3 shared files into the GTK crate and confirming parity (WP1 T1.2/T1.4). Drop `@tauri-apps/api`.

## D9 — Human legal review of `THIRD_PARTY_NOTICES.md` (B-LEGAL-007)

The owner, or someone they choose, must review it before the release
candidate is final. The agent prepares a list of every bundled component
(Rust crates from the SBOM, npm packages, transcribe.cpp, model weights) with
its licence.

Answer: **A** (owner, 2026-09-29): the owner reviews it personally. The agent prepares the complete inventory and the rewritten notices, and flags unusual licences and the model licence mismatch. Approval is recorded in `agent/DECISIONS.md` with the file and SBOM hashes (WP6 T6.4).

## D10 — GNOME reference machine access (not needed for the first release)

Only needed for side quest SQ1, after the Pop!_OS release.

## D11 — Dictation History on by default? (F-H17)

Today the daemon always keeps History. Every transcript is stored for
**7 days** in `~/.local/state/narrivy/history.db`, as well as in the
Inbox/recovery copy. Nothing in onboarding says so.

- **A (recommended):** History **off** by default. Onboarding and the
  Privacy screen offer "Keep history for 7 days" as an opt-in.
- **B:** keep it on for 7 days, and disclose it clearly in onboarding and
  the Privacy screen.

Either way, privacy zones (PRIV-003) are not claimed for 1.0: nothing can
create them, and the target application is never known.

Blocks: T2.12 item 9. Answer: (owner, 2026-09-29): answered by D6. History is on by default with a **10-entry** limit, configurable 0–1000 (0 = off). Privacy zones are not claimed for 1.0 (D13).

## D12 — Should Clean mode turn number words into digits? (F-H9)

Clean mode rewrites every standalone number word: "no one" → "No 1",
"one day" → "1 day". The punctuation-spacing bug is fixed either way.

- **A (recommended):** drop number-word conversion for 1.0. Whisper already
  writes most numbers as digits.
- **B:** keep it only for runs of two or more number words ("one two three" →
  "1 2 3"), never for a single word.

Blocks: T2.12 item 6. Answer: **Optional, global plus per model** (owner, 2026-09-29):
  - a context-aware "Convert spoken numbers to digits" step in
    post-processing: "twenty five" → 25, "three point one four" → 3.14, but
    "no one", "one of them" and "the one" are left alone;
  - a **global setting, off by default**, plus a **per-model override**;
  - each model's catalog entry declares whether it already writes digits
    (whisper does). For such models the step stays off unless explicitly
    overridden for that model, so it can't double-convert;
  - wired end to end to the daemon;
  - the current word-by-word ITN table is removed;
  - table tests for every example.

## D13 — Two release-required features that exist only as library code (FUN-041, PRIV-003)

The second pass found that two `required` Linux 1.0 requirements have library
code but no product wiring:
- **FUN-041 (spoken aliases → canonical forms):** `crates/narrivy-history/src/dictionary.rs`
  is called only by its own tests. No correction or delivery path uses it,
  and there's no UI to manage aliases.
- **PRIV-003 (Privacy Zones override permissions):** zones load from the
  History database, but nothing can create one. The target application is
  never known (`session.rs:376`), and zones are applied only to History.

Options:
- **A (fastest release):** move FUN-041 and PRIV-003 to post-1.0 with a
  recorded amendment in `agent/DECISIONS.md` and the traceability CSV.
  Remove the UI and doc claims for 1.0.
- **B:** implement them for 1.0:
  - aliases applied in the Clean pipeline, plus a Dictionary screen with
    add and remove, about 1–2 days;
  - zones need a trusted target-app identity on COSMIC, which doesn't exist
    today. High risk; not recommended for 1.0.
- **C:** B for FUN-041 only, A for PRIV-003 (recommended if aliases matter to
  you).

Related, with no decision needed: FUN-064, PRIV-004 and PRIV-007 concern
remote/cloud data. 1.0 has no network client at all. Their procedures should
measure that (no network client, no remote provider composed), not pretend a
cloud path exists.

Blocks: WP4 procedures for these IDs. Answer: **C** (owner, 2026-09-29):
  - **aliases in 1.0:** apply the dictionary in Clean post-processing, plus a
    small Dictionary screen to add, edit and delete aliases, backed by
    daemon commands;
  - **privacy zones (PRIV-003) move to post-1.0**, with a recorded amendment,
    because they need trusted target-app identity on COSMIC;
  - remove the zone claims from the 1.0 UI and docs;
  - FUN-064, PRIV-004 and PRIV-007 are proven by measuring that no
    network/remote provider exists.

## D14 — Audio features that exist only in probe code (F-C7)

The shipped daemon records from the default microphone with `pw-record` and
nothing else. Pre-roll, ducking and pausing other audio, mic ranking and
override, and hotplug handling exist only in probe examples.

- **A (recommended, fastest):**
  - FUN-007 = **leave only** for 1.0 (other audio is untouched);
  - move FUN-006 (pre-roll), FUN-012, REL-005 and REL-009 (duck/pause) to
    post-1.0 with a recorded amendment;
  - FUN-011 reduced to what the product does: default source, a clear error
    when no mic exists, and the next dictation works after a replug (tested
    natively);
  - PERF-006 measured on the real capture thread.

  About half a day of work, mostly traceability and procedures.
- **B:** wire ducking (with the crash-safe journal fixed) and mic selection
  into the daemon for 1.0. About 3–5 days plus native qualification. Ducking
  changes the user's system volume, so crash bugs are user-visible.
- **C:** A, plus a microphone picker in Settings (FUN-011 user override).
  About 1 day.

Pre-roll needs the microphone open before the shortcut is pressed, which is a
privacy trade-off. Recommended to stay post-1.0 in every option.

Blocks: WP4 T4.1 (audio-policy recorder) and T2.x wiring if B/C. Answer: **C** (owner, 2026-09-29):
  - FUN-007 = **leave** (other audio is untouched);
  - **microphone picker** in Settings, backed by a daemon command, which
    `pw-record` uses as its target;
  - a clear error when no microphone exists; after a replug, the next
    dictation works (native test);
  - FUN-011 is restated to exactly that;
  - PERF-006 is measured on the real `recovery.rs` capture thread;
  - FUN-006 (pre-roll), FUN-012, REL-005 and REL-009 (ducking) move to
    post-1.0 with a recorded amendment;
  - the probe-only audio code is archived per the cleanup plan.

## D15 — Do the open-source cleanup (`08-open-source-readiness.md`)?

- **A (recommended):** yes. Phase A as WP1b before the freeze, Phase B
  before the first push, Phase C after release.
- **B:** only Phase B (front-door files), and leave the structure as it is.

Answer: **A** (owner, 2026-09-28): do it. Phase A as WP1b before the freeze, Phase B before the first push, Phase C after release.

## D16 — Keep the autonomous-build machinery in the public repository?

This covers `agent/` (release-process state and evidence), the agent prompts
and orchestrators, and the build-pack files.
- **A:** keep `agent/` public (trimmed per T-OSS.5) and archive the rest.
  Transparent: anyone can re-verify the release evidence.
- **B:** keep only what re-verification needs (`agent/evidence`,
  `agent/phase-reports`, `spec/`), and move the rest to a private branch or
  repository.
- **C:** move all of it private; ship the source plus the release SBOM and
  checksums.

Recommended: **A** for 1.0, because the evidence is a selling point for a
privacy product. Revisit later.

Answer: **A** (owner, 2026-09-28): keep a trimmed `agent/` public (T-OSS.5); archive the rest.

## D17 — Contributor sign-off

- **A (recommended):** DCO (`git commit -s`), checked by a GitHub app or
  action. Lightweight, and standard for Linux projects.
- **B:** none. Simplest, but weaker provenance.
- **C:** CLA. Heavier; only needed if you may relicense later.

Answer: **A** (owner, 2026-09-28): DCO sign-off (`git commit -s`), checked in CI.

## D18 — Public identity and contacts (extends D3)

Needed for README, SECURITY, CODE_OF_CONDUCT and package metadata:
- GitHub organisation/repository URL;
- website (or use the GitHub URL);
- a security contact email and/or GitHub Private Vulnerability Reporting;
- a conduct contact.

Answer: (owner, 2026-09-28, following the recommendation):
  - repository and homepage: `https://github.com/iamwaqargulzar/narrivy`;
  - security reports: GitHub Private Vulnerability Reporting on that repository (the owner enables it in the repository settings);
  - conduct reports: a private message to the maintainer through GitHub, as named in `GOVERNANCE.md`.

  A dedicated email address is optional and can be added later. This also answers the homepage half of D3; the maintainer name and email for the package are still open under D3.

## D19 — Docs website?

- **A:** plain markdown on GitHub for 1.0; mdBook later (recommended).
- **B:** mdBook on GitHub Pages at release.

Answer: **A** (owner, 2026-09-28): plain GitHub markdown for 1.0; mdBook later.

## D20 — Public git history at the first push

- **A (recommended):** squash to one "Narrivy 1.0" commit. The build
  history stays in your local repository and the `archive/` folder.
- **B:** full history after a `git filter-repo` scrub of personal data
  (P-9).

Answer: **A** (owner, 2026-09-28): squash to one "Narrivy 1.0" commit at the first push. It is the owner who pushes, never the agent.

## D21 — Speech-engine pipeline for 1.0 (already locked by the owner; not a new decision)

**This was documented before the review and must not be asked again.** Source:
`agent/RESOLVED_VERSIONS.json` (`vulkan_acceleration`, `openvino_strategy`,
`model_tiers_locked`, `model_library_strategy`, `huggingface_strategy_locked`,
`npu_cache_strategy`, `quant_selection_ux`, `quantizations`), owner-locked
2026-08-21 and reaffirmed 2026-09-21. On 2026-09-29 the owner confirmed it
applies to 1.0.

- **Order:** **NPU (OpenVINO 2026.4)** → **GPU pool: Vulkan (transcribe.cpp /
  ggml), OpenVINO GPU, and CUDA/ROCm/TensorRT via ONNX Runtime where
  present** → **CPU fallback (transcribe.cpp), mandatory and last resort.**
- **Models:** three tiers. Accuracy = Whisper large-v3 (OpenVINO IR
  FP16/INT8, ONNX FP16, GGML); Balanced = the Whisper small family; plus a
  CPU-fallback tier. Quantizations and the "which quant is better for you"
  recommendation follow `quantizations` and `quant_selection_ux`.
- **Where models and runtimes come from:** the **narrivy Hugging Face org**
  (<https://huggingface.co/narrivy>), with pinned immutable revisions, a
  model manifest and SHA-256 verification (ADR-0017). Upstream sources are
  candidates only (candidate → verified → qualified → default-eligible).
  Optional runtime packs (such as OpenVINO) are versioned and
  Narrivy-managed.
- **NPU compiled cache:** cold compile → cached blob → warm load, keyed by
  model hash, recipe, precision, runtime, device/driver and compile options.
- **Qualification:** a tier is used only after its exact
  model/runtime/driver/device tuple passes a local self-test. Runtime
  failure falls back down the order. Release claims cover only tuples
  qualified on the reference Core Ultra 9 288V (Intel NPU + Arc iGPU).
  Tuples that can't be qualified there (CUDA/ROCm/TensorRT, AMD NPU) are
  still built and self-tested on users' machines, but carry no release
  claim.
- **Correction:** the 2026-09-28 note that Linux 1.0 "makes no accelerator
  claim" (D7) covered only AMD hardware. It does not reduce this pipeline.
- Tasks: WP2 T2.15 (GPU pool and CPU), T2.16 (NPU), T2.17 (model and
  runtime-pack delivery).

## D22 — Follow Handy: Tauri 2 on every OS, keep the daemon (answered 2026-09-29)

**Answer (owner):** follow Handy for cross-platform capability, overlay and
text handling. In detail:
- **Tauri 2 is the desktop host on Linux, Windows and macOS**, at Handy's
  current version line (2.11.5+).
- **Narrivy's daemon, portable core, delivery safety, History and helper
  isolation are kept.** Tauri is a client of the daemon.
- The owner **accepts the remaining Linux Tauri advisories** after upgrading.
  They are listed by ID in `deny.toml` and justified in ADR-0075.
- The overlay uses Handy's layer-shell approach (GTK3 `gtk-layer-shell`).
- Text insertion uses Handy's typing and paste ladder.
- Press-and-hold uses `handy-keys` (evdev, `input` group).
- This supersedes ADR-0064 and D8's Tauri removal. The GTK4 shell is
  retired after parity.

See `docs/adr/ADR-0075-tauri-everywhere-with-daemon.md` and
`09-cross-platform-architecture.md`.

## D23 — Module Store, Refine screen and light noise suppression (answered 2026-09-30)

**Answers (owner):**
- **Store and Refine become sidebar entries.** The Store lists what can be
  added and whether it fits this computer; Refine holds the rules and the
  optional language model.
- **Noise suppression:** 1.0 ships a small built-in cleanup that works on
  any computer. Stronger, hardware-specific modules arrive through the
  Store after 1.0.
- **Refinement language model:** local only first. Cloud may follow later
  as a clearly marked opt-in.
- **Trust in 1.0:** Narrivy Verified items only (ADR-0017 unchanged).
- **Hosting:** the Narrivy Hugging Face organisation for models, runtimes
  and modules: one trust root, pinned revisions, checksums.

Design: `docs/roadmap/design/module-store.md`.

## D24 — Shortcut capture on COSMIC (answered 2026-09-30)

**Answers (owner):**
- **Narrivy may add and remove its own entries** in the desktop's keyboard
  shortcuts, and only its own. Setting a shortcut is click a button, press
  the keys. Copying a command stays as the second option. The onboarding
  sentence "Narrivy does not create or change desktop shortcuts" changes to
  say it changes only the ones set in Narrivy.
- **A combination already in use:** show what uses it and offer both
  choices: pick another combination, or replace the other binding after an
  explicit confirmation. Never replace silently.
- **COSMIC first.** Other desktops show the copy-a-command option until
  their adapters are added after 1.0.

Design: `docs/roadmap/design/shortcut-capture.md`. Press-and-hold is
unchanged (D2, T2.13).

## D25 — Vulkan by default; CUDA and OpenVINO as Store add-ons (owner direction 2026-09-30)

**Owner:** "This software should have Vulkan for acceleration by default on
Linux and Windows, and CUDA support would be an external Store add-on, so is
OpenVINO."

- **Vulkan is part of the base install** on Linux, and on Windows when that
  port exists. It needs no download.
- **CUDA and OpenVINO are Store add-ons** (T2.19): optional runtime packs
  from the Narrivy Hugging Face organisation, as D21 and D23 already
  describe for OpenVINO. D21 listed CUDA "via ONNX Runtime where present";
  it is now an add-on the user installs, not something probed for.
- The NPU → GPU → CPU order and the self-test rule of D21 are unchanged.

Implemented so far: ADR-0076 (Vulkan in the worker, self-test, CPU
fallback). Remaining: T2.15 (stored qualification, device setting,
packaged build), T2.16 (OpenVINO engine), T2.19 (Store).

## D26 — One OpenVINO worker, one shared language model, and a writing assistant (owner direction 2026-09-30)

**Owner:**
- "I do want everything to be loaded through OpenVINO as it is the most
  efficient and the quickest path."
- They asked for a Grammarly alternative: "a browser extension … for
  Chrome and Brave", where "the LLM would come into play and also the
  language logic engine to basically fix the text, improve the text,
  rewrite the text".
- On memory: "that model wouldn't unload; it would be the same model …
  so that our whole footprint is reduced and kept in control."

**What this means:**
- **With the OpenVINO add-on, every model stage runs in one OpenVINO
  worker process:** noise suppression, speech recognition and the
  language model. Each stage has its own device (CPU, GPU or NPU), is
  self-tested (D21), and falls back when it fails.
- D25 is unchanged: OpenVINO is a Store add-on, and without it recognition
  stays on transcribe.cpp with Vulkan.
- **Exactly one language model is loaded.** Dictation Refine and the
  writing assistant share it. It is never loaded twice. Dictation takes
  priority, and extension requests can be cancelled.
- **Writing assistant:**
  - a Chrome and Brave extension (Manifest V3);
  - it connects through native messaging to a limited Narrivy host,
    and never through a local web port;
  - a rule-based checker gives underlines while typing;
  - the shared language model fixes, improves and rewrites text on
    request;
  - results pass the ADR-0018 guard, and the user previews and can undo
    every change.
- **Privacy:**
  - the extension reads only the focused field, and never password,
    payment or one-time-code fields;
  - each site can be switched off;
  - there is no network access, and text is never logged.

**Owner answers (2026-09-30):**
- **When:** after the release. It is an added feature (SQ-WRITE in
  `07-post-release-side-quests.md`).
- **Without OpenVINO:** the language model also runs on Vulkan (the
  default), on CUDA (add-on) and on macOS with Apple's hardware
  acceleration. It is a global feature on every supported platform.
- **Languages:** English by default. Others are chosen by the user; which
  ones are offered is not decided yet.

Design: `docs/roadmap/design/writing-assistant.md`. Tasks: T2.16 (one
worker for all OpenVINO stages), T2.20 (shared model), SQ-WRITE (writing
assistant, after the release).

## D27 — The Store holds accelerators and modules; models stay on Models (owner direction 2026-10-01)

**Owner:**
- "Models should only show up in models."
- The Store "needs proper redesigning": accelerators with a CUDA option;
  Vulkan "is packaged with this software so that shouldn't be there but
  to notify that it is already there".
- "Have other modules over here such as the refining modules in LLM
  small language specialists."

**What this means:**
- `ListStore` lists runtimes and modules, never speech models. OpenVINO
  model packs are installed and removed on the Models screen.
- **Accelerators:** Vulkan is shown as built in and in use when the speech
  model runs on the GPU; OpenVINO installs; CUDA is shown as coming later,
  with whether an NVIDIA GPU is present.
- **Refine and noise suppression:** the planned modules from
  `spec/catalogs/modules.json` are shown as coming later: the refinement
  language model (shared, D26), grammar and style check, neural
  punctuation, offline translation, and noise suppression on the NPU or
  the processor.
- Design: `docs/roadmap/design/module-store.md`.

## D28 — Ideas from other projects, our own implementation (owner direction 2026-10-01)

**Owner:** "We will take ideas from both. We will rewrite our own as I don't
want to attribute any other repo. Libraries and dependencies are okay."

**What this means:**
- Other dictation and voice projects (OpenWhispr, VoiceStudio and others)
  are sources of ideas only. Narrivy writes its own implementation and never
  copies or ports their code, so no feature carries another repository's
  attribution.
- Libraries and dependencies are allowed, under the usual licence and
  supply-chain checks.
- Ideas noted from the 2026-10-01 review, not yet scheduled: live text for
  Whisper by decoding in chunks; honest delivery status ("Inserted" or
  "Copied") with the clipboard restored; editing selected text by voice and
  a translation shortcut on the shared language model (D26); dictionary
  hints for OpenVINO models; pause and resume; a local API for other apps.

## D29 — Dictation experience first, then a workspace (owner direction 2026-10-01)

**Owner:** after trying a polished competing app, they want:
- its smooth flow: "live streaming the words and it auto corrects them then
  paste them";
- one way of inserting text on Linux, macOS and Windows, to reduce
  development and testing time;
- a language-model area, notes, a direct chat and a dictionary;
- meetings with "both mic and speaker connected", with the notes placed in a
  notes area;
- possibly a video area (not decided).

Stack: Tauri stays (owner, 2026-10-01). The web view only draws; the daemon
does the work.

**What this means:** `docs/roadmap/design/dictation-experience-and-workspace.md`.
- **P1, dictation polish** (with the reliability details in section 9 of the design):
  - inserting text through the clipboard, with Inserted or Copied, the
    clipboard restored, and terminal-aware paste chords;
  - live text for Whisper;
  - the dictionary with recognition hints;
  - optional clean-up by the shared language model, behind its guard.
- **P2:** edit by voice, translation, learning from corrections, spoken
  commands, snippets, undoing the last dictation, one shortcut per job.
- **P2b:** Narrivy's own ideas: per-app behaviour, review before inserting,
  hands-free mode, fitting the computer, a privacy dashboard.
- **P3:** notes, including a Markdown folder mirror.
- **P4:** meetings (microphone and computer sound) and importing audio or
  video files.
- **P5:** a chat agent with tool calling (local tools first, then opt-in web
  research with citations; owner noted that chats without tools or web
  access are a limitation), search by meaning, telling speakers apart, live
  captions.
- **Left out:** accounts, team spaces, cloud sync, outside connectors,
  calendars, leaderboards and a mobile app.
- **Open question:** whether the "video area" means importing video files
  (planned in P4) or something else.

## D37 — No beta; how the package delivers text; crash dumps (owner, 2026-10-20)

- **Beta:** no `1.0.0~beta1`. The first public package is 1.0, after the
  full release gate. (Closes the question left open in D36.)
- **Delivery in the package:** typing by default. Paste (the clipboard put
  back afterwards) is a user setting (T2.4), not the unit's environment.
  Paste through uinput (`paste-keyboard`) stays a further opt-in for users
  who already have `/dev/uinput` access.
- **Crash dumps (F-H18):** blocked fully: `LimitCORE=0` in every unit copy
  and `prctl(PR_SET_DUMPABLE, 0)` early in `narrivyd` and in each worker it
  starts (check they inherit it or set it again). Amend the ADRs' "release
  blocker" wording to "core dumps: contained; swap: documented residual",
  and document swap in `docs/user/privacy.md`.
- **Package identity:** as D3 (maintainer, homepage, `1.0.0~rcN`, then
  `1.0.0`).

## D36 — Windows comes forward; macOS is set aside (owner direction 2026-10-19; amends D7)

**Owner:** "yes we bring windows forward" and "remove the MAC point for
time being".

**What this means:**
- **Windows** no longer waits for the Pop!_OS release to be tested (D7,
  SQ2's entry condition). The work in `07-post-release-side-quests.md` SQ2
  can start now, alongside the Linux release work: compile proof for the
  portable crates, then `adapters/platform-windows` (capture, delivery,
  activation, a named-pipe IPC transport), packaging and a Windows lane.
  Development and most testing can run in a Windows VM (KVM; CPU only, no
  NPU or GPU passthrough); the final check needs real Windows hardware.
- **macOS:** no work and no plan for now (SQ3 is parked). A macOS VM on
  this Intel PC is not an option: Apple's licence allows macOS VMs only on
  Apple hardware, and it would have no Metal or Neural Engine to test.
- **Unchanged:** Linux 1.0 (Pop!_OS COSMIC) stays the release target and
  `BUILD_TARGET.json` is not changed. Linux release work keeps priority
  where the two compete.
- **Open:** whether to ship a `1.0.0~beta1` for Pop!_OS before the full
  1.0 gate passes (asked 2026-10-19, not yet answered).

## D35 — The assistant researches the web, uses the computer and watches with the user (owner direction 2026-10-13; amends D34)

**Owner:** "Have it view through with me a video or a meeting and later I
could quiz it", "a button to load it and then … a toggle to auto unload or
not", "a full interface that I can see what the agent is doing in terms of
web research", and "full capabilities regarding computer and internet
research".

**What this means:**
- **Watch with me:** a meeting started from the assistant keeps the screen
  every few seconds (one picture per screen or window the user picks in
  the portal, so several screens work) and, by the user's choice, the
  microphone. A live thumbnail of each source shows what is being watched.
  Afterwards the pictures are described and the assistant answers
  questions about the video or meeting.
- **Loading:** the model is not loaded at start; a Load button loads it and
  "Unload when idle" (on by default) decides whether it is freed after a
  while.
- **Web research (network):** off until turned on for a chat. Search goes
  through Brave Search (the user's own key, kept in the desktop keyring),
  a SearXNG address the user gives, or DuckDuckGo and Wikipedia; pages are
  fetched with the system `curl`, public addresses only (no localhost or
  private networks). This is the assistant's only network access, and it
  is the user's per-chat choice, so extension network access stays
  deny-by-default.
- **Computer:** off until turned on for a chat. Reading folders and files
  and looking at the screen ask once (or always in the chat); writing a
  file, running a command and opening something are asked every time.
- **Activity panel:** every search, page, file and command is shown as it
  happens, with its results and the sources.
- **Linked to its chat, pausable and stoppable** (owner: "This should be
  linked to the chat … This process should be PAUSABLE AND STOPPABLE"):
  watching starts from a chat (the open one, or a new one) and what was
  watched is added to that chat. Going over it can be paused, resumed or
  stopped from the chat and from Meetings; deleting the chat stops it.
- **Screen share and screen recording** (owner, the same day: "Watch with
  me … should be renamed to screen share. And there should be an option to
  record the screen … that session can be given to the LLM"): *Share live*
  belongs to the open chat, which sees the screen with every question,
  spoken or typed, and hears what is said. Once it stops it is kept as a
  session (owner: "after I stop the screen share, it should show as a
  session that can be given to the AI agent and not that instant"), like a
  recording, and given to a chat only when the user chooses.
  *Record a session* keeps the screen and, as chosen, the microphone and
  the computer's sound; it is transcribed and its pictures described, and
  it can be given to any chat from the composer's "+" or from Meetings.
- **Permissions** (owner: "File access should be on permission and also on
  full permission. Web research should not have any permission as the
  screen sharing"): web research and looking at the screen never ask; the
  computer's files and commands ask first, or, with the chat's *Full
  access*, run without asking (every step still shows in Activity).
- **Sound only or screen and sound** (owner: "a separation between the live
  transcribe and screen sharing … a podcast … I just need the transcribes …
  a video … transcribe and the video share"): both sharing live and
  recording a session can keep the sound only (a live transcript, no screen
  dialog) or the screen with the sound.
- **Switch the screen and the transcript as the meeting goes** (owner: "in a
  meeting we are talking at first, then we move on to screen sharing … enable
  and disable screen sharing when I want to and enable and disable the
  transcribe when I want to"): Screen and Transcript switches on every
  meeting, share and recording while it records; the screen asks which one
  only the first time.
- **Live transcript:** while anything is recorded, every chat can read what
  has been said so far; only the sharing chat sees the screen.
- **Model status:** the status bar says whether the speech model, the
  assistant's model and voice mode are loaded, and a notice says when the
  assistant's model loads or is unloaded.
- **Tools in voice mode too:** a spoken question in a chat with the web or
  the computer switched on uses them like a typed one; only the final
  answer is spoken, naming its sources rather than reading out links.

## D34 — The assistant extension: voice mode, a library, pictures, screen-aware meetings and tools (owner direction 2026-10-13)

**Owner:** "Implement everything as we have already discussed, as these are
and will be in the extension category." The features are rebuilt from
Pithagoras' and OpenWhispr's ideas (D28: no code taken), local only.

**What this means:**
- **Extension, not core:** all of it belongs to the AI assistant and
  meeting extensions (D33). Dictation never waits on it, and it can fail
  without touching dictation.
- **Voice mode:** hands-free spoken turns (Silero VAD, about 1 s of silence
  ends a turn, Whisper while you speak), replies spoken sentence by
  sentence (Kokoro English and Piper Urdu through sherpa-onnx, on the CPU),
  talking over a reply stops it, "say that again", speed 1–1.75×,
  push-to-talk, mute, sound cues, an orb, and a per-turn timing line.
  The voices are downloaded by the owner's choice (Kokoro v1.0, Apache-2.0;
  Piper ur_PK fasih, MIT); none ships with Narrivy (D32).
- **Library kept by the daemon:** chats (so a reply is saved with no window
  open), memory, personas, projects (with the folder's own instructions
  file), skills loaded only when named (`/name`), and routines run on a
  schedule, results in the app only. Edit, delete and steer messages.
- **Pictures:** questions can carry pictures for the vision-language
  model; the assistant can draw bar and line charts.
- **Screen-aware meetings:** with the user's portal permission, screens
  that change are kept as pictures in the meeting's folder and described by
  the same Qwen3.5-4B after the meeting; notes use speech and slides.
- **Tools (MCP):** local MCP servers the user adds, every tool call
  approved by the user first; no network access is given by Narrivy.

## D33 — Store sections; extensions; a live voice changer (owner direction 2026-10-02)

**Owner:** keep the Store as Accelerators, Modules and Extensions.
"Extensions would include the Grammarly extension … and also the AI
assistant area … Meeting transcribing and I would also like to add a voice
changer … take the mic … make a virtual mic that I can use in any
application and it will change the voice live." The owner's research found
two voice-conversion models that run on the NPU (to be named by the owner).

**What this means:**
- **Store sections:** Accelerators (what makes things faster), Modules (what
  works inside dictation: clean-up, grammar, punctuation, translation, noise
  suppression), Extensions (what brings Narrivy into other apps or adds a
  whole area). "Plugins" stays reserved for third-party add-ons after 1.0.
- **Extensions:** the browser writing assistant (Chrome and Brave, SQ-WRITE),
  the AI assistant (F24, F25), meeting transcription (F22) and the voice
  changer (F31). Each is turned on from the Store; their models are
  downloaded from Models (D32). Recommended: the extension's code ships with
  Narrivy and is off until turned on, so no downloaded code runs outside the
  runtime-pack trust path (ADR-0017).
- **Voice changer (F31):** microphone in, converted voice out as a virtual
  microphone (a PipeWire source) that any app can choose; runs on the NPU.

## D32 — No model ships with Narrivy; language models get their own Models section (owner direction 2026-10-02)

**Owner:** "We are not going to ship a model with this first release. It
is … a type of extension. … the model would be in the model section …
another model area which will have the language based or LLM based models
separate from the speech to text models. That clear distinction is
required as both models serve a different purpose."

**What this means:**
- **No model is bundled in the first release.** Speech and language models
  are downloaded by the user. "Default" in D30/D31 means **Recommended**:
  the model the app suggests first, with a Recommended badge.
- **The Models screen has two separate sections:**
  - **Speech to text:** the speech recognition models (as today);
  - **Language models:** grammar models and AI models (D31), each marked
    with what it does (clean-up, AI area).
- **The Store keeps extensions, not models:** accelerators and modules.
  The Refine module (language-model clean-up) is an extension in the Store;
  the model it uses is chosen from Models → Language models. A module that
  needs a model links there, as speech add-ons do today.
- **Licence notices (D31)** apply on the Language models section.

## D31 — A grammar model for clean-up, a large model for AI (owner direction 2026-10-02; amends D30)

**Owner:** add "the model which is specifically designed to correct grammar
and also have a large LLM option as well for the AI system, but that AI
system model can also be used for correction as well … it will have 2
models active if a person wants."

**What this means:**
- **Clean-up model (default):** a model built only to correct grammar and
  punctuation (a rewrite model, not a chat model). It is the one model
  loaded by default when Refine clean-up is on. Because it can only
  rewrite, it cannot answer the dictation; the five anti-answering layers
  stay as a safety net.
- **AI model (optional):** a large general model for the AI area (edit by
  voice, translation, notes, chat, research).
- **The AI model can also do clean-up:** in Refine, the user picks which
  model cleans up: the grammar model or the AI model.
- **Two models at once only when the user wants it:** for example the
  grammar model for every dictation and the AI model for chat. Otherwise
  one model is loaded. Dictation keeps priority on the device.
- **Languages:** grammar models are mostly English. For a language the
  grammar model does not cover, clean-up uses the AI model if it is
  installed, otherwise only the deterministic clean-up runs.
- **Before choosing a model:** F30 benchmarks the candidates on the NPU and
  GPU and records each licence.
- **Non-commercial models are allowed (owner, 2026-10-02):** "we can't
  actually know … what they are using it for … show a notice that it is
  no commercial model please obey by the rules." A model whose licence
  limits use (for example non-commercial) can be offered in the Store with:
  - a notice naming the licence and its limit, which the user accepts
    before the download starts;
  - the licence and attribution kept with the model and shown on its
    Store card;
  - a model that does not allow redistribution at all is never hosted in
    `narrivy/store`.
- **Recommendation for the default:** where a model with an unrestricted
  licence is close in quality, it is the default, so an installation with
  no extra choices carries no limit. The owner decides after F30.

## D30 — A default model and an agent model (owner direction 2026-10-01; amends D26)

**Owner:** "We can have a better agent model for AI area and a normal one
for transcribing … we can switch when we want to but the smaller one would
be active by default." Benchmarking comes later, not now.

**What this means:**
- **Default model:** small (2B class). It is the only language model loaded
  by default, and it does clean-up (Refine).
- **Agent model:** larger and optional, for the AI area (edit by voice,
  chat, research, notes). It is loaded when that area is used and unloaded
  when idle.
- **Switching:** the user can switch which model a job uses.
- D26's "exactly one model" now means one model per role, never the same
  model loaded twice. Dictation keeps priority on the device.
- Model sizes and devices are chosen after benchmarking on the NPU and GPU.

## D29 scope note (2026-10-01)

The owner wants the D29 scope in the full v1 release. The ordered tasks are
in `agent/completion-plan/10-v1-features.md` (WP2c).

## D38 — Refine leaves the side menu (owner, 2026-10-21; amends D23)

**Owner:** asked again why Refine was still in the side menu (an earlier
request had not been recorded anywhere), then chose: move it into
Pipelines.
- The Refine sidebar entry is removed. Its clean-up rules and the choice of
  clean-up model live under the pipeline's Refine stage in Pipelines.
- The Refine module itself stays an extension in the Store (D30).
