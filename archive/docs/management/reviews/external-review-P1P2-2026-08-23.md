# External Review P1+P2 — 2026-08-23

> **2026-09-28 note:** the reviewer's own report, `agent/external-review-P1P2.md`
> (referenced below in Check 6 and in the reviewer integrity note), is **not in
> the tree** and cannot be re-derived from this summary. Everything else the
> review names still resolves: the five `agent/evidence/P2/*.json` files
> exist, and the four validator scripts exist. The counts quoted below are the
> numbers as of 2026-08-23 and do not match today
> (`verify_traceability.py` now reports 94 Linux 1.0 gated / 42 roadmap, not
> 98 / 38). This file is a historical record; do not read its numbers as
> current.

Reviewer: EXTERNAL ADVERSARIAL REVIEWER v2 (independent; did not build this repo).
Scope: phases P1 and P2 only. This file is the sole artifact written by this review.
Status: COMPLETE.

See verdict at end of file (final section).

---

## Check 1 — GATE REPLAY (done)

| command | exit | outcome |
|---|---|---|
| `python3 scripts/run_gate.py --phase P1` | 1 | All 13 evidence refreshes PASS, owner gates OK, traceability/architecture/pipeline-contracts OK, workspace tests pass; final step fails with `FAIL: phase report validation failed: phase report git_end_commit does not match current commit` |
| `python3 scripts/run_gate.py --phase P2` | 1 | Same pattern: all P2 scenarios + 10 VER-* refreshes PASS except `p2-trigger-gnome` which truthfully reports `result=fail` off-lane (COSMIC host); gate proceeds past it; final step fails identically on git_end_commit |

Anomalies found:

1. **Structural replay failure (both phases).** `scripts/phase_gate.py:439-440` requires
   `data["git_end_commit"] == HEAD`. Phase reports bind permanently: P1.json `git_end_commit=4f961f5`,
   P2.json `git_end_commit=ec2e717`. Replaying any completed phase at a later HEAD therefore always
   exits 1 regardless of code health. Every substantive gate check passes at today's HEAD d33286b;
   the failure is purely the historical-commit binding. Consequence: the documented ritual
   (AGENTS.md step 4: re-run previous passing gate) cannot succeed for P1/P2 as written.
2. **Concurrent writer mutated HEAD mid-run.** During my P1 replay, evidence was stamped at HEAD
   `d05518d`, then a commit `d33286b` ("manifest reseal after P2 gate evidence refresh", author
   `Narrivy Agent <agent@narrivy.local>`, reflog timestamp 2026-08-23 01:33:01 +0500) appeared
   seconds after my run wrote BUILD-PACK-MANIFEST.json (mtime 01:32:51). No script in
   scripts/ or tools/narrivy-qualify/ performs commits; no non-sample git hooks exist. An external
   process (concurrent builder agent loop) is committing in this repo while review runs; it swept
   up my refreshed evidence files AND this review file into commit d33286b. This makes gate
   reproduction racy: evidence binds to a SHA that stops being HEAD moments later.
3. Reviewer side effects: running the mandated gate commands regenerates `agent/evidence/P1/*` and
   `agent/evidence/P2/*` by design (refresh ritual). No other repo content was touched by me.
4. Compiler warnings present in debug/test profile (`journal.rs:253` unused var;
   `services/narrivyd/src/main.rs:901,220,225,434` unused vars/dead code). AGENTS.md requires
   warnings-as-errors only in release CI, so not a violation, but noted.

## Check 2 — EVIDENCE RE-EXECUTION (in progress)

### Check 2 results (done) — 5 records re-executed by reviewer

| record | attested command | exit | observed |
|---|---|---|---|
| P1/VER-SEC-005 | `cargo test -p narrivy-ipc -- ipc_` | 0 | 28 passed, 0 failed |
| P1/VER-OPS-006 | `cargo test -p narrivy-core --locked -- migration` | 0 | 3 passed (fresh_open / hash_tamper / interrupted_recovers) |
| P1/VER-FUN-030 | `cargo test -p narrivy-pipeline -- insert update conflict save load corrupt legacy` | 0 | 12 passed |
| P2/VER-PRIV-006 | `python3 tools/narrivy-qualify/p2_runner.py --log-hygiene` | 0 | result=pass (schema OK); NOTE: this tool writes its own evidence file (`Wrote agent/evidence/P2/VER-PRIV-006.json`) — self-attesting evidence, flagged as MINOR observation |
| P2/VER-FUN-010 | `cargo test -p narrivyd --test e2e_mock_dictation -- e2e_cancel_right_after_start_delivers_nothing` | 0 | 1 passed |

All five match their attested outcomes. No fabrication detected in this sample.

## Check 3 — ANTI-FABRICATION (in progress)

### Check 3a — trace: can mock output leak into the transcribe-cpp lane? NO (verified)

Transcript-text provenance chain when `NARRIVY_ASR_ENGINE=transcribe-cpp`:

1. `services/narrivyd/src/main.rs:144-166` (`EngineConfig::from_env`): `transcribe-cpp` maps to
   `AsrEngineKind::TranscribeCpp` ONLY under `#[cfg(feature = "asr-transcribe-cpp")]`; without the
   feature `from_env` returns Err and the daemon refuses to start (re-checked defensively at
   main.rs:183-188).
2. Worker job dispatch `services/narrivyd/src/main.rs:895-920`: closed `match` on
   `AsrEngineKind`. `Mock => Box::new(MockAsr::new(MOCK_TRANSCRIPT))`;
   `TranscribeCpp => TranscribeCppEngine::with_model_path(model)` whose init failure prints to
   stderr and `return`s — **there is no fallback edge from TranscribeCpp to MockAsr**.
3. Real text source `engines/transcribe-cpp/src/sidecar.rs` (`decode_blocking`): buffered PCM ->
   canonical 16 kHz WAV -> subprocess `$TRANSCRIBE_CPP_BIN -m <model> -q --backend cpu -o
   transcript.txt input.wav` -> trimmed UTF-8 read from `transcript.txt`, bounded by
   `DECODE_TIMEOUT = 120 s`. The Final event carries exactly that file's content.
4. Cross-checks that hold even if dispatch were subverted: mock canned text is
   `"hello world from mock asr"` (main.rs:65) which can never contain the JFK fragment;
   capabilities advertise `mock.asr` vs `asr.transcribe-cpp` (main.rs:237-245).

Two weaknesses found in the runner's checks (`tools/narrivy-qualify/p2_runner.py`):
- line ~607: `caps_ok = "asr.transcribe-cpp" in status.stdout` — substring also matches the
  degraded capability string `asr.transcribe-cpp-unavailable` (main.rs:245). Weak signal.
- lines ~626-631: if `jfk_fragment_matched == False` the scenario still records `result="pass"`
  with only a `"fragment mismatch"` note appended; the notes then claim "real GGUF transcription
  verified end-to-end". A pass with wrong-but-nonempty sidecar text is therefore recordable.
  Mitigation: in this lane nonempty text can only originate from the sidecar output file, so mock
  leakage remains impossible; rated MINOR (evidence strictness, not a leak path).

### Check 3b — real-path execution by reviewer: PASS

- Command: `python3 tools/narrivy-qualify/p2_runner.py --e2e-native-wayland` → EXIT=0,
  evidence `agent/evidence/P2/p2-e2e-native-wayland.json`: result=pass,
  `jfk_fragment_matched=true`, `inbox_artifacts=1`, `text_len=105`, `log_leak_hits=0`.
- Independent verification (not via the runner): ran the pinned sidecar myself —
  `code/vendor/transcribe.cpp/build/bin/transcribe-cli -m ~/.cache/narrivy/models/whisper-tiny.en-Q8_0.gguf -q --backend cpu -o /tmp/reviewer-out.txt ~/.cache/narrivy/fixtures/jfk16.wav`
  → EXIT=0, output text: *"And so my fellow Americans ask not what your country can do for you ask
  what you can do for your country."* (contains `ask not what your country`); 11 s audio decoded in
  258 ms (43x realtime), which explains the runner's short wall time (~0.84 s).
- Conclusion: the E2E transcript is genuinely produced by a local GGUF whisper decode of the speech
  fixture. No fabricated text detected.

### Check 3c — reviewer-designed stale-output scenarios vs cancel semantics

Method: imported `p2_runner.Daemon` read-only; custom cases beyond the shipped `--cancel-race`
(3 attempts, cancel@50 ms vs 800 ms start delay). All runs used private tempdir XDG_RUNTIME_DIR;
nothing in the repo was modified. Official rerun first: `--cancel-race` → EXIT=0, result=pass.

| case | design | observed |
|---|---|---|
| A | immediate cancel (@0 ms) | 0 delivered, 0 inbox files |
| B | tightest race: cancel @790 ms (delay=800 ms) | 0 delivered, 0 inbox files |
| C | double cancel @815 ms (just after delay expiry) and @+50 ms | both cancels REJECTED exit 1 (`IpcErrorCode::UnknownCommand`, "no active session to stop/cancel", main.rs:1056-1071) because the fast mock job had already completed and legitimately delivered pre-cancel; no post-cancel output appeared during a 2 s stale watch |
| D | control healthy session after A-C | delivers exactly `Hello world from mock asr.` (corrector contract `EXPECTED_DELIVERED`) |

Result: **zero stale outputs across all timings**; late cancels are honestly rejected rather than
silently ignored; daemon remains healthy after cancellations. Cancel semantics hold.

**Defect found (journal state conflation).** The cancelled session's journal entry records
`"state": "delivered"` while `variants.delivered = null`:
`crates/narrivy-core/src/journal.rs:18-22` defines only `Pending|Committed|Delivered` (no
Cancelled/Abandoned variant), and `services/narrivyd/src/main.rs:775-781` (`cancel_path`)
deliberately finalizes cancelled transactions as `JournalEntryState::Delivered` with the comment
"Cancelled transactions leave nothing pending in the journal". Consequences: an audit/recovery
consumer reading `state==Delivered` cannot distinguish a real delivery from a cancellation
(text presence currently discriminates, and the e2e test
`e2e_cancel_right_after_start_delivers_nothing` checks variants, not state);
`tests/e2e_mock_dictation.rs:220` counts `state==Delivered` entries, so a prior cancelled session
in the same journal would inflate that count (masked today because each test uses a fresh tempdir).
No text is fabricated and no stale delivery occurs, so this is an honesty/auditability defect, not
a delivery-safety breach.

## Check 4 — CLAIMS-VS-CODE (in progress)

### Check 4 results (done) — all six claims located and confirmed

| # | claim | code evidence | verdict |
|---|---|---|---|
| 1 | hashed transactional migrations proven against interruption | `crates/narrivy-core/src/persistence.rs` — hash = sha256 of SQL (`with_hash`, :56-58); tests named `migration_fresh_open_applies_all_and_reopen_is_noop` (:256), `migration_hash_tamper_aborts_with_actionable_error` (:281), `migration_interrupted_by_failure_recovers_on_next_open` (:295); all 3 re-executed passing in Check 2 | CONFIRMED |
| 2 | SO_PEERCRED same-user authentication | `crates/narrivy-ipc/src/security.rs:63-68` — `libc::getsockopt(fd, SOL_SOCKET, SO_PEERCRED, ...)` inside a documented unsafe block; tests `ipc_peer_credentials_same_user_validate_ok` / `_mismatch_denied` / `_real_socket_reports_self_user` passed in Check 2 | CONFIRMED |
| 3 | journal fsync+rename atomicity | `crates/narrivy-core/src/journal.rs:89-105` — "Atomic durable write: serialize -> tmp file -> `sync_all()` -> `std::fs::rename()` -> dir `sync_all()`" | CONFIRMED |
| 4 | worker bounded shutdown | `crates/narrivy-core/src/worker.rs:144-160` (`join_one` uses `rx.recv_timeout(timeout)`, maps to truthful `WorkerStop`) and `shutdown_all` (:205+) cancels all then bounded-joins each within its own timeout (REL-007); worker tests re-executed passing (VER-REL-002 lane) | CONFIRMED |
| 5 | privacy gate consults before boundary | `crates/narrivy-policy/src/lib.rs:189-222` — `GatedChannel::send` calls `engine.gate_data(class, boundary)` FIRST; only `PolicyDecision::Allow` reaches `sink.push(value)`; Deny returns typed error; order proven by test `gated_channel_consults_gate_before_inner_sink` (:409) | CONFIRMED |
| 6 | version-bump-on-save registry | `crates/narrivy-pipeline/src/lib.rs:796-810` — `update()` requires `def.version == current.version + 1`, otherwise `PipelineError::VersionBumpRejected { expected, actual }`; also re-validates cross-pipeline bindings on save | CONFIRMED |

## Check 5 — REVOCATION SWEEP (in progress)

### Check 5 results (done) — revocation sweep, 82 hits classified

Rule applied (from `agent/DECISIONS.md:211-216`, owner decision chain 2026-08-22): the dedicated
Whisper-family ENGINE/RUNTIME entity is revoked; "the family name survives ONLY in model-weights
sense"; enforced absence lives in `spec/catalogs/engines.json` (verified: 26 rows, ZERO whisper)
via `verify_architecture_map.py`.

**ENGINE-sense violations found (5 locations, all documentation, none in enforced catalogs/code):**

| location | text (excerpt) | why violation |
|---|---|---|
| `docs/architecture/speech/engine-catalog.md:5` | table row `\| whisper \| ASR \| reference \| Speech recognition… \| model \|` | whisper listed as an Engine row in the human-readable engine catalog; contradicts spec/catalogs/engines.json (26 rows, no whisper) |
| `docs/architecture/module-system.md:26` | "ASR engine \| Whisper/OpenVINO/other engines" | names Whisper as an engine class in the module trust table |
| `BUILD-PACK-CHANGELOG-v1.3.md:6` | "Made **Whisper** explicit as the Linux 1.0 reference ASR Engine." | engine-sense claim in top-level changelog |
| `VALIDATION-REPORT.md:10` and `VALIDATION-REPORT-v1.3.md:16` | "18 Engine families, with **Whisper** explicit as the Linux 1.0 reference ASR Engine." | engine-sense claim in both validation reports |
| `archive/docs/management/autonomous-build-review-2026-08-22.md:94-97` | "Whisper Vulkan builds need the bindgen/clang environment"; "whisper engine lands at P2/P5" | speaks of a future whisper ENGINE landing; post-revocation P2/P5 lanes are transcribe.cpp/Qwen3 |

All five are stale-documentation residues outside validator scope (`verify_architecture_map.py`
checks spec/catalogs + docs/architecture/README.md only). No code, no enforced catalog, no spec
JSON carries engine-sense whisper. Consolidated finding severity: MINOR each; collectively a
documentation-hygiene MAJOR because VALIDATION-REPORT*.md are the first files an auditor reads.

**MODEL-sense (allowed) — remaining 77 hits**, representative classes:
- Weight catalogs / model families: `spec/catalogs/modules.json:525-530` (explicitly "MODEL-WEIGHT
  lane only"), `engines/transcribe-cpp/src/catalog.rs:64-72` (family key "whisper" for GGUF specs),
  `runtimes.json:55` (transcribe.cpp runtime description), `engines/README.md:8`.
- Model artifacts/pins: `whisper-tiny.en-Q8_0.gguf` in `services/narrivyd/src/main.rs:28,221,937`,
  `engines/transcribe-cpp/BUILD-NOTES.md:53-60`, HF pins `openai/whisper-large-v3@rev`.
- Test fixture identities: `crates/narrivy-pipeline/tests/contracts.rs` (14x "whisper-tiny" as an
  example model resource identity).
- Mandatory upstream attribution: `THIRD_PARTY_NOTICES.md:11` (required by DECISIONS.md:144(3)).
- UI mock text naming the selected MODEL: `spec/ui/reference-images/README.md:6-7`.
- Non-model English word: `docs/development/testing/asr-evaluation.md:6` ("quiet/whisper speech").
- Competitor product name: `archive/docs/competitive-positioning.md:24` ("Superwhisper").

## Check 6 — GOVERNANCE FIVE (in progress)

### Check 6 results (done) — Governance Five

| validator | exit | result |
|---|---|---|
| `python3 scripts/validate_pack.py` | **1** | All internal checks pass (traceability, phase model, action pins, source boundaries, pipeline contracts, schemas); FAILS only in `verify_manifest.py` step: 5 hash mismatches (`agent/evidence/P2/{VER-PRIV-006,p2-cancel-race,p2-cpu-fallback,p2-e2e-native-wayland,p2-inbox-fallback}.json`) + size/hash of `agent/external-review-P1P2.md` |
| `python3 scripts/verify_owner_gates.py` | 0 | Owner-gate integrity OK: 13 gates verified |
| `python3 scripts/verify_traceability.py` | 0 | 136 catalog requirements mapped exactly once; 98 Linux 1.0 gated, 38 roadmap |
| `python3 scripts/verify_architecture_map.py` | 0 | 20 feature groups / 136 requirements, 12 pipelines, 26 engines, 66 modules, 8 runtimes (revoked whisper engine entity absence still enforced) |
| `python3 scripts/verify_pipeline_contracts.py` | 0 | 2 examples validated |

validate_pack analysis: the failing hashes are EXACTLY the files whose mtimes post-date the
working-tree manifest seal (BUILD-PACK-MANIFEST.json written 01:37:17 by my P2 gate replay;
phase_gate re-ran the four scenarios at 01:37:19, then my mandated Check-2/3b/3c executions
rewrote three more at 01:39-01:45; this report grows continuously). This is the designed
"working tree intentionally holds the refreshed evidence (one refresh ahead of HEAD)" behavior
(scripts/run_gate.py:7-8), aggravated by the concurrent committer sealing HEAD mid-review.
Not fabrication: every mismatch traces to a live, attested re-execution. A clean reseal
(`build_manifest.py` + commit) would restore consistency.

---

## Verdict: PASS-WITH-FINDINGS

P1/P2 substantive claims survived independent adversarial replay: all sampled evidence commands
exit 0 with matching outputs; the transcribe-cpp lane provably cannot emit mock text (closed
dispatch match, no fallback edge) and was proven REAL end-to-end (independent sidecar reproduction
produces the exact JFK sentence from the fixture in 258 ms); cancel semantics hold across
immediate/tight-race/post-completion timings with zero stale outputs; all six spot-checked claims
exist in code at cited locations; owner-gate/traceability/architecture-map/pipeline-contract
validators are green. No fabricated evidence was found anywhere in the sample.

## Findings table

| severity | finding | evidence |
|---|---|---|
| MAJOR | Completed-phase gate replay structurally cannot succeed: phase reports permanently bind `git_end_commit` (P1=4f961f5, P2=ec2e717) and `phase_gate.py:439-440` requires equality with current HEAD, so the AGENTS.md-mandated "re-run previous passing gate" ritual exits 1 forever after later phases land | Check 1 table: both replays exit 1 solely on this binding; every substantive check passes |
| MAJOR | Revocation-sweep residue: 5 documentation locations still speak of Whisper ENGINE-sense, contradicting the owner revocation (agent/DECISIONS.md:211-212) and the enforced catalogs (spec/catalogs/engines.json has 26 rows, zero whisper) | docs/architecture/speech/engine-catalog.md:5; docs/architecture/module-system.md:26; BUILD-PACK-CHANGELOG-v1.3.md:6; VALIDATION-REPORT.md:10 + VALIDATION-REPORT-v1.3.md:16; archive/docs/management/autonomous-build-review-2026-08-22.md:94-97 |
| MAJOR | Concurrent writer mutated repository state during review: commit d33286b ("manifest reseal after P2 gate evidence refresh", author Narrivy Agent, 01:33:01 +0500) landed between my evidence refresh and gate validation, invalidating the SHA my evidence had just bound; it also committed this review file. No script/hook in-repo performs commits | Check 1 anomaly 2; git reflog; .git/COMMIT_EDITMSG |
| MINOR | Journal conflates cancellation with delivery: `cancel_path` finalizes cancelled sessions as `JournalEntryState::Delivered` with null variants ("leave nothing pending"); enum lacks a Cancelled/Abandoned state; audit consumers reading state==Delivered mislabel cancellations as deliveries; e2e delivered-count assertion (tests/e2e_mock_dictation.rs:220) counts cancelled entries too (masked by fresh tempdirs) | services/narrivyd/src/main.rs:775-781; crates/narrivy-core/src/journal.rs:18-22; Check 3c journal dump: `"state":"delivered"` with `"delivered":null` after cancel |
| MINOR | E2E evidence strictness gaps in p2_runner: capability substring `"asr.transcribe-cpp" in stdout` also matches degraded `asr.transcribe-cpp-unavailable`; a JFK-fragment MISS records result="pass" with only a note while notes claim "verified end-to-end" | tools/narrivy-qualify/p2_runner.py ~607, ~626-631 vs services/narrivyd/src/main.rs:245 |
| MINOR | Evidence self-attestation: p2_runner writes its own pass records into agent/evidence/P2 (VER-PRIV-006 explicitly labeled "(self-written)"); mitigated by scenario_hash binding to spec/scenarios/*.json checked by the gate (phase_gate.py:350) | Check 2 row 4; scripts/phase_gate.py:350 |
| MINOR | Debug/test-profile compiler warnings (policy permits; release CI is warnings-as-errors): crates/narrivy-core/src/journal.rs:253 unused var; services/narrivyd/src/main.rs:220/225/434 dead code, :901 unused var | P1 gate replay warning output |

## Checks executed

1. GATE REPLAY — `run_gate.py --phase P1` exit 1, `--phase P2` exit 1; both fail ONLY on the
   git_end_commit-vs-HEAD binding after all substantive checks pass (13/13 P1 evidence PASS,
   9/9+scenarios P2 PASS incl. truthful off-lane GNOME fail). PARTIAL FAIL — structural, see
   Finding 1. Anomalies recorded: concurrent committer moved HEAD mid-run; warnings present.
2. EVIDENCE RE-EXECUTION — 5 records (VER-SEC-005, VER-OPS-006, VER-FUN-030, VER-PRIV-006,
   VER-FUN-010): all exit 0 with observed test counts matching attestations (28/3/12/–/1 passed).
   PASS.
3. ANTI-FABRICATION — (a) dispatch trace: mock leak into transcribe-cpp lane IMPOSSIBLE (closed
   match, no fallback, distinct capability strings, distinct canned text); two runner strictness
   gaps noted. (b) real path executed by reviewer, EXIT=0, plus INDEPENDENT sidecar reproduction
   containing "ask not what your country" — genuine GGUF decode. (c) own stale-output scenarios
   (immediate / 790 ms / post-expiry double-cancel / control): zero stale outputs; late cancels
   honestly rejected; one new defect found (journal Delivered-state conflation). PASS with
   findings.
4. CLAIMS-VS-CODE — 6/6 CONFIRMED at named locations (persistence.rs:256/281/295,
   security.rs:63-68, journal.rs:89-105, worker.rs:144-160+205, narrivy-policy lib.rs:189-222,
   pipeline lib.rs:796-810). PASS.
5. REVOCATION SWEEP — 82 hits classified; 77 MODEL-sense/other allowed, 5 ENGINE-sense violations
   (documentation only, none in enforced catalogs/code). PARTIAL FAIL — Finding 2.
6. GOVERNANCE FIVE — verify_owner_gates 0, verify_traceability 0, verify_architecture_map 0,
   verify_pipeline_contracts 0, validate_pack 1 (manifest freshness only, fully attributed to
   mandated re-execution + mid-review reseal). PASS with explained exception.

Reviewer integrity note: only `agent/external-review-P1P2.md` was authored by this review.
Mandated gate/evidence commands regenerate `agent/evidence/**` by design (documented ritual);
no source, spec, catalog, config, or history was modified by the reviewer.
