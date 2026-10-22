# WP2 — Product fixes from the code review

**Goal:** fix every Critical and High finding in `00-review-findings.md`, plus
the Medium findings the owner has not waived. Each fix ships with a
regression test.

**Order matters.** Tasks are listed in the order to do them. T2.1 must come
before T2.4, or text could be typed twice.

**Per-task routine (from `docs/maintainers/agent/token-efficient-workflow.md`):**
1. Re-read the cited code; confirm the finding still holds.
2. Write the failing test first, and run it to see it fail.
3. Make the smallest correct fix.
4. Run the focused test, then the affected crate's full tests and clippy:
   `cargo test -p <crate> --locked` and
   `cargo clippy -p <crate> --all-targets --locked -- -D warnings`.
5. If you changed an ADR-governed boundary, write or update the ADR in
   `docs/adr/`.
6. Commit, then log the task in `PROGRESS.md`.

Crate names: `narrivy-core`, `narrivy-platform-linux`, `narrivyd`,
`narrivy-cli`, `narrivy-types`, `narrivy-desktop-gtk`. Front end: `npm test`
and `npm run build` in `apps/narrivy-desktop`.

---

## T2.1 — Stop the delivery ladder after the first dispatch (F-C2) — Critical

- **Files:** `adapters/platform-linux/src/delivery/routed.rs` (the loop at
  roughly lines 582–676, and its tests module).
- **Change:** after any rung outcome that sets `dispatched = true`
  (`SubmittedUnverified`, `CopiedOnly`, `Failed` other than `PolicyDenied`),
  `break` out of the loop. `Committed` already returns.
  `CancelledBeforeDispatch` already breaks.
- **Tests to add:**
  - `routed_ladder_stops_after_unverified_submission`: first rung
    `SubmittedUnverified`, second rung a recording rung. Assert the second
    rung never dispatched, the outcome is `InboxFallback`, and the attempts
    are `[portal-eis-text: SubmittedUnverified, inbox: InboxFallback]`.
  - `routed_ladder_stops_after_dispatch_failure`: the same, with the first
    rung returning `Failed(BackendError{..})`.
  - Keep `routed_ladder_cancellation_and_stale_retry_never_double_dispatch`
    passing.
- **Check:** `cargo test -p narrivy-platform-linux --locked delivery` then the
  full crate tests. Also run `cargo test -p narrivyd --locked strict_routed`,
  because session tests compose this ladder.
- **Docs:** add one sentence to ADR-0072 (or a short follow-up ADR): "At most
  one rung may dispatch per transaction."

## T2.2 — Enlarge the capture queue (F-H1) — High

- **Files:** `adapters/platform-linux/src/recovery.rs:57–58` and tests.
- **Change:** raise `STREAM_CAPTURE_QUEUE_CHUNKS` so the queue holds at least
  10 s (128 chunks at 1,280 samples = 10.24 s at 16 kHz). Update the constant's
  doc comment with the memory bound (about 655 KB).
- **Test:** a producer/consumer test where the consumer waits 2 s before
  draining. All chunks must arrive in order, with no `Backpressure` terminal.
  Use the existing `sync_channel` test seams (look near lines 2977 and 3166).
- **Check:** `cargo test -p narrivy-platform-linux --locked recovery`.

## T2.3 — (moved) Portal keyboard delivery for GNOME

Moved to side quest SQ1 (owner decision 2026-09-28: first release is Pop!_OS
COSMIC only). Skip it now. T2.1 still matters, because the ladder must never
dispatch twice.

## T2.4 — Ship the routed lane and depend on `wtype` (F-C1, F-C4) — Critical

Depends on T2.1.

- **Files:**
  - `packaging/deb/narrivy-daemon.service`,
    `packaging/linux/deb/narrivy-daemon.service`,
    `packaging/recovery/narrivy-daemon-candidate.service` (all three
    byte-identical);
  - `scripts/build_deb_installable.py` (control fields around lines 90–100);
  - `packaging/deb/control` and `packaging/linux/deb/control`;
  - front-end remediation text for a missing `wtype`.
- **Change:**
  1. Set `NARRIVY_RECOVERY_MODE=linux-mic-routed-inbox` and
     `NARRIVY_DELIVERY_BACKEND=routed-inbox` in all three unit files.
  2. Add `wtype` to `Recommends:` (or `Depends:`; ask if unsure).
  3. Confirm the daemon starts with this environment (ADR-0072 §5 says the
     routed lane needs PipeWire, transcribe.cpp and a bound model). Run the
     release daemon locally with the same environment and check that
     `narrivy-cli status` reports the routed lane.
  4. When the capability report says `wtype_missing`, the Home/Diagnostics
     text should tell the user to install it
     (`sudo apt install wtype`). Find where delivery capability states render
     (`apps/narrivy-desktop/src/lib/capabilityStates.ts`).
- **Tests:** a packaging test that the unit selects the routed lane and that
  the control file recommends `wtype`. A frontend test for the remediation.
  `tests/packaging/test_build_deb_candidate.py` must still pass the
  byte-identical check.
- **Check:** `python3 -m pytest -q tests/packaging`, `npm test`,
  `systemd-analyze verify packaging/deb/narrivy-daemon.service` (tolerate only
  the warnings it already had).

## T2.5 — Hotkey works when the daemon is not running; unit tied to the session (F-H4, F-H5) — High (D5 answered 2026-09-29)

- **Files:** `apps/narrivy-cli/src/main.rs`, the three unit files, and the
  `postinst`/`prerm`/`postrm` scripts under `packaging/deb` and
  `packaging/linux/deb` (and `packaging/linux/scripts` if the builder uses
  them; check `--postinst` inputs).
- **Change (CLI):** for `toggle`, `start` and `stop`, if connecting fails
  with "not found" or "connection refused":
  1. run `/usr/bin/systemctl --user start narrivy-daemon.service` with a
     bounded timeout. Reuse the pattern in
     `apps/narrivy-desktop/src-gtk/src/shell.rs:280` (`run_systemctl`). Use a
     fixed absolute path and never search PATH;
  2. retry the connection for up to about 3 s;
  3. on final failure, send one desktop notification over D-Bus
     (`org.freedesktop.Notifications.Notify`; `zbus` is already a dependency)
     with a content-free message, and exit non-zero.
- **Change (unit):** `WantedBy=graphical-session.target`,
  `PartOf=graphical-session.target`,
  `After=graphical-session.target pipewire.service`.
- **Change (D5, owner 2026-09-29):** no global enable. The package never
  enables the unit for users. Add a **"Start Narrivy at login"** setting in
  the desktop app, off by default:
  - it runs `/usr/bin/systemctl --user enable|disable narrivy-daemon.service`
    for the current user, with a fixed path and a bounded timeout, like
    `run_systemctl` in `shell.rs`;
  - it displays the real state from `systemctl --user is-enabled`, never a
    stored guess;
  - it has a frontend test and a shell-side test with an injected runner.

  The CLI on-demand start above keeps the shortcut working when the setting
  is off.
- **Tests:** CLI unit tests with an injected connector and starter: start is
  called exactly once, the retry succeeds, and the timeout path notifies
  once. Packaging tests for the unit and scripts.
- **Check:** `cargo test -p narrivy-cli --locked`,
  `python3 -m pytest -q tests/packaging`.

## T2.6 — Sessions never stop on silence (F-H3) — High

Documented behaviour, not a decision: toggle stops on the second press, hold
stops on key release, and neither stops on silence.

- **Files:** `services/narrivyd/src/daemon/session.rs` (around 3364–3376).
- **Change:** pass `endpointing: None` for dictation sessions. Remove the
  `NativeEnergyVad`/`DictationEndpointer` construction there. Leave
  `crates/narrivy-core/src/endpoint.rs` in place, unused by default. The 300 s
  safety limit still ends runaway sessions.
- **Tests:** a streaming session with a mid-utterance silence longer than
  1.5 s is still listening, and finalizes only on Stop. Update any test that
  expected endpoint auto-finalization in the daemon path.
- **Check:** `cargo test -p narrivyd --locked` and
  `cargo test -p narrivy-core --locked`.

## T2.7 — Save what was said when the microphone disappears (F-H2) — High (D6 answered)

- **Files:** `crates/narrivy-core/src/recovery.rs` (streaming around
  1006–1015; batch around 490), and the tests that currently pin discard
  behaviour, such as
  `batch_source_ended_journals_reason_and_fails_before_any_tail_side_effect`.
- **Change:** on `CaptureEnd::SourceEnded` with a non-trivial amount of audio
  (for example ≥ `minimum_speech_ms`), finalize the ASR stream and run the
  normal tail, but force recovery-only delivery (into the unified History from
  T2.8, D6), with no typing. Journal `capture_end =
  SourceEnded` so the UI can explain it. With too little audio, keep today's
  failure.
- **Time box:** if this takes more than one working day, stop and ask the
  owner. D6 makes this a wanted behaviour, so don't drop it silently.
- **Tests:** the updated core tests. Assert that no direct rung runs on the
  salvage path.
- **Check:** `cargo test -p narrivy-core --locked`, `cargo test -p narrivyd --locked`.

## T2.8 — One durable History for every dictation (F-M3, F-M4, F-H17, F-M19; owner D6 + D11) — High

Owner decision D6/D11 (2026-09-29): every dictation, delivered or not, is
kept in **one** History. Undelivered or unverified entries are flagged, with
Copy and Retry. The size is set by entry count: **default 10, range 0–1000**,
oldest removed first. **0 keeps nothing.** This replaces the separate tmpfs
Inbox and the 7-day time retention.

- **Files:**
  - `services/narrivyd/src/main.rs` (Inbox directory at 482, History
    composition at 417);
  - `services/narrivyd/src/daemon/retained_history.rs` and
    `crates/narrivy-history/src/history.rs`;
  - `adapters/platform-linux/src/delivery/inbox.rs` and `routed.rs` (Inbox
    fallback write);
  - `crates/narrivy-core/src/recovery.rs` (startup replay: it marks
    `InboxRecovered`);
  - the IPC protocol;
  - the front end (the History screen, Settings → Privacy, onboarding).
- **Change:**
  1. **Store:** History lives under the private state directory
     (`$XDG_STATE_HOME/narrivy/`, 0700 directory, 0600 files, explicit
     modes, not umask).
     - Each entry records the text variants (raw/clean), the time, the
       pipeline and the **delivery status** (delivered / submitted-unverified
       / not delivered, plus the reason code).
     - Use `PRAGMA secure_delete=ON`, plus a WAL checkpoint after deletions
       (F-H17).
  2. **One write path:** the routed ladder's fallback, journal startup
     replay, the mic-loss salvage (T2.7) and normal delivered dictations all
     record into History. When the limit is 0, write nothing. The journal
     alone then protects in-flight text for up to 15 minutes.
     - Replay must not mark a journal entry terminal (`InboxRecovered`)
       unless History actually stored it. With a limit of 0, keep today's
       journal TTL behaviour.
     - Migrate any existing tmpfs Inbox files once, at startup.
  3. **Limit:** a count cap, enforced in **one transaction** (delete all but
     the newest N; never clear and re-insert, per F-H17). Any read or parse
     error when loading the setting falls back to **10**, never to
     unlimited (fail closed; `history.rs:274` fails open to Forever today).
     Remove the time-based `Days7`/`Forever` default.
  4. **IPC:** list (paged), read one, delete one, clear all, retry delivery
     of one (runs the normal routed ladder again, with the same
     double-dispatch guard as T2.1), get/set the limit. All are same-user,
     bounded, and never logged.
  5. **UI:**
     - the History screen: undelivered entries are flagged and offer Copy and
       Retry; every entry can be deleted; "Clear all";
     - Settings → Privacy → "Keep last N dictations", with 0, 10, 50, 100,
       500 and 1000;
     - when 0 is picked, say: "Nothing is saved. If typing fails, text is
       kept only until the next restart.";
     - onboarding tells the user History exists and shows the default.

     Remove the cosmetic retention control (F-H13).
  6. **Remove** the now-dead parts: time-based retention, the privacy-zone
     History hook (zones are post-1.0 per D13), and the old tmpfs Inbox
     directory.
- **Tests:**
  - daemon: every write path lands in History; limit 0 stores nothing; the
    limit is enforced atomically (with injected failure); secure-delete is
    on; the fallback is 10 on a corrupt setting;
  - IPC round-trips;
  - frontend: flags, Copy, Retry, Delete, and the limit setting;
  - SIGKILL replay into History (F-M18);
  - `scripts/check_log_redaction.py` passes, and the support report
    contains no text.
- **Traceability:** PRIV-002 ("retention user-configurable, can be
  disabled") is met by the count limit including 0. Its Privacy-Zone clause
  moves post-1.0 under D13 (a recorded amendment).

## T2.9 — Better default model (F-M7) — Medium (D4 = A: measure base.en first)

- Measure the candidate models with the existing CPU performance procedure,
  without editing the recorder script:
  `scripts/record_cpu_performance_evidence.py` in a scratch run, or
  `scripts/qualify_installed_model.py`.
- If D4 = A and `base.en` passes, add its registry pin (id, SHA-256, size,
  licence, source) the same way `whisper-tiny.en-Q8_0` is pinned (search the
  model registry for that id). Then change the unit's
  `NARRIVY_TRANSCRIBE_MODEL_ID` and the builder's `--model` input.
- Record measurements in `agent/RESEARCH_LOG.md`.

## T2.10 — COSMIC keyboard accessibility blocker (ACC-001, ACC-005, TEST-007)

- **Evidence so far:** the recorder
  (`scripts/record_native_accessibility_evidence.py:375–445`) sends keys with
  `wtype`. `wtype` types into the window that has **compositor** keyboard
  focus. The recorder only sets **AT-SPI** focus. Its failure message says
  COSMIC did not route Return to the AT-SPI-focused window.
- **Hypothesis to test first:** the Narrivy window never received compositor
  focus. It was launched from a script without an xdg-activation token, and
  COSMIC's focus-stealing prevention kept focus on the terminal. That would be
  a harness defect, not an app defect.
- **Steps:**
  1. Reproduce by hand: launch the packaged app from the app launcher (which
     passes a token), then use Tab, Return and Escape through the onboarding,
     main navigation and Diagnostics screens. If everything works by hand, the
     app is fine.
  2. Check whether the GTK shell calls `present()` on its window at startup
     (`apps/narrivy-desktop/src-gtk/src/shell.rs`). If not, add it; that is
     correct app behaviour anyway.
  3. Fix the harness so it launches through the desktop entry (for example
     `gtk-launch narrivy`, which requests activation) or otherwise gives the
     window real compositor focus. Then assert compositor focus (the first
     `Tab` moves AT-SPI focus) before sending Return.
  4. Harness changes alter the recorder script's hash. Update the
     `runner.sha256` of **every** procedure that uses this script in
     `spec/evidence-verification-profiles.json`, then run
     `python3 -m pytest -q tests/governance`. Do this **before** the freeze in
     WP5.
- **If the manual test fails:** it is a product bug. Trace the WebKit key
  handling in the shell and fix it with a regression test.

## T2.11 — Small correctness fixes and dead code (F-M1, F-M2, F-L1, F-L2)

- **F-M1:** in `NativeEnergyVad::classify`, reject `chunk.channels != 1` with
  `vad_audio_format_invalid` (simplest). Add a stereo test.
- **F-M2:** in `classify`, compute the number of windows the chunk would
  complete before mutating state. Reject it with `vad_chunk_too_large` if it
  exceeds `MAX_ACTIVITY_DECISIONS`. Extend the existing rejected-chunk test.
- **F-L1:** delete `engines/transcribe-cpp/transcribe-sys/` and update the
  docs that mention it. Confirm with `rg -n transcribe-sys` that nothing
  builds it.
- **F-L2:** confirm with `rg -n "route_and_deliver|DeliveryRouter"` that no
  production composition uses the legacy router. Then gate it behind
  `#[cfg(test)]` or delete it together with its tests.
- **Check:** crate tests and clippy for `narrivy-core` and
  `narrivy-platform-linux`; a full `cargo test --workspace --locked` once at
  the end of T2.11.

## T2.12 — Second-pass fixes (replaces the old "bounded review")

The whole-repository review is done (see "Second-pass findings" in
`00-review-findings.md`). Do these in order. Each one needs the test named in
its finding. Re-read the cited code first, because line numbers move.

1. **F-C6 (Critical):** move the `job_model_path` requirement in
   `services/narrivyd/src/daemon/session.rs` into the non-resident branch.
   Add the installed-mode session test. Do this before WP3's native smoke:
   without it, no installed dictation can work.
2. **F-C5 (Critical):** add the trusted packaged-source import in
   `adapters/platform-linux/src/model_import.rs` for the one fixed bundled
   path. Make `scripts/qualify_candidate.py` provision as uid 1000, not root.
3. **F-H10:** fix the six test call sites in `daemon/policy.rs`. Add
   `cargo test -p narrivyd --locked --features asr-transcribe-resident` and
   the full `python3 -m pytest -q tests` to `.github/workflows/ci.yml` and to
   every WP exit check from now on.
4. **F-H7:** fix the compute-probe pipe (drain while waiting, byte cap).
   Build the compute plan once, and move compute discovery after `serve`.
   Make startup-to-first-handshake reliable (finish mandatory startup before
   `bind`, or send a typed "starting" state with client retry). Test with a
   1 MiB fake program.
5. **F-H8:** add a constant `LC_ALL=C.UTF-8` to the `WaylandTextInput` helper
   environment in `adapters/process-linux/src/lib.rs`. Test with a fake
   `wtype` that reports its locale.
6. **F-H9 and D12:**
   - fix `services/narrivyd/src/correct.rs` punctuation spacing;
   - replace the word-by-word ITN table with an **optional, context-aware
     "Convert spoken numbers to digits"** post-processing step. It needs a
     global setting (off by default) and a per-model override;
   - add a `writes_digits` (or similar) flag to each model's registry/catalog
     entry. Whisper models set it, so the step stays off for them unless
     overridden per model;
   - wire it end to end: daemon setting, IPC, UI;
   - table tests: "twenty five" → 25, "three point one four" → 3.14,
     "no one" / "one of them" / "the one" unchanged, and the model-flag
     interaction.
7. **F-H16:** stop tests from composing the real wtype rung (the
   `session.rs:8298` test and any other `System*Rung` use in tests). Add a
   governance check. Until then, run `cargo test` with `WAYLAND_DISPLAY`
   unset.
8. **F-H12, F-H13, F-H14, F-M8, F-M9, F-M10, F-M11:** front-end truth fixes:
   - event sequence reset on reconnect;
   - remove or disable the unwired Settings controls and pipeline
     import/export;
   - `listening` state name;
   - keep the failed overlay state visible;
   - never stop the event forwarder on a full queue;
   - single-flight status polling;
   - onboarding offers the bundled-model install;
   - show the provisioning failure reason;
   - stale "Ready";
   - Save-overwrites-newer;
   - app-level cancel latch;
   - palette focus trap.

   Each fix needs a Vitest case that drives the real protocol state names.
9. **F-H17:** done inside T2.8 (the unified History, D6/D11): single
   transaction compaction, secure delete, a fail-closed limit, and no
   privacy-zone claims in the 1.0 UI or docs.
10. **F-M12, F-M13, F-M14:**
    - one app ID across `GtkApplication`, the `.desktop` file name and
      `StartupWMClass`;
    - one version source for Cargo and Debian;
    - the mock engine only with an explicit `NARRIVY_ASR_ENGINE=mock`;
    - the installable builder checks the daemon's installed-runtime
      capability.
11. **F-M16:** replace the fake tests listed in the finding with ones that
    exercise the real path. At minimum:
    - the bundled-model provisioning test uses a root-owned source;
    - the installable builder `_inspect`/`build()` success path;
    - a packaged-shape shutdown-signal test;
    - History tests wait for `active_session == None`.

12. **F-C7 (D14 = C, owner 2026-09-29):**
    - leave other audio untouched;
    - add a **microphone picker**: a daemon command lists PipeWire sources
      (real devices first, monitors excluded) and stores the choice, and
      `pw-record` receives `--target <node>`. If the chosen source is gone,
      fall back to the default and tell the user;
    - a clear error when no microphone exists; after a replug, the next
      dictation works (native test);
    - measure PERF-006 on the real `recovery.rs` capture thread;
    - traceability: FUN-007 = leave; FUN-011 is restated to default plus
      picker plus no-mic error plus replug; FUN-006, FUN-012, REL-005 and
      REL-009 move post-1.0 with a recorded amendment in `agent/DECISIONS.md`
      (done by the owner decision; the agent writes the rows).
    - The older text of this item follows, for reference.
    - Under A: amend the traceability rows and docs, and make the
      audio-policy recorder measure the real `recovery.rs` capture path
      (PERF-006) and the replug behaviour.
    - Under B or C: wire the kept features into the daemon capture path, with
      tests. Fix the B38 bugs first if ducking is kept.

    Never let a probe example stand in for daemon behaviour.
13. **F-H18:**
    - `LimitCORE=0` in all three unit copies;
    - `prctl(PR_SET_DUMPABLE, 0)` at daemon and worker startup;
    - document the swap residual risk and amend the ADR wording;
    - add the tests in the finding.
14. **F-M17–F-M20:**
    - content-free environment error messages;
    - a SIGKILL replay test;
    - a daemon-backed History retention setting and Clear action per D11;
    - onboarding gets model-install and shortcut-setup steps.
15. **Second-pass Medium items** (fold in while touching the same code):
    - `ipc-unix` `publish_event` blocks on the per-client writer mutex before
      the 50 ms bound (`server.rs:497`): use `try_lock` or a bounded
      per-client queue;
    - subscribers are dropped after 30 s idle (`server.rs:959`): exempt
      subscribed connections or add a keepalive;
    - CLI: never auto-retry a sent command after a timeout; report "outcome
      unknown" (`client.rs:242`);
    - the batch ASR path must require `is_final` (`core/recovery.rs:516`);
    - worker panic payloads become a content-free code
      (`core/worker.rs:387`);
    - History retention fails closed to Off on any read error
      (`narrivy-history/src/history.rs:274`);
    - journal `.corrupt` files follow the 15-minute TTL, and the TTL also runs
      at startup and on a timer.

**Check:** `cargo test --workspace --locked`,
`cargo test -p narrivyd --locked --features asr-transcribe-resident`,
`npm test` in `apps/narrivy-desktop`, `python3 -m pytest -q tests`.

## T2.14 — Spoken aliases / custom vocabulary (FUN-041; owner D13 = C) — Medium

- **Files:**
  - `crates/narrivy-history/src/dictionary.rs` (today only its tests use it);
  - `services/narrivyd/src/daemon/policy.rs` (pipeline correction
    strategies at about 145) and `services/narrivyd/src/correct.rs`;
  - the IPC protocol;
  - the front end (a small Dictionary screen).
- **Change:**
  1. Apply the user's aliases in Clean post-processing, after correction and
     before the optional number conversion (D12). Match whole words, case
     insensitively, with **Unicode-aware** folding and boundaries. Today it
     is ASCII-only (B32). Apply in one pass so a replacement is never
     re-expanded.
  2. Store the dictionary under the private state directory with explicit
     0600 modes and atomic writes (temp file, fsync, rename).
  3. IPC commands: list, add, edit, delete. Same-user and bounded; alias
     values never appear in logs.
  4. UI: a Dictionary screen (spoken form → written form), with add, edit
     and delete, and keyboard accessible.
- **Tests:**
  - unit: whole-word, case, Unicode, no chaining, and an empty dictionary is
    a no-op;
  - daemon: a dictation with an alias produces the canonical form;
  - frontend: the CRUD flows;
  - log redaction still passes.
- **Traceability:** FUN-041 gets a procedure bound to the daemon-level test.
  PRIV-003 (privacy zones) moves post-1.0 under D13.

## T2.15 — GPU pool (Vulkan, OpenVINO GPU, ONNX Runtime CUDA/ROCm/TensorRT) with CPU fallback (locked pipeline, D21) — High

- **Files:** `engines/transcribe-cpp/` (worker build and backend
  selection), `scripts/stage_installed_runtime.py` and
  `scripts/build_deb_installable.py` (package the backend libraries),
  `crates/narrivy-compute` (planner and qualification), and
  `services/narrivyd/src/daemon/compute.rs` (plan wiring).
- **Change:**
  1. Build the pinned transcribe.cpp with **Vulkan + dynamic backends**
     (the Handy 0.9.7 approach), so one worker runs on CPU-only machines and
     loads Vulkan when a usable device exists. The rest of the documented
     **GPU pool** is **OpenVINO GPU** (through the T2.16 OpenVINO engine) and
     **CUDA/ROCm/TensorRT through ONNX Runtime where present** (ONNX Runtime
     execution providers, loaded only when the vendor runtime exists).
     Everything is self-tested; tuples untestable on the reference machine
     carry no release claim. Bump the pin from v0.2.3 to
     the current release only with recorded hashes (BUILD-NOTES). Package
     deps: `libvulkan1` (Depends); `mesa-vulkan-drivers` (Recommends).
  2. The worker reports its devices. The planner picks
     **NPU → dGPU → iGPU → CPU** among devices that passed a **local
     self-test**:
     - the known-phrase decode of `tests/fixtures/jfk16.wav` must match;
     - the RTF must be within target;
     - run it on first use or after a model or driver change;
     - store the record under the private state directory.

     Fix the known qualification gaps first:
     - the vacuous `p5_qualify_host` tests;
     - the tuple identity not enforced (B28);
     - case-sensitive fragment matching;
     - the qualification-path mismatch between the runner and the daemon
       (B26).
  3. **Runtime fallback:** a decode error, timeout or device loss on the
     chosen tier finishes the dictation on the next tier (CPU last). The
     tier is marked unhealthy until the next self-test. The dictation is
     never lost (TEST-006 device loss).
  4. Move compute discovery off the startup path (F-H7).
  5. **UI:** Settings → Compute shows the chosen device and why, with an
     override (Auto / a specific device / CPU). It is wired to the daemon,
     not browser storage (F-H13).
- **Tests:** planner unit tests (every tier fails, then CPU); a
  fallback-on-decode-error test with a controlled worker; native: the self
  test selects the Arc iGPU, and forcing a Vulkan failure falls back to CPU.
- **Evidence:** `qualification.json` `accelerator_claims` for the exact
  tuple (hardware + driver + runtime + model + quant + OS + Narrivy version),
  a PERF comparison of CPU vs Vulkan, and the CPU-fallback run (TEST-010).
- **Estimate:** 3–5 days for Vulkan and CPU. The ONNX Runtime providers
  add about 1–2 weeks (a new engine crate, `engines/asr-onnx`, and
  packaging).
- **Status 2026-09-30 (ADR-0076, D25):** done: the `vulkan` worker feature;
  `--compute auto|cpu`; the worker's load-then-self-test on the GPU with CPU
  fallback; a CPU replacement when a GPU worker fails to start or fails a
  one-shot decode (same audio, not lost); the attested
  `asr.transcribe-cpp.device` token and its display; `libvulkan1` in the
  package; a CI build. **Open:** stored self-test records per tuple and
  re-test only on change; the planner using them (and the gaps in item 2);
  replaying a failed streaming session on the CPU; item 4 (discovery off the
  start-up path); item 5 (the Settings control); building the packaged
  worker with Vulkan; the evidence. D25 makes CUDA a Store add-on instead of
  an ONNX Runtime provider probed for at start-up.

## T2.16 — Intel NPU (and OpenVINO GPU) speech recognition via OpenVINO 2026.4 (locked pipeline, D21) — High

- **Prior art to adapt** (owner-authored, clean-room allowed):
  - `code/handy-npu/` (gitignored locally): an isolated OpenVINO NPU worker
    (C++) with the Eddy Parakeet decoder and a Rust supervisor, proven at
    about 220–240 ms warm on this NPU;
  - `Handy-Pop-OS/docs/openvino-npu-linux-design.md`;
  - `docs/roadmap/speech/openvino-npu.md`;
  - the runtime pins in `agent/RESOLVED_VERSIONS.json` (OpenVINO 2026.4).
- **Change:**
  1. A new engine crate, `engines/asr-openvino`, with an **isolated worker
     process** under `adapters/process-linux` containment (like the resident
     transcribe.cpp worker), speaking the same resident protocol shape.
     OpenVINO types stay inside the engine (ADR boundary).
  2. Package the minimal pinned OpenVINO runtime for the NPU plugin, and
     check the userspace NPU driver (Level Zero / `intel-npu-driver`) on
     Pop!_OS 24.04. If it isn't in the Pop!_OS or Ubuntu archives, **stop
     and ask the owner** how to ship it. Health-check the driver and the
     `/dev/accel*` permissions, with a clear UI message when missing.
  3. NPU compiled-model cache: cold compile → cache → warm load (locked
     2026-08-21), stored in the private cache and invalidated on driver or
     runtime change (PERF-008).
  4. The planner treats NPU as tier 1, with the same self-test and runtime
     fallback as T2.15.
  5. Models: the locked tiers (Whisper large-v3 OpenVINO IR FP16/INT8 for
     accuracy; the small family for balanced). They are delivered through
     T2.17, with licence and provenance records.
- **Tests:** a controlled worker for fallback and cancellation; native
  qualification of the NPU tuple on the reference machine; forcing an NPU
  failure falls back to Vulkan, then CPU.
- **Estimate:** 2–3 weeks. It is the largest single item added to 1.0.

**Status 2026-10-01 (ADR-0078):** first slice done.
- `engines/openvino` worker on NPU and CPU, loaded from a Store-installed
  runtime pack.
- Model packs installed and verified file by file.
- `compute: npu`, a self-test before any accelerator is used, and the
  compile cache.
- Measured: Whisper large-v3-turbo INT8 runs at 0.45 s per 11 s clip on the
  NPU against 3.0 s on the CPU. The first NPU compile takes 106 s; cached
  loads take 5.3 s.

Remaining:
- the OpenVINO GPU, which needs Intel's GPU compute runtime (an owner
  decision on how to ship it);
- warming the cache at install time;
- skipping the re-hash of an unchanged pack;
- packaging;
- Parakeet and Qwen3 on the NPU;
- stored qualification.

**D26 addition (2026-09-30):** the OpenVINO worker is one process for all
OpenVINO stages. It hosts noise suppression, speech recognition and the
shared language model, each on its own chosen and self-tested device.
See `docs/roadmap/design/writing-assistant.md`.

## T2.17 — Model and runtime-pack delivery from the narrivy Hugging Face org (locked pipeline, D21) — High

The locked strategy (`model_library_strategy`, `huggingface_strategy_locked`,
ADR-0017) delivers models, and optional runtime packs such as OpenVINO, from
<https://huggingface.co/narrivy>. The current code only imports a bundled or
local file (`NoNetworkTransport`), and the privacy docs say "no network
client". Implementing the locked design means:
1. Publish the converted, pinned artifacts (IR/ONNX/GGML per tier) to the
   narrivy org with model manifests. This is an owner step: the account and
   the upload.
2. A **user-initiated, verified download** transport in
   `crates/narrivy-model-manager`:
   - HTTPS to huggingface.co only, at a pinned revision;
   - size and SHA-256 checked before activation;
   - resumable and cancellable;
   - no telemetry.

   The daemon stays network-free apart from this explicit action.
3. Update the privacy docs, onboarding and the Privacy screen: the only
   network use is downloading models and runtime packs when the user asks.
4. The bundled CPU-fallback model stays in the `.deb` so dictation works
   offline out of the box.
5. **Choosing the model** (owner direction 2026-09-30): the Models page
   downloads; the pipeline's Transcribe stage chooses. **Done for a daemon
   started with an explicit model:** `SelectModel` (IPC minor 14), the
   chooser in the Transcribe panel, and the remembered choice. **Open:** the
   same for the packaged composition (the registry-bound installed model)
   and the download itself (items 2 and 3). **Done 2026-09-30:** a model
   and a device per pipeline (`SetPipelineEngine`, IPC minor 15), reloaded
   at start only when they differ from what is loaded; and the download
   (ADR-0077, IPC minor 16): the system `curl` as a revalidated helper,
   Hugging Face hosts only, resumable, stoppable, verified against the pinned
   size and SHA-256 before use, for a daemon started with an explicit model.
   **Open:** downloads in the packaged composition, multi-file packs,
   deleting a model, proxy support.

## T2.18 — Overlay the Handy way (F-H11; owner direction 2026-09-29) — High — **re-scoped by D22: implement in the Tauri host (T1.4 step 5) with GTK3 `gtk-layer-shell` from the Pop!_OS archives; the GTK4 bundling in item 5 below no longer applies**

**Owner direction:** the overlay follows the Handy approach. That approach
is already binding in `docs/product/ux/overlay.md` ("COSMIC/Wayland platform
truths, owner-fork-verified 2026-08") and in the 2026-08-21 streaming-overlay
decision (`agent/DECISIONS.md`). Reference implementation (owner-authored,
adapt, don't copy): the Handy-Pop-OS fork, plus upstream Handy 0.9.6+
(`gtk-layer-shell` on Linux, `tauri-nspanel` on macOS). The investigation
is in `code/handy-npu/docs/2026-08-01-overlay-changes-investigation.md`
(gitignored locally).

- **Files:** `apps/narrivy-desktop/src-gtk/src/overlay.rs` (today a plain
  `gtk::Window`, which is the F-H11 risk), `src-gtk/Cargo.toml`, and the
  package builder.
- **Change:**
  1. Make the overlay a **layer-shell surface** (overlay layer, anchored top
     or bottom per setting) with **keyboard interactivity = none**, so it can
     never take focus from the app being dictated into.
  2. **Monitor selection:** at each trigger, map an invisible 1×1
     layer-shell probe with no requested output. The compositor puts it on
     the pointer's monitor; read that monitor, place the card there, then
     destroy the probe. Never trust global pointer coordinates on COSMIC.
  3. Follow the rest of `overlay.md`:
     - clear transcript state on every session start;
     - width 35% of the monitor, clamped to 400–760 px (400 before the first
       text, with a 200 ms ease-out when the first text arrives);
     - height grows with the text and scrolls beyond half the monitor
       height;
     - design tokens rather than hard-coded colours;
     - a reduced-motion alternative;
     - no "Transcribing" flash for very short stages.
  4. **Streaming vs non-streaming** (decision of 2026-08-21):
     - streaming models show a voice equalizer plus live partial text,
       optional or configurable per `overlay.md`;
     - non-streaming models show the equalizer only, then the final result.
  5. **Library:** Pop!_OS 24.04 ships only the GTK3 `libgtk-layer-shell0`
     (0.8.2). The GTK4 `gtk4-layer-shell` is not in the 24.04 archives. Build
     it from a pinned upstream release (MIT licence) and bundle it in the
     `.deb`, as the transcribe.cpp worker is handled: record the hashes and
     add it to the licence inventory (T6.4). Use the `gtk4-layer-shell` Rust
     crate. Do **not** fall back to GTK3, which would bring back the
     advisories that ADR-0064 removed.
  6. **When layer-shell isn't available** (another compositor, or a missing
     library): never show a focusable window. Hide the overlay before
     delivery, and report the capability as `overlay: unavailable`.
  7. Fix F-M8 in the same change: keep a failure visible until a timeout
     instead of hiding it on the next `Idle`.
- **Tests:**
  - unit: reducer states (streaming vs not; clear on each session);
  - native (WP3 smoke): on the owner's 3-monitor mixed-DPI rig, the card
    appears on the pointer's monitor, and **focus never leaves COSMIC Text
    while text is typed** (closes F-H11).
- **Estimate:** 2–4 days, including bundling the library.

## T2.19 — Module Store, 1.0 slice (owner D23) — High

**Status 2026-10-01 (ADR-0078):** first slice done.
- IPC minor 17: `ListStore`, `RemoveStoreItem`; `DownloadModel` installs
  runtime and model packs.
- The daemon decides each item's fit verdict, and the Store screen shows
  it.
- A developer source (`NARRIVY_STORE_LOCAL_DIR`) is used until the runtime
  pack is published.

Remaining:
- the signed catalog snapshot and updates;
- the install-time self-test;
- "Add…" on pipeline stages;
- noise-suppression and refinement items.

Design: `docs/roadmap/design/module-store.md`. Builds on T2.17 (verified
downloads from the narrivy Hugging Face org), T2.15 and T2.16.

1. Daemon: a store catalog (a versioned, signed snapshot) and a "fits this
   computer" verdict per item, computed from the discovery report: fits,
   fits after adding a runtime, does not fit (with the reason), unknown
   until tested. The frontend renders the verdict; it never decides it.
2. Install, update, rollback and removal as the transaction in ADR-0017,
   with the states in the design. Narrivy Verified items only.
3. UI: a Store sidebar entry, and "Add…" on each pipeline stage opening the
   Store filtered to that stage.
4. Tests: every edge case in the design's table has a test or a recorded
   reason it is covered elsewhere.

## T2.20 — Refine: rules and a local language model (owner D23, ADR-0018) — Medium

1. Rules: per-rule settings for the deterministic corrector (numbers as
   digits with T2.12, vocabulary and replacements with T2.14), owned by the
   daemon and reported through status.
2. Language model (local only first): choose an installed refinement model
   (through T2.19), where it runs (scheduler-qualified, ADR-0018), and the
   processing level: faithful cleanup, polish, rewrite.
3. The transformation guard ("meaning check") before delivery, with the
   protected tokens listed in ADR-0018. The raw transcript stays
   recoverable until delivery commits.
4. UI: the Refine sidebar screen (the read-only first version exists as of
   2026-09-30).

5. **D26:** exactly one language model is loaded. Refine and the writing
   assistant (SQ-WRITE, after the release) share it through one daemon-owned queue. Dictation
   has priority, and extension requests can be cancelled and are dropped
   when stale.

## T2.21 — Built-in light noise suppression (owner D23) — Medium

The catalog's built-in light DSP cleanup (`narrivy-lite-dsp`, linux-1.0) as
an optional stage before ASR, off by default until qualified. It must work
on any computer, add no dependency download, and report its state through
status so Home and Pipelines show it truthfully. Hardware-specific
noise-suppression modules come later through the Store (T2.19).

## T2.13 — Press-and-hold activation on Pop!_OS COSMIC (FUN-001, FUN-031)

**Spec:** two modes, toggle and press-and-hold. FUN-001 lets hold be advertised
only where key-release is proven (`docs/platforms/linux-global-shortcut-matrix.md`,
"PTT acceptance").

**Facts (checked 2026-09-28 on the reference machine):**
- COSMIC (`xdg-desktop-portal-cosmic 0.1.0pop1`) still has **no**
  `org.freedesktop.portal.GlobalShortcuts`, only `RemoteDesktop`;
- COSMIC custom shortcuts run a command on key press only, with no release
  event. So `narrivy-cli` alone cannot implement hold;
- the matrix doc allows a "low-level input/device access" path as **opt-in
  only, not default**. The reference user is in the `input` group; normal
  users are not.
- The daemon today has only toggle. `TriggerKind::HoldHotkey` exists in
  `adapters/platform-linux/src/trigger.rs` and `narrivy-pipeline`, but has no
  live source.

**Work:**
1. **Toggle (default, always shipped):** unchanged path,
   `/usr/bin/narrivy-cli toggle` bound in COSMIC Settings. Complete T2.6 and
   T2.5 first.
2. **Hold (opt-in):** add a hold trigger source in `adapters/platform-linux`
   that reads key press/release for **one** user-chosen key from evdev
   (`/dev/input/event*`). It must:
   - be off by default and enabled only by an explicit setting with a clear
     explanation. Reading input devices needs `input` group membership, which
     lets that user's processes observe keystrokes; say so in the UI;
   - read only the configured key code and never log or store other key
     events;
   - handle device hotplug, suspend/resume and missed releases (a watchdog
     that stops the session if the key is no longer reported down);
   - map press to start and release to stop for the bound pipeline, reusing
     the existing atomic admission;
   - report `push_to_talk=true` in capabilities **only** while the source is
     running and healthy; otherwise the UI shows toggle only.
3. **UI:** Settings → Activation shows "Toggle" (always) and "Press and hold
   (advanced)", which is disabled with a reason when the capability is
   absent.
4. **Tests:** unit tests with a fake evdev stream:
   - press/release starts and stops exactly once;
   - rapid presses never leave capture active;
   - a lost release times out;
   - unrelated keys are ignored and never stored.

   Plus a frontend test that the hold option is disabled without the
   capability.
5. **Qualification:** extend the FUN-001 procedure (WP4 T4.2) with the six
   "PTT acceptance" checks from the matrix doc, run on the reference machine.
6. **Time box:** 2 working days. D2 (toggle **and** press-and-hold) is a
   locked owner decision, so the agent must **not** fall back to
   toggle-only on its own. If hold doesn't pass the PTT acceptance checks by
   then, **stop and ask the owner**. Report what was tried, what failed on
   COSMIC (for example no key-release event from custom shortcuts), and the
   options: keep going, a different mechanism, or a recorded owner amendment
   of D2. Only an owner answer recorded in `agent/DECISIONS.md` may move hold
   out of 1.0.

---

**Input from the Handy comparison (`09-cross-platform-architecture.md` §3a):**
the owner's Handy fork gets reliable press-and-hold on COSMIC by reading
`/dev/input` (evdev), which requires the user to be in the `input` group. That
membership lets any process of that user read every keystroke. If no portal or
compositor route gives key-release events, present this trade-off to the
owner. Do not adopt it silently.

**Also (Sol S01):** `spec/catalogs/pipelines.json:5` says the default
pipeline activates by "toggle or hold", but the canonical pipeline disables
hold and the UI hides it. Make the catalog, the UI and the capability report
say the same thing as T2.13's outcome.

## WP2 exit check

Run once, at the end:

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo deny check
(cd apps/narrivy-desktop && npm test && npm run build)
python3 -m pytest -q tests
python3 scripts/check_log_redaction.py
python3 scripts/verify_secrets.py
python3 scripts/build_manifest.py && python3 scripts/validate_pack.py
```

All must exit 0. Update `STATE.json`, then commit.
