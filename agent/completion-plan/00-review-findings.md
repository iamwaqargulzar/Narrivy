# Code review findings (2026-09-28, HEAD `b1e16ce`)

This register lists every issue found in the pre-release review. Each finding
has an ID, severity, location, failure scenario, fix and test. Work packages
refer to findings by ID.

**Severity scale**

- **Critical:** the core promise ("speak, text arrives in the app or is
  recoverable") does not work, or text could be inserted twice or in the
  wrong place. Blocks release.
- **High:** likely to lose a dictation or confuse users in normal use. Fix
  before release.
- **Medium:** a real defect in a less common path, or a packaging/UX issue a
  reviewer will notice. Fix before release unless the owner accepts it.
- **Low:** cleanup. Do it only if it is cheap or reduces risk.

**Before fixing anything:** re-read the cited code. Line numbers were correct
at `b1e16ce` and may have moved. If a finding turns out to be wrong, record
why in `PROGRESS.md` and skip it. Do not "fix" working code to match this
document.

---

## What was reviewed, and what was not

Reviewed in depth:

- VAD and endpointing (`crates/narrivy-core/src/endpoint.rs`);
- the streaming session loop (`crates/narrivy-core/src/recovery.rs`, lines
  664–1100);
- routed delivery and its rungs
  (`adapters/platform-linux/src/delivery/routed.rs`, lines 460–1000);
- production rung composition (`services/narrivyd/src/daemon/session.rs`,
  lines 300–362);
- PipeWire streaming capture (`adapters/platform-linux/src/recovery.rs`,
  lines 700–960);
- IPC authentication and framing (`adapters/ipc-unix/src/security.rs`,
  `server.rs`);
- the GTK/WebKit shell's bridge and navigation policy
  (`apps/narrivy-desktop/src-gtk/src/shell.rs`);
- model import integrity, and journal durability and retention;
- the systemd unit, Debian control file and maintainer scripts;
- the CLI connection path.

Automated scans ran across all production Rust for `unwrap`, `expect`,
`panic!`, lock poisoning and `unsafe`. They found no production `unwrap`.
Three `expect` calls are in `capture.rs` and two in `model-manager/registry.rs`.
`unsafe` appears only in the Linux adapters and in the unused FFI stub.

**Second pass (2026-09-28, same day): the whole repository.** After the first
review, every tracked file was read. The first 218 files in repository order
(root, `.cargo`, `.github`, `crates/`, the first part of `adapters/`) were
read directly. The remaining ~1,150 files went to external reviewers:

- GPT-5.6-Sol at high effort: all product code, scripts, tests, tools and
  packaging, in 27 batches of about 5,000 lines;
- DeepSeek 4.1 Flash: docs, spec, agent state and evidence files;
- two end-to-end traces by Sol: one dictation on the installed package, and
  install → first run → upgrade → remove.

Every Critical and High claim in the new findings below was checked against
the code before being listed, and marked **confirmed** (code read,
reproduced, or compiled) or **native-verify** (needs the real machine). Claims
that turned out false were dropped. The new findings are in
"Second-pass findings" near the end of this file.

The old "not reviewed" list (`session.rs` outside 300–362, `daemon/portal.rs`,
`portal_eis*.rs`, `process-linux`, `resident.rs`, the React front end) is now
covered. T2.12 is replaced by concrete tasks.

**Verified strengths (do not rework these):**

- IPC checks the peer UID with `SO_PEERCRED` and requires a 0700 directory
  and 0600 socket. It caps frames at 1 MiB and connections at 64, and uses a
  50 ms per-client event write timeout.
- Journal writes use a temp file with `sync_all` plus a directory fsync, and
  TTL and count retention.
- Inbox files are mode 0600 and written by atomic rename.
- Model import is digest-verified, uses `renameat`, and rejects symlinked
  cache roots.
- The WebKit view allows only its own asset origin, blocks new windows, and
  denies JavaScript clipboard access. Events are passed as a JSON string
  through `JSON.parse`, so transcript text cannot inject script.
- There is no network client in the product at all.
- The Stop-vs-endpoint race is handled by a first-wins atomic latch
  (`crates/narrivy-types/src/lib.rs`, `SessionFinalization::request`).

---

## Critical

### F-C1 — The shipped package never types text into applications

- **Where:** `packaging/deb/narrivy-daemon.service:11,15` (and the two copies
  that a test requires to be byte-identical:
  `packaging/linux/deb/narrivy-daemon.service`,
  `packaging/recovery/narrivy-daemon-candidate.service`).
- **What:** the unit sets `NARRIVY_RECOVERY_MODE=linux-mic-inbox` and
  `NARRIVY_DELIVERY_BACKEND=inbox`. The routed lane that uses the portal and
  `wtype` is `linux-mic-routed-inbox` / `routed-inbox`
  (`services/narrivyd/src/daemon/policy.rs:62,460,518`).
- **Scenario:** a user installs the `.deb`, dictates, and nothing appears in
  the focused app. The text lands only in `$XDG_RUNTIME_DIR/narrivy/inbox/`.
- **Fix:** after F-C2 and F-C4 are fixed, switch all three unit files to
  `NARRIVY_RECOVERY_MODE=linux-mic-routed-inbox` and
  `NARRIVY_DELIVERY_BACKEND=routed-inbox`. Keep them byte-identical. Check that
  startup validation accepts the combination (ADR-0072 §5).
- **Test:** extend `tests/packaging/test_build_deb_candidate.py` (or add a
  governance test) to assert the shipped unit selects the routed lane. Native
  proof comes from FUN-004/TEST-004 in the campaign.

### F-C2 — The delivery ladder keeps going after text was already sent (double insertion)

- **Where:** `adapters/platform-linux/src/delivery/routed.rs:582–676`
  (`impl DeliveryPort for RoutedThenInboxDelivery`, the `for rung in
  self.rungs.iter_mut()` loop).
- **What:** on `RungOutcome::SubmittedUnverified` (line 642), `CopiedOnly`
  (650) and non-policy `Failed` (658), the code sets `dispatched = true` and
  **continues to the next rung**. Production composes the portal/EIS rung
  first and `wtype` second (`session.rs:338–361`). The portal rung *always*
  returns `SubmittedUnverified` (ADR-0072 §3).
- **Scenario:** once a target binding exists (F-C3) and the user has granted
  keyboard permission, the portal types the text, then `wtype` types it
  **again**. A `wtype` failure part-way through typing is followed by any
  later rung typing it once more. Today this is masked only because
  production never binds a target, so the portal rung is always skipped.
- **Fix:** after the first outcome that sets `dispatched = true`, `break` out
  of the rung loop. Then do the Inbox recovery write, as the code already does
  when `dispatched` is true. Update the doc comment at the top of `deliver`.
- **Test:** add a unit test in `routed.rs` tests. Use a bound target and
  permission; the first rung returns `SubmittedUnverified`, the second is a
  recording rung that would return `Committed`. Assert the second rung's
  dispatched list is empty, the outcome is `InboxFallback`, and the attempts
  are exactly `[first, inbox]`. Add the same test with the first rung
  returning `Failed(BackendError)`.

### F-C3 — No text insertion path exists on GNOME — **moved to post-release side quest SQ1** (owner decision 2026-09-28)

> The first release is Pop!_OS COSMIC only, so this no longer blocks release. It is kept here for the record and is implemented in `07-post-release-side-quests.md` SQ1. Do **not** do it before the Pop!_OS release.

- **Where:** production binds `pre_activation_target_binding_absent`
  (ADR-0072 §2), so `PortalEisTextRung::requires_target()` (`routed.rs:771`)
  skips the portal rung everywhere. The only other rung is `wtype`. GNOME
  Mutter does not implement the `virtual-keyboard-unstable-v1` protocol that
  `wtype` needs. Clipboard rungs are declared unavailable
  (`session.rs:354–360`).
- **Scenario:** on the GNOME reference lane every dictation goes only to the
  Inbox. (Before 2026-09-28, TEST-010 required GNOME; it no longer does.)
- **Fix (in SQ1, after release):** the
  recommended option is to let the portal/EIS rung run without a pre-bound
  target, with the same "type into whatever is focused at delivery" semantics
  the `wtype` rung already has. Keep it gated on an active attended grant.
  Write an ADR superseding the relevant part of ADR-0072. F-C2's fix is a
  prerequisite, or text doubles on COSMIC.
- **Test:** unit tests for rung qualification with an unbound target and an
  active grant, plus native GNOME evidence in the campaign.

### F-C4 — `wtype` is not a package dependency

- **Where:** `scripts/build_deb_installable.py:98–100`, `packaging/deb/control`.
- **Scenario:** on a fresh Pop!_OS or Ubuntu system `wtype` is absent.
  `SystemWtypeKeyboardRung::new` reports `wtype_missing`, and COSMIC users get
  Inbox-only delivery with no explanation in the install flow.
- **Fix:** add `wtype` to `Recommends:` (or `Depends:` if the owner prefers).
  The rung already accepts only a root-owned `/usr/bin` or `/bin` binary, so
  the packaged one qualifies. Make the Diagnostics/Home screens show a clear
  remediation ("install wtype") when the rung reports `wtype_missing`.
- **Test:** packaging test asserting the control field. A frontend test for
  the remediation text when the capability reports `wtype_missing`.

---

## High

### F-H1 — The capture queue holds only 640 ms; a short stall kills the dictation

- **Where:** `adapters/platform-linux/src/recovery.rs:57–58`
  (`STREAM_CAPTURE_CHUNK_SAMPLES = 1_280`, `STREAM_CAPTURE_QUEUE_CHUNKS = 8`)
  and `send_stream_chunk` (line 925). On `TrySendError::Full` it sets
  `Backpressure` and stops.
- **What:** 8 × 1,280 samples at 16 kHz is 640 ms. Capture starts *before* the
  ASR stream opens (`core/recovery.rs:706`). The consumer then runs VAD,
  `asr.push_audio` over a pipe to the worker, and event draining for each
  chunk.
- **Scenario:** a busy CPU, a slow resident-worker admission, or a GC pause in
  the desktop stalls the session thread for more than 0.64 s. The producer
  hits a full queue and the session fails with a backpressure code. The
  user's speech is lost and nothing is typed.
- **Fix:** raise the queue to at least 10 s of audio (for example 128 chunks
  ≈ 10.2 s ≈ 655 KB of `f32`). Keep the fail-closed behaviour for real
  overload. Document the memory bound next to the constant.
- **Test:** a unit test with a consumer that sleeps 2 s before reading, which
  must still receive every chunk in sequence.

### F-H2 — If the microphone source ends mid-dictation, all captured speech is discarded

- **Where:** `crates/narrivy-core/src/recovery.rs:1006–1015` (streaming) and
  `:490` (batch). `CaptureEnd::SourceEnded` means `terminate_active(...,
  "capture_source_ended")`, and no text is committed.
- **Scenario:** the USB mic is unplugged, PipeWire restarts, or `pw-record`
  exits after 40 s of dictation. All 40 s are lost. This contradicts the
  product pillar "text reaches the intended place or is recoverable".
- **Fix (recommended):** on `SourceEnded` with `total_samples` above a small
  minimum, finalize the ASR stream with the audio already pushed. Commit the
  result through the normal tail with delivery forced to Inbox-only. Journal
  it with `capture_end = SourceEnded` so the UI can say "microphone
  disconnected; text saved". If this takes more than about a day, ask the
  owner whether to ship it as a documented known limitation instead
  (**D6**).
- **Test:** a core unit test with a capture that ends with `SourceEnded`
  after N chunks. It must produce an `InboxRecovered` terminal with the
  transcript. The existing test
  `batch_source_ended_journals_reason_and_fails_before_any_tail_side_effect`
  must be updated deliberately, not deleted silently.

### F-H3 — Dictation stops by itself after 1.5 s of silence (contradicts the documented activation modes)

- **Spec:** activation has exactly two documented modes (`docs/product/requirements.md`
  FUN-001, FUN-031; `docs/platforms/linux-global-shortcut-matrix.md`):
  - **toggle:** press the key, speak, press the key again to stop;
  - **press-and-hold:** hold the key while speaking, release it to stop.

  Neither mode ends on silence. The owner confirmed this on 2026-09-28.
- **Where:** `services/narrivyd/src/daemon/session.rs:3364–3376` always
  installs `NativeEnergyVad` plus `DictationEndpointer`
  (`trailing_silence_ms: 1_500`, `endpoint.rs`) for streaming sessions.
  `core/recovery.rs` then calls `request_endpoint_finalization` and ends
  capture.
- **Scenario:** the user presses the key, says a sentence, pauses 2 s to think,
  and keeps talking. The session has already finalized and the rest is lost.
- **Fix:** do not install endpointing for dictation sessions (pass
  `endpointing: None`). A session ends only on the user's stop (second toggle
  press, or key release in hold mode), on cancel, or on the 300 s safety limit
  (`MAX_CAPTURE_SECONDS`). Keep the VAD/endpoint code for a future, explicitly
  selected hands-free pipeline, but do not use it by default.
- **Test:** a daemon/core test where a session with a 3 s silence gap stays
  listening and finalizes only on Stop. An existing endpoint-driven test that
  asserts auto-finalization must be changed deliberately, not deleted.

### F-H4 — The hotkey fails silently when the daemon isn't running

- **Where:** `apps/narrivy-cli/src/main.rs:213` (`connect`). The error goes to
  stderr, and exit code 1 is invisible from a keyboard shortcut. The daemon is
  only started by the desktop app (`src-gtk/src/shell.rs:266–267`,
  `systemctl --user start`), or at login if the unit is enabled.
- **Scenario:** after a reboot, the user presses the Narrivy shortcut before
  opening the app. Nothing happens and there is no feedback.
- **Fix:** in `narrivy-cli toggle`, if the socket is missing or refuses the
  connection, run `/usr/bin/systemctl --user start narrivy-daemon.service`
  (fixed absolute path, bounded timeout, like `shell.rs:280`). Wait for the
  socket with a bounded retry, then send the toggle. If it still fails, send
  a desktop notification through `org.freedesktop.Notifications` with a
  content-free message ("Narrivy could not start. Open Narrivy for details.").
- **Test:** CLI unit tests with an injected connector: first attempt fails,
  start is called once, the retry succeeds; and a timeout produces a single
  notification.

### F-H5 — The user unit can start before the graphical session exists

- **Where:** `packaging/deb/narrivy-daemon.service:3,53`: `WantedBy=default.target`
  with `After=graphical-session.target`, but no `PartOf`/`Requisite`.
- **Scenario:** if the unit is enabled, it starts at user-manager start, before
  the compositor exports `WAYLAND_DISPLAY`. `wayland_text_input_supported()`
  (`routed.rs:830`) reads the daemon's environment once per composition.
  `wtype` then reports `wayland_session_absent` until the daemon restarts. It
  also keeps running after logout into a dead session.
- **Fix:** use `WantedBy=graphical-session.target`,
  `PartOf=graphical-session.target` and `After=graphical-session.target
  pipewire.service`. Update all three copies. Decide with the owner (**D5**)
  whether the package enables it for all users
  (`systemctl --global enable narrivy-daemon.service` in `postinst`, reversed
  in `prerm`/`postrm`).
- **Test:** `systemd-analyze verify` (already used), plus native login and
  logout observation in the campaign.
- **Second pass, confirmed on the reference machine's boot log**
  (`journalctl --user -b -o short-monotonic`, 2026-09-28):
  - the user manager reached `default.target` at 8.07 s and skipped the
    installed (old 0.2.3) unit because `ConditionEnvironment=WAYLAND_DISPLAY`
    was not met;
  - `cosmic-session` started at 324.6 s;
  - `graphical-session.target` and `cosmic-session.target` were reached at
    325.3 s.

  The current unit drops that condition but keeps
  `WantedBy=default.target`. So, once enabled, it would start about five
  minutes before the Wayland session exists and never get `WAYLAND_DISPLAY`.
  `process-linux` copies the variable from the daemon's own environment
  (`adapters/process-linux/src/lib.rs:1969`). `graphical-session.target` is
  active on COSMIC, so the fix above is confirmed to be the right one. Native
  check: after a reboot and login, read `/proc/$(pgrep -x narrivyd)/environ`
  and confirm it contains `WAYLAND_DISPLAY`, then dictate once without
  restarting anything.

### F-H6 — The systemd user unit has never run natively

- **Where:** handoff notes: the Docker lifecycle "deliberately does not qualify
  … systemd user-service behavior", and the native slices ran the daemon
  directly.
- **Risk:** `ProtectHome=read-only`, `PrivateTmp=true`,
  `ProtectSystem=strict` and `CapabilityBoundingSet=` in a *user* unit depend
  on unprivileged user namespaces. The `Unset/Environment` lines change what
  the daemon sees. Any of these can break startup, the portal D-Bus, or
  `wtype` access to the Wayland socket.
- **Fix:** before the campaign, install the candidate on COSMIC. Start it with
  `systemctl --user start narrivy-daemon`, dictate once through each rung,
  then `stop`, `restart`, log out and back in. Fix whatever breaks. (GNOME is
  checked later, in side quest SQ1.)
- **Test:** this becomes part of the OPS-002/TEST-009 native procedures.

---

## Medium

### F-M1 — VAD mixes up samples and frames for multi-channel audio

- **Where:** `crates/narrivy-core/src/endpoint.rs:277`. `window_samples =
  bind_rate(...)` is a **frame** count, but the loop at 278–286 counts
  interleaved **samples**. `StreamingEndpointing::observe` then tells the
  endpointer each decision covers `window_frames` frames.
- **Scenario:** with stereo input, every "20 ms" decision covers 10 ms, so
  the 1.5 s hang-over fires after 0.75 s and truncates speech. Production
  capture is mono (`capture.rs:55`, `CAPTURE_CHANNELS`), so this is latent.
- **Fix:** either multiply the window by `chunk.channels` (downmix by
  averaging each frame), or reject `channels != 1` with a typed error.
  Rejecting is simpler.
- **Test:** a stereo chunk test that fails before the fix.

### F-M2 — A rejected VAD chunk can leave state half-updated

- **Where:** `endpoint.rs:237–241`. `vad_chunk_too_large` is raised inside
  `decide_window` after earlier windows of the same chunk already mutated the
  floor and `active` state. This contradicts the comment at 268–270.
- **Fix:** check `chunk.samples.len() / window_samples <= MAX_ACTIVITY_DECISIONS`
  up front, before mutating anything.
- **Test:** extend `energy_vad_rejected_chunk_leaves_state_untouched`.

### F-M3 — Recovered text sits on tmpfs and has no UI — **raised to High by the second pass**

Second pass, confirmed: the Inbox is `runtime_dir.join("inbox")`
(`services/narrivyd/src/main.rs:482`), under the unit's
`RuntimeDirectory=narrivy`, which systemd removes when the service stops and
at logout. Startup replay only replays `Committed` and `FailedRecoverable`
journal records (`crates/narrivy-core/src/recovery.rs:126-133`), not
`InboxRecovered` ones. The journal deletes everything after 15 minutes
(`crates/narrivy-journal-api/src/lib.rs:12`). So every piece of recovered
text is lost at logout. Fix together with D6/T2.8.

- **Where:** `services/narrivyd/src/main.rs:482` puts the Inbox under
  `runtime_dir`, which is `$XDG_RUNTIME_DIR/narrivy` (tmpfs, cleared at
  logout). No desktop screen lists Inbox items; `History` shows only the
  optional retained history.
- **Scenario:** delivery fails, text goes to the Inbox, the user logs out
  before copying it, and it is gone from the Inbox. The journal still has it
  until TTL, but nothing shows it. Users are told their text is "saved" but
  cannot find it.
- **Fix:** owner decision **D6**. The recommended option is to move the Inbox
  to the state dir (`$XDG_STATE_HOME/narrivy/inbox`, already 0700 via
  `StateDirectory`) with the same TTL as the journal. Add a minimal
  "Recovered text" list, with copy and delete, to the Home or History screen,
  over a small new IPC command.
- **Test:** daemon test that Inbox files survive a daemon restart with a new
  runtime dir. Frontend test for the list, copy and delete.

### F-M4 — Every typed dictation is also saved as a plaintext Inbox file

- **Where:** `routed.rs:678–690`. Any outcome other than `Committed` writes an
  Inbox copy, and `wtype` and the portal can never report `Committed`.
- **Scenario:** a user dictating a password reset email or medical notes
  finds a full plaintext log in the Inbox. The Privacy screen doesn't mention
  it.
- **Fix:** this is by design for unverified delivery. Keep it, but disclose it
  on the Privacy/Settings screen, apply the journal TTL to Inbox files, and
  offer "clear recovered text". Tie this to F-M3.

### F-M5 — The WebKitGTK version floor may not install on stock Ubuntu 24.04 — **moved to side quest SQ1**

> Pop!_OS already ships 2.52, so this does not affect the first release. Handle it in SQ1 before claiming Ubuntu support.

- **Where:** `scripts/build_deb_installable.py:98`,
  `libwebkitgtk-6.0-4 (>= 2.52)`. The developer host has 2.52.6 (Pop!_OS).
- **Fix:** on the Ubuntu machine, run `apt-cache policy libwebkitgtk-6.0-4`.
  Set the floor to the lowest version that has the WebKit6 API the shell uses
  (`webkit6 = 0.6.1`) and is available in `noble-updates`. Record the
  evidence in `agent/RESEARCH_LOG.md`.

### F-M6 — Package metadata problems

- **Where:** `scripts/build_deb_installable.py:90–100`, `packaging/deb/control`.
- **What:**
  - `Build-Using:` is not a valid Debian binary-package field (the real field
    is `Built-Using`, and it is for embedded source, not the compiler);
  - `Homepage: https://narrivy.local` and `release@narrivy.local` are
    placeholders;
  - the version `0.1.0~rcN` sorts **below** the `0.2.3` already installed on
    the reference machine, so apt treats it as a downgrade.
- **Fix:** drop `Build-Using`. Use a real homepage and maintainer (owner
  supplies them; see **D3**). Adopt the version scheme from **D3**, for
  example `1.0.0~rc1`.
- **Test:** `lintian` on the built `.deb` if available, and a packaging test
  for the fields.

### F-M7 — The default model is the least accurate one

- **Where:** unit `NARRIVY_TRANSCRIBE_MODEL_ID=whisper-tiny.en-Q8_0`.
- **Scenario:** tiny.en is Whisper's weakest model. The known-phrase miss in
  P11 history may be partly accuracy. Users will judge the product on it.
- **Fix:** owner decision **D4**. Measure `base.en` and `small.en` (quantized)
  with the existing CPU performance recorder on this machine. If `base.en`
  keeps release-to-committed latency under target, ship it as the default.

---

## Low (cleanup)

### F-L1 — The dead FFI stub contains unverified `unsafe` code

- **Where:** `engines/transcribe-cpp/transcribe-sys/`. Its own docs say its
  declarations are "hypotheses"; it is excluded from every build, and the
  worker uses the upstream `transcribe-cpp = 0.2.3` crate. `TcppContext`
  stores a raw model pointer with no lifetime tie to `TcppModel`, so it could
  outlive it. `unsafe impl Send/Sync` are marked "assumptions flagged for P5
  confirmation".
- **Fix:** delete the directory and its references (`sidecar.rs` and
  `lib.rs` doc comments, `BUILD-NOTES.md`). Reseal the manifest.

### F-L2 — The legacy delivery router is unused in production

- **Where:** `adapters/platform-linux/src/delivery/router.rs`
  (`route_and_deliver`, `DeliveryRouter`). It uses PATH-resolved helpers
  through `StdCommandRunner`, which ADR-0072 rejected for production.
- **Fix:** confirm with `rg` that no production composition uses it, then
  delete it or make it `#[cfg(test)]`-only, so nobody wires it back in.

### F-L3 — Very large files

`session.rs` (9k lines), `resident.rs` (5.8k) and `core/recovery.rs` (5.5k)
are hard to review. **Do not refactor before release.** Record a post-1.0
item.

---

## Second-pass findings (whole repository, 2026-09-28)

Status tags:
- **confirmed:** code read, reproduced, or compiled during the review;
- **native-verify:** only the reference machine can settle it (put it in the
  WP3 smoke).

Line numbers are at the working tree of 2026-09-28 (HEAD `9099b5b` plus
uncommitted plan changes).

### Critical

#### F-C5 — The bundled model can never be installed by a normal user — confirmed

- **Where:** `adapters/platform-linux/src/model_import.rs:575` rejects any
  source whose `uid()` differs from the daemon's own UID. The packaged model
  `/usr/share/narrivy/models/whisper-tiny.en-Q8_0.gguf` is root-owned 0644
  (`scripts/build_deb_installable.py:425,479`). The desktop sends that fixed
  path (`apps/narrivy-desktop/src-tauri/src/bridge.rs:126,224`).
- **Scenario:** a user clicks "Install model". The result is
  `failed/source_rejected`, the daemon stays `awaiting_model`, and no
  dictation is possible.
- **Why evidence missed it:** `scripts/qualify_candidate.py` runs the
  container as root, so the UIDs matched. `BundledModelInstall.test.tsx`
  mocks the success.
- **Fix:** add a packaged-source path for the one fixed bundled file only. It
  requires owner uid 0, mode exactly 0644, symlink-free root-owned ancestors
  that group and others cannot write, and the registry-pinned size and digest.
  Keep the same-user rule for user-chosen files.
- **Test:** a unit test with an injectable "trusted system owner" and a
  root-owned fixture (or an injected owner map). The qualification provision
  step runs as an unprivileged user (`docker exec --user 1000`). Native smoke:
  install the `.deb`, click Install model, reach Ready.

#### F-C6 — Installed mode can never complete a dictation — confirmed

- **Where:** `services/narrivyd/src/daemon/session.rs:3237`. The strict worker
  does `let Some(model) = job_model_path.clone() else { … "model_unavailable";
  return }` before `if job_resident_mode` (line 3241).
  - `job_model_path` is `engine_config.transcribe_model_path` (line 3158).
  - Installed mode forbids that path (`policy.rs:628-635`), and the unit sets
    `UnsetEnvironment=NARRIVY_TRANSCRIBE_MODEL`.
  - The installed model is bound into the resident engine state
    (`main.rs:531-622`), never into `engine_config`.
- **Scenario:** with the model installed (after F-C5 is fixed), every toggle
  prints `Started`. The worker then fails at once with `model_unavailable`:
  no capture and no text.
- **Why evidence missed it:** no test runs a dictation in installed mode; only
  `policy.rs` parse tests cover it. `qualify_candidate_known_phrase.py:1043`
  uses `NARRIVY_TRANSCRIBE_RUNTIME=resident` with an explicit model path.
  `qualify_candidate.py` (installed mode) never dictates.
- **Fix:** require `job_model_path` only in the non-resident (sidecar) `else`
  branch.
- **Test:** a session-level test using an installed-mode `EngineConfig` and a
  controlled resident engine that asserts a Final is produced. WP5 native
  evidence must dictate through the installed unit, never a hand-started
  daemon.

#### F-C7 — Seven audio requirements would be "proven" on code the product never runs — confirmed

- **Where:** `adapters/platform-linux` `audio_policy` (duck/restore),
  `capture::PreRollRingBuffer`, `capture::AudioPolicyJournal`, `rank_mics`
  and the hotplug parser, and `pipewire_monitor::LiveCapture`. They are used
  only by `examples/p4_*` probes. `rg "audio_policy::|PreRollRingBuffer|LiveCapture" services apps`
  finds nothing. The shipped daemon captures through
  `adapters/platform-linux/src/recovery.rs` (`pw-record` from the default
  source) with no pre-roll, no ducking, no mic ranking or override, and no
  hotplug handling.
- **Requirements affected** (all `required`, all `blocked`): FUN-006
  (pre-roll), FUN-007 (leave plus a proven duck/pause), FUN-011 (mic ranking,
  override, hotplug, fallback), FUN-012 (new streams inherit the policy),
  REL-005 and REL-009 (duck restoration), PERF-006 (real-time capture thread;
  its structural gate audits the unused `pipewire_monitor` loop).
  `record_native_audio_policy_evidence.py` drives the probes, so a
  "successful" campaign would qualify features users don't have. The unused
  code also has its own bugs (Sol, B38): ducking mutates the volume before the
  journal is durable, it records `@DEFAULT_AUDIO_SINK@` instead of a stable
  sink, `restore_own` deletes the whole journal, repeated ducks restore the
  wrong baseline, and the probe prints `ok: true` on a stop error.
- **Fix:** owner decision **D14**. Whatever is kept for 1.0 must be wired into
  the daemon's capture path and qualified there. PERF-006 must measure the
  `recovery.rs` capture thread.

#### F-C8 — `wtype` on COSMIC: conflicting reports — native-verify

One February 2026 write-up saw `wtype` output arrive as key numbers on
COSMIC. An OpenWhispr maintainer and the owner's Handy fork report that it
works. Narrivy has no evidence either way. The check (5 minutes, first item
of the WP3 smoke) and both outcomes are in
`09-cross-platform-architecture.md` §3. If it fails, it is Critical: `wtype`
is the only 1.0 typing route.

### High

#### F-H7 — The daemon is unreachable for 10–40 s after every start — reproduced/confirmed

- **Where:**
  - `adapters/platform-linux/src/compute_probe.rs:96-115` waits for the child
    to exit before reading stdout. `ldconfig -p` prints about 95 KB on the
    reference machine, more than a 64 KiB pipe holds, so the child blocks
    until the 10 s `PROBE_TIMEOUT` kills it. A Python equivalent reproduced
    the hang. The result is treated as an empty listing, so OpenVINO always
    reads as absent.
  - The host has `/dev/accel0`, so `services/narrivyd/src/daemon/compute.rs:156-164`
    runs the probe, then probes OpenVINO again: two timeouts.
  - Startup binds the socket at `main.rs:442`, then does model verification
    and resident load (`main.rs:568-621`, allowed up to 30 s by
    `resident.rs:55`), then compute (`main.rs:688`), and only then serves
    (`main.rs:695`). The CLI handshake timeout is 5 s
    (`adapters/ipc-unix/src/client.rs:26`); the desktop waits 3 s
    (`shell.rs:35`).
- **Scenario:** the first shortcut after login, restart or upgrade times out.
  The desktop shows "daemon unavailable".
- **Fix:**
  - drain stdout on a thread, or `read_to_end` under the deadline, and cap the
    bytes;
  - build the compute plan once from the discovery report;
  - run optional compute discovery after `serve` starts and publish the
    capability when it's done;
  - either finish mandatory startup before `bind`, or answer the handshake
    with a typed "starting" state that clients retry on within a budget.
- **Test:** a fake program writing 1 MiB completes quickly; a daemon
  integration test answers a handshake within 1 s of the socket appearing.

#### F-H8 — Any non-ASCII text fails in `wtype` — reproduced

- **Where:** `adapters/process-linux/src/lib.rs:53,1969`. The Wayland helper
  environment is cleared except `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`.
  `wtype` 0.4 (the Pop!_OS package, `0.4-3`) calls `setlocale` and
  `mbstowcs`.
- **Reproduction:** a C program under `env -i` gets locale `C`, and
  `mbstowcs("café")` and `mbstowcs("it’s")` both return -1. With
  `LC_ALL=C.UTF-8` they return 4.
- **Scenario:** a transcript containing é, ’, — or … is not typed, or is only
  partly typed, and falls back to the Inbox.
- **Fix:** add a constant `LC_ALL=C.UTF-8` to the `WaylandTextInput`
  environment (a constant, not inherited).
- **Test:** a process-backed test with a fake `wtype` that reports its locale
  or `mbstowcs` result. Native smoke phrase: "café — it’s done".

#### F-H9 — Clean mode (the default) corrupts ordinary text — confirmed

- **Where:** `services/narrivyd/src/correct.rs:47`. A space is inserted after
  every `, . ; : ! ?` that is followed by a letter or digit, then
  `capitalize_sentences` uppercases the next letter. The ITN table
  (`correct.rs:14-28`) replaces every standalone number word. Clean is the
  unit default (`NARRIVY_DEFAULT_PIPELINE=clean`).
- **Examples:**
  - `3.14` → `3. 14`
  - `example.com` → `example. Com`
  - `e.g.` → `E. G.`
  - `10:30` → `10: 30`
  - `1,000` → `1, 000`
  - "no one" → "No 1"
  - "one day" → "1 day"
- **Fix:** only add a space after punctuation when a letter precedes it and a
  letter follows it, outside URL, email, number and abbreviation tokens (or
  drop rule 2: whisper already spaces punctuation). For ITN, see **D12**
  (recommended: drop ITN for 1.0).
- **Test:** table tests for every example above, plus the existing ones.

#### F-H10 — The release daemon's tests don't compile; CI doesn't run them — confirmed

- **Where:** `cargo check -p narrivyd --features asr-transcribe-resident
  --tests` fails with six E0061 errors at `services/narrivyd/src/daemon/policy.rs:2015,
  2044, 2073, 2099, 2114, 2140`: `capability_strings_with_resident` now takes
  three arguments.
- **Gaps:**
  - CI (`.github/workflows/ci.yml`) only runs `cargo check` for this feature.
  - The Python job runs only some governance modules.
    `test_evidence_measurements.py`, `test_phase_report_generation.py` and
    `LiveFrontendGuardTests` never run in CI.
- **Fix:** pass `None` as the third argument. Add
  `cargo test -p narrivyd --locked --features asr-transcribe-resident` and
  the full `python3 -m pytest -q tests` to CI and to the WP exit checks.

#### F-H11 — The overlay may take keyboard focus while text is being typed — native-verify (Critical if true)

- **Where:** `apps/narrivy-desktop/src-gtk/src/overlay.rs:205-216`. The
  overlay is a normal `gtk::Window` toplevel. `focusable(false)` is a GTK
  widget property; it doesn't stop the compositor from activating a newly
  mapped toplevel. The overlay is visible from Listening through Processing,
  which is when `wtype` types.
- **Scenario:** if COSMIC focuses the overlay, the text goes into the overlay
  and is lost. The report says `SubmittedUnverified`, and only the Inbox copy
  exists. The overlay also has no placement or always-on-top guarantee.
- **Check first (WP3 smoke):** with the shell running and COSMIC Text
  focused, dictate. Confirm focus never leaves the text editor and the text
  lands there.
- **Prior art:** upstream Handy 0.9.6+ uses a GTK3 `gtk-layer-shell` overlay on Wayland, and the owner's Handy fork solved it on COSMIC: a
  native GTK layer-shell card that never takes focus, placed on the right
  monitor through a 1×1 probe surface
  (`Handy-Pop-OS/docs/2026-08-01-overlay-changes-investigation.md`).
  It is owner-authored, so it can be adapted.
- **Owner direction (2026-09-29):** follow the Handy approach, which is already binding in `docs/product/ux/overlay.md`. Implement it as WP2 **T2.18** whether or not the focus steal reproduces.
- **Fix if confirmed:** a layer-shell overlay (`gtk4-layer-shell`; COSMIC
  supports wlr-layer-shell) with `keyboard_interactivity = none`, or hide the
  overlay before delivery starts.

#### F-H12 — The UI ignores all events after the daemon restarts — confirmed

- **Where:** `apps/narrivy-desktop/src/state/SessionContext.tsx:175-184`
  never resets `lastEnvelopeSeq`. The server's `event_seq` starts at 0 for
  each daemon process (`adapters/ipc-unix/src/server.rs:286`).
- **Scenario:** after a crash restart or an upgrade, the UI drops every event
  until the new counter passes the old one, which can take many dictations.
  Controls look frozen.
- **Fix:** send a connection generation from `session_events.rs` on reconnect
  and reset the sequence when it changes. The GTK overlay reducer already
  handles this (`overlay.rs:83-97`).
- **Test:** a high sequence, then sequence 1 after a reconnect, is accepted.

#### F-H13 — About 16 Settings controls change nothing — confirmed

- **Where:**
  - `apps/narrivy-desktop/src/lib/settings.ts` `FIELD_SPECS` and
    `Settings.tsx:190-200` (`GenericSection`) only write browser storage.
  - `useDictationActions.ts:100` sends only the recognition settings.
  - `Home.tsx:130` then presents these values as "custom" choices.
  - Affected controls: launch at startup, copy on finalize, pause on idle,
    compute device and threads, model, delivery mode paste/copy, paste
    delay, input device, noise gate, punctuation, spoken punctuation, privacy
    retention "session only", local only, log level.
- **Pipeline import/export as well:**
  - `Settings.tsx:117-119` writes to `$XDG_RUNTIME_DIR/narrivy/pipelines`
    (`bridge.rs:411-417`), which is tmpfs.
  - The daemon lists and admits only hard-coded pipelines (`session.rs:4008,
    4332`) and never reads that registry (`policy.rs:1594,1676`).
- **Fix for 1.0:** remove or disable every control that isn't wired to the
  daemon, and hide pipeline import/export. Add a test that every rendered
  setting maps to a daemon command or is read-only status.

#### F-H14 — The status pill never shows "recording" — confirmed

- **Where:** `StatusPill.tsx:4` and `hooks/usePerfMarks.ts:7-13` check
  `phase === 'recording'`, but the daemon sends `listening`
  (`session.rs:139`). Only the fake bridge emits `recording`.
- **Scenario:** a live microphone looks like a generic "busy" state. The
  start-to-listening performance mark never fires in the real app.
- **Fix:** use `listening` everywhere. Add a contract test that the fake
  bridge's states are a subset of the daemon's `SessionState` strings.

#### F-H15 — Uninstall and upgrade leave the old daemon running — confirmed

- **Where:**
  - `packaging/linux/deb/prerm:3-7` does nothing on `remove` or `upgrade`.
  - `postinst:5-18` doesn't reload or restart anything.
  - `postrm purge` prints a cleanup command that leaves out
    `~/.local/state/narrivy`, which holds the journal and History.
  - `postinst` calls a nonexistent `/usr/bin/narrivy-migrate` (harmless
    `|| true`).
- **Scenario:** after `apt remove`, the daemon keeps running and writing
  state. After an upgrade, every logged-in user keeps the old daemon until
  logout, and a new UI may talk to an old daemon.
- **Fix (with D5):**
  - `prerm remove`/`upgrade`: for each active user manager, stop the unit
    (e.g. `systemctl --user -M "$user@" stop narrivy-daemon.service`, bounded;
    or use `deb-systemd-invoke --user` if available on noble). Never touch
    home directories.
  - `postinst configure`: **no** global enable (D5, owner 2026-09-29: off by
    default, opt-in in the app); a bounded `daemon-reload` plus a restart of
    active user instances.
  - Fix the purge message to include the state directory.
- **Test:** packaging tests on the script text. WP3 native: upgrade while the
  daemon runs, then confirm the daemon build changed; remove, then confirm no
  `narrivyd` process remains.

#### F-H16 — A unit test can type into your focused window — confirmed code, not executed

- **Where:** the `services/narrivyd/src/daemon/session.rs:8298` test builds
  the real routed ladder (`compose_strict_routed_delivery_with`, which
  includes the real `SystemWtypeKeyboardRung` from line 351), with
  `FixedPermission{keyboard_insert: true}`. It then delivers
  "typed unavailable target".
- **Scenario:** `cargo test` run from a COSMIC terminal (with
  `WAYLAND_DISPLAY` set and `/usr/bin/wtype` present) can type that text
  into the focused window, including during the owner's evidence runs. Not
  executed during the review, on purpose.
- **Fix:** tests use a controlled wtype rung. Add a governance check that no
  test composes a `System*Rung`, or clear `WAYLAND_DISPLAY` in the test
  harness.

#### F-H17 — History is on by default, and its storage has three defects — confirmed

- **Where:**
  - `main.rs:417` always composes History. `retained_history.rs:59` defaults
    retention to 7 days, and the unit sets no `NARRIVY_HISTORY_RETENTION`.
    Every transcript is kept for 7 days in `history.db`, as well as in the
    Inbox. See **D11**.
  - `retained_history.rs:344-362`: above 200 rows, every dictation runs
    `clear_all()` and then re-inserts 200 rows in separate commits. A crash
    in between erases History.
  - Expired rows stay readable in SQLite free pages and in the WAL (no
    `secure_delete`, no checkpoint).
  - Privacy zones never match: the target binding is always unavailable
    (`session.rs:376,387`), and no UI or CLI can create a zone. The
    `history_retention_composition.rs:449` test injects an app ID that
    production never has.
- **Fix:**
  - one-statement compaction in a transaction;
  - `PRAGMA secure_delete=ON` plus `wal_checkpoint(TRUNCATE)` after pruning;
  - don't claim privacy zones (PRIV-003) for 1.0;
  - apply the owner's D11 default.

#### F-H18 — Crash dumps can capture audio and transcripts; the ADRs call this a release blocker — confirmed

- **Where:**
  - ADR-0035:57, ADR-0036:65, ADR-0037:60 and ADR-0047:68 each say
    "core-dump/swap containment … remain explicit release blockers";
  - no code or unit addresses it: `rg "PR_SET_DUMPABLE|MADV_DONTDUMP|LimitCORE|mlock"`
    finds nothing;
  - on Pop!_OS, `systemd-coredump` or apport can store a crashed
    `narrivyd`'s or worker's memory (audio buffers, transcripts) on disk.
- **Fix:**
  - `LimitCORE=0` in the user unit;
  - `prctl(PR_SET_DUMPABLE, 0)` early in `narrivyd` and in
    `narrivy-transcribe-worker` (both already have narrow `unsafe`/libc
    boundaries in the Linux adapters; keep it there);
  - verify the worker inherits it, or set it again;
  - swap can't be fully prevented without locking all buffers. Document the
    residual risk in `docs/user/privacy.md` (swap is encrypted when full-disk
    encryption is on) and amend the ADRs' "release blocker" wording to "core
    dumps: contained; swap: documented residual".
- **Test:**
  - a unit or integration check that `/proc/self/status` shows
    `CoreDumping`/dumpable off after startup;
  - native: `systemctl --user show narrivy-daemon -p LimitCORE` shows 0;
  - `kill -SEGV` the worker in a scratch run and confirm no core appears in
    `coredumpctl list`.

### Medium

- **F-M8** Failure is invisible in the overlay. `session.rs:2645-2646`
  publishes `Failed` and then `Idle` back to back; the shell drains both in
  one pass (`shell.rs:591-619`), and `Idle` hides the overlay. Keep the failed
  state until a timeout.
- **F-M9** A full UI queue stops events forever. At `shell.rs:112-116`,
  `TrySendError::Full` maps to `Stop`, and `session_events.rs:48-51` returns
  permanently. Fix: coalesce partials, and retry or block for terminal events.
- **F-M10** Status polls stack up. The 750 ms `setInterval` in
  `StatusContext.tsx:125-158` fires without waiting for the previous poll, so
  requests pile up behind the GTK shell's single worker. With F-H7 this blocks
  Start and Cancel. Use a single-flight poll.
- **F-M11** First run is a dead end:
  - onboarding has no model-install step (`Onboarding.tsx:121-145`), so Start
    stays disabled on a clean install;
  - `Models.tsx:68` hides the typed failure reason;
  - Home keeps showing "Ready" after the daemon stops (`Home.tsx:185`);
  - Save after an edit overwrites a newer transcript (`Home.tsx:351`);
  - Cancel from the palette doesn't cancel Home's pending Start
    (`useDictationActions.ts:54`);
  - `CommandPalette.tsx:96` doesn't trap focus.
- **F-M12** Identity mismatches:
  - the GTK app ID `app.narrivy.shell.gtk` vs `narrivy.desktop` with
    `StartupWMClass=narrivy-desktop`: COSMIC shows a generic or duplicate
    dock entry;
  - the CLI crate version is `0.0.0`, while the package is 0.1.0 (support
    reports show 0.0.0).

  Use one app ID and one version source.
- **F-M13** Running `narrivyd` without the unit's environment runs the mock
  engine. `DEFAULT_PIPELINE_ID = "mock-dictation"` (`policy.rs:35`), and the
  catalogs mark `asr-mock` `default-fallback`. A hand-started daemon returns
  fake transcripts. Require an explicit `NARRIVY_ASR_ENGINE=mock` for the mock.
- **F-M14** `build_deb_installable.py` accepts any prebuilt `narrivyd` without
  checking that it was built with `asr-transcribe-resident`. Add a build-time
  check that the daemon reports the installed-runtime capability.
- **F-M15** The legacy router is reachable with
  `NARRIVY_DELIVERY_BACKEND=platform` (F-L2); that path also reports every
  success as a failure. Fold into T2.11.
- **F-M17** Environment-controlled values reach ordinary stderr: for
  example `NARRIVY_AUDIO_SOURCE={other:?}` and the delivery backend errors in
  `daemon/policy.rs` echo the raw value (Sol S05). Replace them with
  content-free "invalid value" messages naming only the variable.
- **F-M18** No test proves that text survives a `SIGKILL` of the daemon
  mid-dictation (the lifecycle promise). Add a subprocess test: kill -9
  after `Committed` and before delivery, restart, and the text is replayed to
  the Inbox. It needs F-M3's durable Inbox (Sol S04).
- **F-M19** History has no user-facing "clear" action, and the retention
  control is cosmetic (F-H13). With D11, add a daemon-backed retention
  setting (off/7 d/30 d) and a "Clear History" command, or remove History
  from the 1.0 UI if D11 = off-by-default-with-no-UI (Sol S03/S04).
- **F-M20** Onboarding and Models are much smaller than the UX spec
  (`docs/product/ux/onboarding.md`, `screens/models-hardware.md`): no
  capability scan, shortcut setup/test, privacy choice, model catalog or
  removal. For 1.0, add the model-install step (F-M11) and a "set up your
  shortcut" step with the exact COSMIC instructions. Mark the rest of the
  spec as roadmap (WP4 T4.7).
- **F-M16** Test gaps that hid real defects:
  - `BundledModelInstall.test.tsx` and `CapabilityDisabledState.test.tsx`
    fake their states;
  - `tests/packaging` qualify fakes return `ok()` unconditionally, and the
    installable builder's `_inspect` and `build()` success path are untested;
  - `daemon_shutdown_signals.rs` uses the mock config, not the packaged
    installed path;
  - the History and support-report tests race `committed` against the
    History write;
  - process-linux tests block before their deadlines;
  - `capability_ui.rs:197` scans `dist/` non-recursively, so the
    no-OS-branching guard is vacuous.
  - `tests/hardening/test_p9_hardening_skeleton.py::test_kill_mid_migration_stub`
    is flaky (measured 2026-09-29: 1 failure in 10 isolated runs, and more
    often inside the full `pytest tests` run). It is timing-based and never
    touches `narrivyd`. Make it deterministic (a readiness handshake) or mark
    it non-gating, so CI and the WP exit checks stay reliable.

### Low (record, fix only if cheap)

- The portal/EIS findings (`portal_eis_zbus.rs:409,598`,
  `portal_eis_sender.rs:148`, `daemon/portal.rs:169`) matter only for side
  quest SQ1. The portal rung never runs in 1.0.
- The sidecar ASR path (`transcribe-cpp/src/lib.rs:48,253`, `prep.rs:82`,
  `sidecar.rs:380`) is not the release path.
- Resident streaming-only issues (`resident.rs:2020,2984,2343,2305`) affect
  only future streaming models. Whisper tiny reports `streaming=false`.
- `TriggerRouter` press/release routing (`trigger.rs:638,650`) is not used by
  the daemon.
- `process-linux` PGID reuse window and FD floor 256.
- `recovery.rs:1696`: oversized text skips the cancel check (WtypeThenInbox
  lane only).
- Diagnostics mislabels (`diagnostics.rs:135`, `session.rs:2464`).
- A double toggle before the first audio reports a failure.
- WAV file-source parsing (developer only).
- `NotUnicode` compute policy.
- `main.rs:206` unbounded join.
- Socket and lock paths in the journal.
- The support-report write isn't atomic (`support.rs:236`).

### Process and release-pipeline findings

- **P-1 (High, confirmed):** the phase-report cycle can never pass.
  `generate_phase_report.py:1012` runs `phase_gate.py`, whose last step
  (`phase_gate.py:453-470`) requires an existing passing report for HEAD. At
  a new commit the generator records a failure, so the gate fails forever.
  The only way out is a `--gate-command` override, which is forbidden. Fix in
  WP4 T4.6.
- **P-2 (High, confirmed):** duplicate evidence IDs block gates.
  `phase_gate.py:333-345` validates every file matching a `verification_id`.
  P10 has historical `p10-deb-install.json` and `p10-deb-build.json` next to
  `VER-OPS-002.json`. Move historical duplicates to `agent/history/` (WP4
  T4.6).
  *Status 2026-10-29:* P10 done. The six `p10-*.json` records moved to
  `agent/history/superseded-p10-evidence-2026-10-29/`. Other phases (P0F, P2,
  P9, P11) still have duplicate IDs.
- **P-3 (High, confirmed):** the SEC-004 runner pin is stale.
  `spec/evidence-verification-profiles.json:2531` pins `74214c49…` (the
  recorder at `1ccafff`). The recorder changed in `5bf08b9` and `bd6adec` and
  is now `068087f2…`, so SEC-004 evidence is always rejected. Re-pin it in a
  reviewed commit, and add a check that every pinned runner hash matches its
  file (WP4 T4.6).
- **P-4 (High, confirmed):** recorder defects beyond T4.1 (WP4 T4.5):
  - `record_native_hardening_evidence.py:376` wraps assertion dicts as IDs,
    so the record is always schema-invalid;
  - v3 recorders refuse to replace their own indexed v2 outputs;
  - `native_or_vm: native` is hard-coded or checked from spoofable
    environment variables in four recorders;
  - log privacy scans look for one fixed phrase, and a missing log counts as
    clean;
  - `record_cpu_performance_evidence.py:457` sets failures to 0 by
    construction;
  - "test ran" means the test name appears in the output, so skipped tests
    count;
  - PERF-002 compares the wrong metric, and negative latencies pass;
  - the upgrade procedure never starts the daemon;
  - REL-008 never runs the packaged support report;
  - the `SO_PEERCRED` rejection is never tested;
  - `cosmic_vertical_slice` uses the wrong scenario ID;
  - OPS-004 accepts `NOASSERTION` licences and unverified checksums.
- **P-5 (Medium-High):** the gates trust self-declared fields.
  - `release_gate.py:192` takes cleanliness from `git_dirty` in the file;
  - `release_gate.py:259` reads `cpu_fallback_pass`, `accessibility_pass` and
    `privacy_security_pass` as bare booleans with no evidence binding;
  - `phase_gate.py:471` falls back to Markdown when the JSON report is
    missing;
  - `phase_gate.py:409` computes `native_seen` from raw fields;
  - `phase_gate.py:205` misses the `[OPEN]` P11 blocker because it matches
    wording;
  - `validate_pack.py:292` never runs `verify_architecture_map.py`;
  - evidence accepts NaN/Infinity and unchecked timestamps.

  Fix in WP6 T6.0.
- **P-6 (High process hazard):** legacy `scripts/refresh_p0/p1/p3/p8_evidence.py`
  overwrite recorder output with hard-coded "native COSMIC" environments.
  AGENTS.md already forbids bulk refresh. Quarantine them (exit 2) in WP1.
- **P-7 (High for the coding agent):** stale files still steer an agent the
  wrong way:
  - `agent/CODING_AGENT_UPDATE_PROMPT_2026-08-24.md` says "continue at P8"
    and to use `run_gate.py`;
  - `agent/PHASES.json:24,44,134` summaries require GNOME;
  - `docs/maintainers/release/linux-1.0-qualification.md:15,32` requires a
    `gnome-wayland/` evidence directory;
  - `archive/docs/autonomous-build/reference-environments.md:3,7` tells the agent
    to provision GNOME itself;
  - `archive/docs/CHECKLIST_MASTER.md`, `docs/maintainers/phases/prompts/p11.md`,
    `docs/maintainers/qualification/reference-environments.md:26`,
    `docs/platforms/{bounded-support.md:37,gnome.md,linux.md,capability-matrix.md}`;
  - `agent/BLOCKERS.md` counts (98 vs 93; "none passed" vs 39 passed);
  - `archive/docs/management/autonomous-build-review-2026-08-22.md` (a
    `run_gate.py` recipe that breaks the INDEX);
  - `RELEASE-EVIDENCE-TEMPLATE.md` and `PHASE-REPORT-TEMPLATE.md` don't
    match what the gates enforce.

  Fix in WP1 T1.3.
- **P-8 (High, legal):** the SBOM (`build_candidate_supply_chain.py:531-587`)
  covers only `Cargo.lock`, with every licence `NOASSERTION`. It omits npm
  packages, transcribe.cpp/ggml and the model. The model's licence is
  `Apache-2.0` in the registry and MIT in the notices, and
  `THIRD_PARTY_NOTICES.md` says the model is "not bundled". Fix with D9 in
  WP6 T6.4.
- **P-9 (Medium-High, before the first public push):** committed files contain
  personal data:
  - `agent/evidence/P0F/p0f-mic-pipewire.json:67` has raw `wpctl status` with
    `<user>@<host>`, PIDs and running app names;
  - `packaging/deb/BUILD_LOG.md` and supply-chain `result.json` contain
    `~/...` paths.

  Add a scrub step and a scanner to the WP6 publish checklist. The owner
  chooses squash or full history at push time; full history would need a
  rewrite.
- **P-10 (Medium):** the docs promise things the code lacks:
  - `NAR-*` error-code registry;
  - observability counters;
  - Secret Service use (PRIV-005 is marked passed, but 1.0 stores no
    secrets: restate it);
  - 36 screen-contract test IDs that don't exist;
  - orphan `spec/ui/design-tokens.json`;
  - ADR-0002 and ADR-0028 describe the retired Tauri/GTK3 stack.

  Fix in the WP4 doc sweep (T4.7).

## Remaining work that is not a code defect

Covered in the work packages:

- 10 release requirements have no qualification procedure (WP4):
  FUN-001, FUN-013, PERF-001, PERF-009, PERF-010, REL-004, PORT-006,
  TEST-004, TEST-010, OPS-005. (PORT-001 moved to post-1.0 on 2026-09-28;
  see `07-post-release-side-quests.md`.)
- The COSMIC accessibility blockers ACC-001, ACC-005 and TEST-007
  ("Return routing") are still open (WP2 T2.10).
- There is no evidence campaign on one frozen commit (WP5), and no release
  evidence files (WP6).
- (GNOME machine provisioning moved to side quest SQ1; not needed for the first release.)
