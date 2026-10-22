# Release task 019: exact-candidate COSMIC physical-microphone runner

Implement one bounded P11 prerequisite. Codex already researched and manually
proved the flow against rc5. Work only in the listed files. Do not commit,
push, access the web, use subagents, install packages, change host audio
defaults, play audio during implementation/tests, build release binaries or a
real package, write qualification evidence, edit the evidence index/profile
contract, promote requirements, or run broad workspace/release gates.

Read:
- `AGENTS.md`
- `SOURCE_OF_TRUTH.md`
- `BUILD_TARGET.json`
- `agent/STATE.json`
- `docs/10-agent/token-efficient-workflow.md`
- `tools/narrivy-qualify/harness.py`
- `scripts/qualify_candidate.py`
- `scripts/evidence_policy.py`
- `spec/schemas/evidence.schema.json`
- `spec/scenarios/p11-cosmic-native.json`
- `docs/09-phases/phase-11-qualification.md`
- `docs/07-testing/performance-benchmark-protocol.md`
- `scripts/README.md`

Allowed files:
- `scripts/qualify_candidate_known_phrase.py` (new)
- `tests/packaging/test_qualify_candidate_known_phrase.py` (new)
- `scripts/README.md`

Use the external-worker evidence contract in
`docs/10-agent/token-efficient-workflow.md`. Apply Ponytail lite: make the
smallest standard-library implementation that is robust enough for this exact
procedure. Repository files and successful tool results are the only evidence.

## Proven manual flow

The exact rc5 package, canonical whisper.cpp `jfk.wav`, packaged daemon,
resident worker, packaged GGUF model, strict PipeWire capture, Clean pipeline
and Inbox delivery already passed on native COSMIC. The public fixture was
played through the default speakers and captured through the default physical
USB microphone. The expected fragment matched, one Inbox file existed, and the
fragment had zero ordinary-log hits. Runtime/state/extraction were deleted.

The current one-off is not reproducible reviewed evidence. The runner must make
that physical-speaker-loopback procedure deterministic, bounded and safe. It
must emit a content-safe **unqualified result only**. Codex will review it,
commit it, register a hash-pinned verification procedure separately, build a
new exact clean candidate from that later commit, and then run it.

## Required behavior

1. Add a non-interactive Python standard-library CLI with explicit absolute
   paths for an existing candidate `.deb`, a PCM WAV fixture and a new output
   JSON; expected candidate and fixture SHA-256 values; and an explicit
   `--confirm-audible-playback` flag. Reject missing confirmation before any
   process or audio interaction. The output must not already exist.
2. Verify before playback:
   - the source repository is the exact current Git toplevel, `HEAD` is clean,
     and bounded scrubbed Git calls succeed;
   - candidate and fixture are regular non-symlink files below fixed size
     bounds and match their expected hashes;
   - the candidate is a Debian archive whose bounded, unique, regular
     `usr/share/narrivy/build-info.json` is schema 2,
     `clean-unqualified-release-candidate`, `git_dirty=false`, and has
     `source_head` equal to current clean `HEAD`;
   - the fixture is RIFF/WAVE PCM16 mono 16 kHz with a bounded duration;
   - the session is native COSMIC Wayland; the default Pulse/PipeWire source is
     a physical source (reject `.monitor` and qualification/null names); and
     required fixed system helpers are present.
3. Extract only into a private temporary directory. Resolve the exact packaged
   daemon, CLI, resident worker and GGUF model; verify their identities against
   embedded build-info before starting anything. Never install the package or
   touch the existing Narrivy service/package.
4. Use private runtime and durable-state roots. Account for Narrivy's secure
   state-ancestor policy: do not place durable state under an unsafe ancestor.
   Preserve the host PipeWire connection explicitly while keeping Narrivy IPC
   and state isolated. Run strict `linux-mic-inbox`, `clean`, PipeWire,
   transcribe.cpp resident and Inbox delivery with exact worker/model path,
   size and digest bindings and a bounded 20-second capture window. Do not set
   virtual-microphone authorization.
5. Start the exact packaged Clean session, play the fixture once with fixed
   `/usr/bin/paplay` to the existing default sink, request stop, wait within a
   fixed total deadline and require idle completion, exactly one durable Inbox
   artifact, and no ignored child failure. Never alter source/sink defaults,
   volumes, mute state or modules.
6. Read the synthetic/public transcript only in memory. Require the fixed
   content-safe JFK fragment already used by the repository (`ask not what your
   country`, case-insensitive). Hash/count the final UTF-8 bytes, scan collected
   daemon/CLI logs for the fragment, require zero hits, and never print or write
   transcript text. Remove transcript-bearing runtime/state/extraction on every
   success or failure path before publishing the result.
7. Atomically create one mode-0600 JSON result only after cleanup. Include
   schema version 1, classification
   `native-cosmic-physical-microphone-known-phrase-exact-candidate-unqualified`,
   candidate/fixture/source/environment identity, content-free lifecycle and
   hash/count observations, cleanup facts, and explicit false flags for target
   insertion, requirement evidence and release qualification. A failed run may
   publish a sanitized failure result, but never partial transcript/artifact
   paths or payload. Stdout must be one concise content-free JSON summary.
8. Use argv-only bounded subprocess calls, fixed absolute helper paths, bounded
   captured output and a minimal deterministic environment. Own and terminate
   every child/process group on timeout. Do not use shell strings. Do not reuse
   `Harness` behavior that can hide startup stderr or leave its journal path
   stale after state-root replacement; either add a narrow local runner or use
   the existing class without modifying it and close those gaps in the caller.
9. Add focused hermetic tests with injected process/audio operations. Cover
   success and fail-closed boundaries: confirmation absent, path/hash/type/size,
   dirty or mismatched HEAD/build-info, unsafe/nonphysical source, wrong native
   lane, malformed WAV, missing package members, startup/playback/stop/idle
   failures, zero/multiple Inbox files, fragment miss, log leak, timeout child
   cleanup, no output overwrite, mode 0600, atomic/no-partial output, and cleanup
   on success/failure. Tests must never access real audio, D-Bus, the rc5 file,
   the network or the user's state.
10. Document the exact invocation and honesty boundary. This runner is a
    physical native supporting procedure, but its output remains unqualified
    until Codex separately reviews/hash-registers it and runs it against a clean
    exact candidate. It does not prove external target insertion, accessibility,
    GNOME, performance or release status.

## Focused checks

Run only:

```bash
python3 -m unittest tests.packaging.test_qualify_candidate_known_phrase -v
python3 -m py_compile scripts/qualify_candidate_known_phrase.py tests/packaging/test_qualify_candidate_known_phrase.py
git diff --check -- scripts/qualify_candidate_known_phrase.py tests/packaging/test_qualify_candidate_known_phrase.py scripts/README.md
```

Finish with actual files changed, exact commands/outcomes, and unresolved
assumptions. Never report a check that did not run as passed.
