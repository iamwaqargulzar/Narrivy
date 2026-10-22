# Current handoff (P11 resumed)

`agent/STATE.json` is authoritative. Product work resumed on 2026-09-20 after
the owner requested a real, cost-bounded Grok task.

## Dark-first desktop checkpoint — 2026-09-21

The owner approved the new desktop direction. Commit `60b7f4c` implements the
dark-first shell and Home dashboard, retains complete light/system themes, and
stores the approved concept under `spec/ui/reference-images/`. The full
frontend suite passes 82/82 and the production frontend build passes. Both
palettes were inspected at narrow and 1280px desktop widths, including the
Settings screen. The UI does not advertise an unverified global shortcut or
invent runtime telemetry.

Exact clean candidate rc19 at `artifacts/releases/narrivy-linux-amd64.deb` is
bound to source `60b7f4c162708442bc37a7c793bad8903dd8d3af`, is 40,355,408
bytes, and has SHA-256
`1a7c4b8ab63e86b7fce41a2be5bb36fe4b2c001144691c4e58eb613c91334648`.
Its disposable Ubuntu 24.04 lifecycle passed all 21 steps, including install,
offline provisioning, ready restart, bounded shutdown, remove/purge, model
preservation, and owned-container removal. This remains an unqualified
candidate: native desktop, physical microphone, external-target delivery,
GNOME, and release provenance still require current evidence. Do not reuse
rc18 evidence for this source.

## Accepted checkpoint

- Tasks 006–016 are accepted in the current source: strict Raw/Clean
  composition, bounded capture and shutdown, target-routed delivery with Inbox
  recovery, desktop race fixes, atomic toggle activation, and checkpoint
  hygiene.
- Grok 4.6 is the active external worker through
  `scripts/run_grok_task.py`. Tasks are bounded, use fresh sessions and `xhigh`
  reasoning, and require independent Codex diff/test review.
- Task017 adds a fail-closed clean-source mode to the retained Debian builder;
  15 focused tests pass. It creates an exact unqualified release candidate, not
  schema-v3 provenance or release qualification.
- Exact release binaries were rebuilt from clean commit
  `df2e16e55a10d462f86d1a25ae11f7816dc38586`. Clean-source rc5 is 40,350,904
  bytes with SHA-256
  `9ac49708230908e9243e5c16bbdaf4847bd2f004a6d7b38e9263734eb1542ed5`.
  Its disposable Ubuntu 24.04 lifecycle passed all 21 install, provision,
  readiness, restart, removal and purge steps; the owned container was removed.
  The archive and receipt remain ignored, unqualified candidate artifacts.
- Task018 replaces the unsafe placeholder SBOM path with a fail-closed exact
  candidate supply-chain generator. Fifteen focused tests pass. The actual rc5
  bundle validates against schema v3 and binds SPDX (Narrivy plus 227 lockfile
  packages), in-toto provenance and all hashes to rc5/source `df2e16e...`.
  Builder identity is explicitly `local-host-unqualified` with unavailable
  image digest; result flags deny requirement, native and release qualification.
- Grok task018 reached its 18-turn cap after 1,148.18 seconds and cost $0.7184
  (1,755,620 tokens including cache). Codex independently completed review,
  added archive/epoch bounds, corrected signing truth and ran the real artifact.
- Read-only inventory confirms the live reference host is native Pop!_OS 24.04
  COSMIC Wayland with PipeWire/WirePlumber 1.6.8, a USB condenser microphone,
  Intel Core Ultra 9 288V, Lunar Lake GPU and `/dev/accel/accel0` NPU.
- Exact rc5 passed a private native admission slice without replacing installed
  Narrivy 0.2.3: both Clean and Raw started/finalized, a real root-owned
  `pw-record` child was observed at 16 kHz mono, two Inbox outputs were created
  without reading content, and SIGTERM stopped in 2.101s with socket removal.
  Transient Inbox/state/extraction was deleted. The retained ignored result is
  explicitly unqualified; known-phrase accuracy and target delivery remain open.
- Exact rc5 also passed a content-safe known-phrase slice using the canonical
  whisper.cpp JFK fixture pinned to upstream commit `5670d5c0...` and SHA-256
  `59dfb9a4...`. A temporary PipeWire null-sink monitor fed the exact packaged
  daemon, resident worker and model through the strict Clean pipeline; the
  expected fragment matched, exactly one Inbox artifact was created, and the
  fragment had zero ordinary-log hits. Transcript-bearing state was deleted,
  the physical default source was restored and the virtual device was removed.
  The ignored `known-phrase-result.json` remains explicitly controlled-fixture
  and unqualified; it does not prove physical-microphone accuracy or external
  target insertion.
- The current pack manifest covers 1088 files. Focused runner tests and a live
  read-only Grok CLI smoke pass.
- The checkpoint is local only. The public origin must not receive a push until
  the owner explicitly signals push time.

## Open release path

1. Qualify COSMIC external-target delivery and accessibility against exact rc5,
   then run CPU-fallback and performance qualification.
2. Provision the GNOME native lane and complete the eight-class target matrix
   for the same
   package hash.
3. Promote requirements only from valid current evidence, then run the release
   gate after its prerequisites change.

Historical recovery and acceptance details are under `agent/history/`.

## Native known-phrase runner checkpoint — 2026-09-21

Grok task019 created the primary physical-microphone runner but reached its
16-turn cap after 970.993 seconds; task020 used its 10 turns reading and made no
changes (84.023 seconds, $0.10896). Codex repaired shutdown/reap truth, moved the
final log scan after shutdown, removed the absolute repository path from output,
added `.deb` enforcement, documentation and 11 hermetic tests. The runner is not
yet evidence. Commit it, register its exact hash, build a clean matching rc6,
then run it on COSMIC before promoting any claim.

## Exact rc7 physical vertical-slice rehearsal — 2026-09-21

Exact clean rc7 (`b9495288...`, source `5ad7b4b`) passed the bounded native
COSMIC run through the physical Samson microphone, packaged local resident STT
worker/model, strict Clean processing and Inbox fallback. The expected phrase
matched; exactly one Inbox artifact was produced; 6,621 ordinary-log bytes had
zero phrase hits; daemon shutdown and all transient cleanup passed; the physical
default source was unchanged. The result remains ignored and unqualified.

The shutdown harness defect exposed by rc6 was fixed: SIGTERM now reaches the
daemon first so its owned worker shutdown remains authoritative; process-group
termination is timeout fallback only. Twelve focused tests pass. A minimal
schema-v2 adapter, hash-pinned fixture and reviewed `VER-TEST-009` procedure are
staged. Commit this checkpoint, build the matching package at
`artifacts/releases/narrivy-linux-amd64.deb`, then run the exact registered
command. Do not reuse or promote rc7.

## Package lifecycle procedure checkpoint — 2026-09-21

The valid rc8 `VER-TEST-009` evidence and its exact dirty patch were preserved
under ignored `artifacts/native-smoke/f1737ef-qualified-checkpoint/`, then the
tracked tree was restored clean so reviewed procedures can be committed before
the next final package.

A hash-pinned `record_package_lifecycle_evidence.py` adapter is staged for
`VER-OPS-002`. It invokes the existing bounded Docker lifecycle once against
the stable release `.deb`: fresh Ubuntu 24.04 apt install, installed paths and
runtime binding, empty-cache startup, offline model provisioning, configured
Clean readiness, bounded shutdown/restart, remove, purge, model preservation and
owned-container removal. It deliberately does not qualify upgrade, rollback,
native desktop or systemd user-service behavior. After commit, rebuild the
stable package and run this procedure once, then rerun the registered COSMIC
vertical slice on the same hash.

## Packaged CPU performance procedure checkpoint — 2026-09-21

A disposable exact-rc9 prototype exercised the package's real resident worker,
sealed model binding and private binary protocol. The worker enforced CPU,
reported final-only capability, became ready in about 121 ms, decoded the
11-second canonical fixture three times in about 781–842 ms, matched the known
phrase and wrote zero stderr bytes.

The release performance protocol now removes nonexistent fixture placeholders
and the impossible n=100 repeated-reboot Cold rule. The staged reviewed runner
records one descriptive first-load Cold observation, 10 fresh-process Warm
samples, five discarded Hot warmups and 100 Hot resident decodes. Its two narrow
procedures cover only PERF-005's conditional partial behavior and PERF-007's
residency states. Portal/capture/overlay/delivery stage metrics remain blocked.
Commit this checkpoint, build a matching stable package, run the CPU procedure,
then rerun package lifecycle and COSMIC evidence on the same hash.
