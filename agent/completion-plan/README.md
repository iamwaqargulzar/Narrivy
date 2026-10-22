# Narrivy Linux 1.0 Completion Plan (first release: Pop!_OS COSMIC)

Written 2026-09-28 at commit `b1e16ce`, after a code review and a full run of
every automated check. This folder is the build guide for the coding agent
that takes Narrivy to a passing
`python3 scripts/release_gate.py --target linux-1.0`: quickly, and with as few
bugs as possible.

This plan does not replace the repository's rules. `SOURCE_OF_TRUTH.md`,
`AGENTS.md` and `docs/maintainers/agent/token-efficient-workflow.md` stay authoritative.
If this plan conflicts with them, they win: stop, record the conflict in
`agent/DECISIONS.md`, and fix whichever document is stale.

## Files in this folder

| File | What it is |
|---|---|
| `README.md` | This file: state, rules, order of work |
| `OWNER-DECISIONS.md` | Owner decisions D1–D36 (D36: Windows forward, macOS parked; the beta question is open). **Read before starting**; several change tasks, including D21 (the locked NPU → GPU pool → CPU pipeline). |
| `00-review-findings.md` | Every defect found in the review, with location, scenario, fix and test |
| `01-truth-and-hygiene.md` | WP1: make state/blocker/handoff docs truthful |
| `02-product-fixes.md` | WP2: fix the Critical/High/Medium findings |
| `03-release-build-pipeline.md` | WP3: one scripted reproducible package build + native smoke |
| `04-qualification-procedures.md` | WP4: make every release requirement recordable |
| `05-qualification-campaign.md` | WP5: freeze one commit and record all evidence |
| `06-release-evidence-and-gate.md` | WP6: release files, final gates, preserve the result |
| `08-open-source-readiness.md` | Cleanup and open-source "front door": Phase A runs as **WP1b** before the freeze, Phase B before the first push, Phase C after release (D15–D20) |
| `09-cross-platform-architecture.md` | Desktop-shell options, the per-OS adapter map, what Handy does, and finding F-C8 (`wtype` on COSMIC) |
| `10-v1-features.md` | **WP2c**: the v1 feature scope (D29, D30, D31, D32, D33): dictation polish, voice tools, notes, meetings, chat agent. Runs alongside WP2; must finish before the WP5 freeze |
| `07-post-release-side-quests.md` | **Not part of the release.** Other Linux distros (Ubuntu/GNOME first), Windows, macOS and AMD. Windows (SQ2) may start now, alongside the Linux release (D36); macOS (SQ3) is parked; the rest waits until the Pop!_OS release is properly tested |
| `PROGRESS.md` | Created by you: one line per finished task |

## How to use this plan

1. Read this README, then `AGENTS.md`, `SOURCE_OF_TRUTH.md`,
   `BUILD_TARGET.json` and `docs/maintainers/agent/token-efficient-workflow.md`.
2. Read `OWNER-DECISIONS.md`. Every decision is answered; follow the
   answers. Ask the owner only about **new** questions that come up.
3. Work through WP1 → WP6 in order. Within a WP, do tasks in the order
   written. Each task has files, steps and an acceptance check. A task is done
   only when you ran its check yourself in this session and it passed.
4. After each task, append to `agent/completion-plan/PROGRESS.md`:
   `date | task | commit | check command | result`. Keep `agent/STATE.json`
   truthful at every commit.

## State on 2026-09-28 (measured, re-measure before relying on it)

| Check | Result |
|---|---|
| `cargo fmt --check` / `clippy -D warnings` / `cargo deny check` | pass / pass / pass |
| `cargo test --workspace --locked` | 1,631 passed, 0 failed (**but** `cargo test -p narrivyd --features asr-transcribe-resident` does not compile — F-H10) |
| Front end (`npm test`) | 113 passed |
| Python (`pytest tests`) | 303 passed |
| `release_gate.py --target linux-1.0` | **FAIL, 257 errors, 0 of 94 requirements accepted** (93 after PORT-001 moved to post-1.0) |

The code is well engineered: defensive, bounded, typed errors, strong IPC
security. It is held back by a few product defects and by the evidence
process.

## The review in one screen (details in `00-review-findings.md`)

**Critical: the product does not yet do its main job**
- **F-C1:** the shipped systemd unit selects Inbox-only delivery. Text is never
  typed into apps.
- **F-C2:** the delivery ladder continues after text was already sent, so text
  gets typed twice once more than one rung is active.
- **F-C3:** there is no working text insertion on GNOME. *(Moved to side quest
  SQ1: the first release is Pop!_OS only.)*
- **F-C4:** `wtype` is not a package dependency.

**High: likely to lose a dictation or confuse users**
- **F-H1:** the capture queue holds 640 ms; a short stall fails the dictation.
- **F-H2:** a mic disconnect mid-dictation discards everything said.
- **F-H3:** dictation auto-stops after 1.5 s of silence, which contradicts the documented toggle/hold behaviour.
- **F-H4:** the hotkey fails silently when the daemon isn't running.
- **F-H5:** the unit isn't tied to the graphical session.
- **F-H6:** the systemd user unit has never been run natively.

**Second pass (whole repository; details in `00-review-findings.md` → "Second-pass findings")**
- **F-C5 (Critical):** the bundled model is rejected for every normal user
  (root-owned file vs the same-UID import rule).
- **F-C6 (Critical):** installed mode fails every dictation with
  `model_unavailable` (`session.rs:3237`).
- **F-C8 (native-verify):** conflicting reports on whether `wtype` (the only typing route) works on COSMIC. Check it first in WP3; Critical if it fails.
- **F-C7 (Critical, needs D14):** seven audio requirements (pre-roll,
  ducking, mic ranking and hotplug, PERF-006) would be "proven" on probe code
  the daemon never runs.
- **D13:** FUN-041 (aliases) and PRIV-003 (privacy zones) exist only as
  library code.
- **F-H7:** the daemon doesn't answer for 10–40 s after start (probe pipe
  deadlock plus pre-serve startup work).
- **F-H8:** non-ASCII text fails in `wtype` (no UTF-8 locale; reproduced).
- **F-H9:** Clean mode corrupts decimals, URLs and number words.
- **F-H10:** the release daemon's tests don't compile; CI skips them.
- **F-H11 (native-verify):** the overlay may steal focus while text is typed.
- **F-H12:** the UI stops receiving events after a daemon restart.
- **F-H13:** about 16 Settings controls do nothing.
- **F-H14:** the "recording" state is never shown.
- **F-H15:** remove and upgrade leave the old daemon running.
- **F-H16:** a unit test can type into your focused window.
- **F-H17:** History is on by default (D11), and its compaction can erase it.
- **F-H18:** crash dumps can store audio and transcripts. ADRs call this a release blocker; the fix is cheap.
- **F-M3 raised to High:** recovered text is lost at logout.
- **Process:**
  - P-1: the phase-report cycle can never pass;
  - P-2: duplicate evidence IDs;
  - P-3: stale SEC-004 pin;
  - P-4: recorder defects;
  - P-5: gates trust self-declared fields;
  - P-6: legacy refresh scripts;
  - P-7: stale agent-facing docs;
  - P-8: licence inventory;
  - P-9: personal data in committed files.

**Process blockers: why no evidence is accepted today**
- Recorders require a fully clean git tree but write tracked files, so a
  campaign can't run more than one of them (WP4 T4.0).
- 27 procedures still use unmeasured assertions (WP4 T4.1).
- 10 requirements have no procedure at all (WP4 T4.2).
- The fixed package path the recorders read holds a stale 2026-09-22 build
  (WP3).

## The rule that shapes WP5 and WP6

The release gate accepts evidence only if it was recorded at the current
`HEAD`, from a source tree with no changes outside evidence outputs
(`scripts/evidence_policy.py:451–454`), with unchanged recorder and package
hashes. So:

- finish **all** code, recorder, procedure and generator changes first
  (WP1–WP4);
- freeze one commit, build one package, and record every piece of evidence in
  one campaign without committing (WP5);
- pass both gates at that commit, then commit the evidence (WP6);
- any source edit after the freeze restarts WP5.

## Work package order

| WP | Depends on | Owner needed |
|---|---|---|
| WP1 Truth and hygiene | — | D8 (Tauri) |
| WP1b Open-source cleanup, Phase A (`08-open-source-readiness.md`) | WP1; parallel with WP2; **must finish before the WP5 freeze** | D15, D16 |
| WP2 Product fixes | WP1; D4, D5, D6 for specific tasks | Some tasks |
| WP3 Build pipeline + native smoke | WP2 (T2.1, T2.4, T2.5 at least); D3 | `sudo` install, microphone |
| WP4 Qualification procedures | WP1; can overlap WP2/WP3 | D9 review early |
| WP5 Campaign | WP1–WP4 all done | Present for native runs on Pop!_OS |
| WP6 Release evidence and gate | WP5 | D9, tag decision |

**Fastest safe path:**
1. Do WP1 quickly (half a day), including T1.3 items 4–5.
2. Then WP2 in this order: T2.12 items 1–3 (F-C6, F-C5, F-H10: without them
   nothing can be tested on the real package), then T2.1, T2.4, T2.2, then
   T2.12 items 4–5 (F-H7, F-H8).
3. Run WP4 T4.0, T4.6 and WP6 T6.0 (gate plumbing) in parallel, because
   they're independent of the product fixes.
4. All owner decisions are answered. D21, the speech-engine pipeline, is the owner's locked August 2026 design (`agent/RESOLVED_VERSIONS.json`). **Check that file and `agent/DECISIONS.md` before asking the owner anything.** Read `OWNER-DECISIONS.md` before starting; several answers change tasks (D5 opt-in autostart, D6/D11 unified History, D12 optional number conversion, D13 aliases, D14 mic picker).

## Out of scope for this release

**The first release is Pop!_OS 24.04 COSMIC (Wayland) only** (owner decision
2026-09-28). Ubuntu/GNOME and other Linux distributions, Windows, macOS and
AMD hardware work are **not** part of it. They follow, in that order, in
`07-post-release-side-quests.md`, each only after the previous release is
properly tested and the owner says to start. Do not let them block or delay
any WP. The release gate, TEST-010, `BUILD_TARGET.json` and the bounded-support
doc were already amended on 2026-09-28 to require only the `cosmic-wayland`
lane.

## Hard rules (the ones that matter most here)

1. **Never push** to `origin`. Only the owner decides push time.
2. **Never fabricate or hand-edit evidence**, or the CSV
   `status`/`evidence` columns. Use the registered recorders and
   `scripts/mark_requirement.py`.
3. **Never weaken a gate** to make it pass. That includes loosening a
   comparator, adding a `cargo deny` ignore, `#[ignore]`-ing a test, or moving a
   requirement to post-1.0 without a recorded owner decision.
4. **Do not use `scripts/run_gate.py`.** It is a quarantined no-op. Use
   `phase_gate.py` and `release_gate.py`.
5. **Mocks do not qualify real behaviour.** Real mic/desktop/package claims
   need native runs of the exact package.
6. **Reseal the manifest** after tracked changes:
   `python3 scripts/build_manifest.py && python3 scripts/validate_pack.py`.
7. **Small commits** on the local branch. Messages describe the behaviour
   change and end with the attribution line your harness requires.
8. **Ordinary logs never contain transcript text, audio, clipboard content,
   home paths, tokens or secrets.**
9. **Never type a password.** When `sudo` is needed, the owner types it.

## When to stop and ask the owner

Stop the loop and ask (write the question in `PROGRESS.md` and in your reply)
when:
- anything physical is needed: speaking into the mic,
  approving a portal prompt, typing a `sudo` password;
- a scope or behaviour decision is not answered in `OWNER-DECISIONS.md`;
- deleting tracked directories, installing or replacing system packages,
  tagging, pushing or publishing;
- the same blocker has failed three honest attempts (record them in
  `agent/BLOCKERS.md` first);
- a time box in a task is exceeded.

Otherwise keep working. Normal compile, test and platform errors are not stop
conditions; follow `agent/FAILURE-RECOVERY.md`.

## Definition of done

- `phase_gate.py --phase P11` and `release_gate.py --target linux-1.0` exit 0
  at the freeze commit, and the output is saved;
- the release `.deb` SHA-256 matches the evidence;
- the evidence is committed, with a re-verification recipe;
- `STATE.json` says `release_gate: passed`;
- nothing has been pushed.

## Kick-off prompt for the coding agent

> You are the implementation agent for Narrivy. Read
> `agent/completion-plan/README.md` and follow it exactly. All owner
> decisions are answered in `agent/completion-plan/OWNER-DECISIONS.md`;
> follow them, and ask me only about genuinely new questions. Work through
> WP1, WP1b (`08-open-source-readiness.md` Phase A) and WP2–WP6 in order, log each finished task in
> `agent/completion-plan/PROGRESS.md`, and stop only for the reasons listed
> under "When to stop and ask the owner". Never push.
