# Build / Validation Scripts

## Start

```bash
python3 scripts/doctor.py
python3 scripts/validate_pack.py
```

On Debian/Ubuntu-family development hosts, inspect prerequisites with:

```bash
scripts/bootstrap-linux-dev.sh
```

and explicitly run `--install` only after review.

## Run the app in development

```bash
scripts/dev-daemon.sh            # real dictation: microphone, resident Whisper, wtype then Inbox
scripts/dev-desktop-entry.sh     # dock/launcher icon for the dev build (--remove to undo)
cd apps/narrivy-desktop && npm run build && cargo run -p narrivy-desktop
```

`dev-daemon.sh` binds the worker and model by exact size and SHA-256
(controlled-build identity). It is for development only and is never release
evidence. Without it, `narrivyd` runs the mock lane: sessions finish
instantly and nothing is typed.

## Fuzzing the untrusted-input boundaries

```bash
scripts/fuzz-untrusted-input.sh [cases]   # longer property-test runs (default 20000 cases); not in CI
```

The same tests run with small case counts in `cargo test`.

## Documentation

```bash
python3 scripts/check_docs.py   # every relative Markdown link outside archive/ resolves
```

## Architecture/traceability

```bash
python3 scripts/verify_traceability.py
python3 scripts/check_action_pins.py
python3 scripts/verify-boundaries.py
python3 scripts/verify_dependency_boundaries.py
```

Dependency-boundary checking requires Cargo and a valid lockfile.

Windows compile check of the portable crates (PORT-001, SQ2 step 1; needs
`rustup target add x86_64-pc-windows-gnu`, no Windows C toolchain):

```bash
python3 scripts/check_windows_portable.py
```

## Verification tiers

Choose one tier that matches the current boundary. Successful lower-tier checks
are not a reason to immediately run every higher tier.

### Tier 1: edit loop

Run the smallest test that proves the changed behavior, plus formatting for the
touched language. Prefer a single Rust test target/filter, one frontend test
file, or one Python test module. Save verbose output to an ignored artifact and
inspect only the failing excerpt.

### Tier 2: local checkpoint

Run checks for affected packages and their direct integration boundary once,
after the bounded diff is reviewed. Use package-scoped `cargo test`/`clippy`,
the relevant frontend suite/build, or the relevant Python suite. Regenerate and
verify the pack manifest when shipped files changed. Record the exact commands
in the checkpoint handoff.

### Tier 3: phase or release artifact

Run the broad baseline below once for a clean checkpoint, and run phase/release
gates only when their prerequisites or evidence changed. Qualification must use
the exact packaged artifact. A gate with an unchanged recorded blocker is not a
diagnostic loop.

## Broad Rust baseline

```bash
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo deny check
```

## Phase/release

```bash
python3 scripts/phase_gate.py --phase P0
# ... through P11
python3 scripts/release_gate.py --target linux-1.0
```

Future phase acceptance specs are intentionally fail-closed until their actual commands/artifacts exist.

## Debian recovery candidates

`scripts/build_deb_candidate.py` creates and removes an install-blocked
composition probe. `scripts/build_deb_installable.py` accepts explicit absolute
paths for all release binaries, the resident worker, the registry-pinned model,
package assets, source identity, version, epoch, and a new output path. Default
mode retains a reproducible, exact-inspected **unqualified** candidate for dirty
diagnostic and install testing. `--clean-source` with `--source-root` proves the
absolute Git toplevel is clean and matches `--source-head`, then embeds
`usr/share/narrivy/build-info.json` as a clean-unqualified-release-candidate.
That exact package is still unqualified: native qualification and external
schema-v3 provenance remain required. The command never overwrites an output
and does not promote P10/P11 or produce a release artifact.

`scripts/build_release_candidate.sh VERSION` (T3.1) is the one command around
it: it refuses a dirty tree and any version outside D3 (`X.Y.Z~rcN`, `X.Y.Z`)
or not above 0.2.3, builds the front end and the release binaries into an
emptied `target/release-candidate` (the worker with Vulkan, the desktop with
`tauri/custom-protocol`), checks the model (`NARRIVY_MODEL`, default
`~/.cache/narrivy/models/`) against its registry pin, runs the builder with
`--clean-source`, copies the package to `artifacts/releases/narrivy-linux-amd64.deb`
(a package of another version moves to `narrivy-previous-amd64.deb`), builds the
supply-chain bundle under `artifacts/candidates/VERSION/` and writes
`artifacts/candidates/VERSION-build.json`. The package is still unqualified.

## Candidate lifecycle qualification

`scripts/qualify_candidate.py` accepts an explicit absolute candidate `.deb` and
its expected SHA-256, rejects mismatched inputs before execution, then exercises
one disposable test-owned Ubuntu 24.04 container: full apt installation, fixed
installed paths, empty-cache awaiting-model startup, explicit offline
provisioning through `scripts/qualify_installed_model.py`, ready worker/model
status, and actual `ListPipelines` responses requiring both real built-in modes
(`clean`, `raw`) advertised and the configured default mode (`clean`) disabled
before provisioning and enabled after verified
provisioning and on restart. It checks bounded SIGTERM shutdown with socket
removal, a second bounded
process stop proven before package removal, then removal and purge with exact
model preservation. It writes one sanitized, non-overwriting result JSON that is
explicitly **unqualified** and is not requirement evidence; the container has no
systemd user service, so nothing here is a user-service stop qualification.
`scripts/probe_installed_status.py` is the read-only status helper it copies
into that container. Pipeline observations retain only the fixed canonical mode
identities and typed enabled boolean; missing, duplicate or malformed entries
fail closed.
Advertising a ready mode does not prove microphone capture, transcription or
delivery. Native hardware and accessibility require separate qualification.

## Native known-phrase candidate probe

`scripts/qualify_candidate_known_phrase.py` runs one bounded, audible native
COSMIC probe against an exact clean candidate without installing it or touching
the existing Narrivy service. It plays the pinned public PCM16 fixture through
the current default speakers, captures it through the current default physical
microphone, and requires the packaged daemon, resident worker, model, Clean
pipeline, known fragment, Inbox recovery, log hygiene, and cleanup to succeed.

```bash
python3 scripts/qualify_candidate_known_phrase.py \
  --candidate /absolute/path/to/narrivy.deb \
  --expected-candidate-sha256 <64-lowercase-hex> \
  --fixture /absolute/path/to/jfk16.wav \
  --expected-fixture-sha256 <64-lowercase-hex> \
  --result /absolute/new/path/result.json \
  --confirm-audible-playback
```

The result is mode 0600, content-safe, non-overwriting, and explicitly
unqualified. It becomes usable evidence only after the runner is separately
reviewed and hash-registered under the evidence policy. The probe does not
prove external target insertion, accessibility, GNOME behavior, performance,
or release status.

`scripts/record_native_recognition_evidence.py` is the reviewed extension for
FUN-028/FUN-029. It first reads the exact packaged model's advertised
recognition capabilities, proves unsupported language/output/script/
translation/locale combinations are rejected before capture, then performs one
audible physical-microphone run with an advertised explicit language and one
bounded context hint. Run it only after the operator explicitly confirms that
speaker playback may begin:

```bash
python3 scripts/record_native_recognition_evidence.py \
  artifacts/releases/narrivy-linux-amd64.deb \
  tests/fixtures/jfk16.wav \
  agent/evidence/P5/VER-FUN-028.json \
  agent/evidence/P5/VER-FUN-029.json \
  --confirm-audible-playback
```

## Candidate supply-chain bundle

`scripts/build_candidate_supply_chain.py` accepts explicit absolute paths for an
existing candidate `.deb`, the source Git toplevel, and a new output directory,
plus the expected package SHA-256 and caller-supplied builder OS, image,
image-digest/status, and toolchain strings. It infers no reproducibility,
signing, qualification, or release status. Before writing, it proves the
package digest, Debian archive, schema-2 clean-unqualified-release-candidate
`usr/share/narrivy/build-info.json` member, matching package version, and that
`Cargo.lock` comes from that embedded commit via bounded Git—not the working
tree. It writes canonical SPDX-2.3, in-toto provenance bound to the exact
package digest, schema-v3 `release-artifacts.json`, and a small `result.json`.
The bundle is classified `candidate-supply-chain-unqualified`: it does not
replace native qualification, signing, immutable-builder proof, or release-gate
evidence. Publishing is atomic and never overwrites an existing output
directory. `scripts/generate_sbom.sh` is quarantined and exits 2; do not use it
for a current candidate.

After the exact package and bundle have been retained at the reviewed paths,
record OPS-004 with:

```bash
python3 scripts/record_supply_chain_evidence.py \
  artifacts/releases/narrivy-linux-amd64.deb \
  artifacts/candidates/linux-1.0-supply-chain
```

The recorder regenerates nothing. It independently checks the package and all
bundle hashes, the schema-v3 manifest, SPDX dependency/license fields and
checksums, the in-toto subject, the embedded clean-source identity, and the
truthful unsigned/not-supported signing state before atomically registering
schema-v2 package evidence. It does not claim native runtime qualification.

## Requirement evidence

Use `scripts/mark_requirement.py REQUIREMENT passed --evidence path` after evidence exists. Never mark pass first and plan to add evidence later.

## Pack manifest

`python3 scripts/build_manifest.py` regenerates the distribution manifest after specification files change. `python3 scripts/verify_manifest.py` verifies every shipped file/hash (excluding the manifest itself).

## Grok worker tasks

`python3 scripts/run_grok_task.py --task-file <path> --result-file <path>` runs
one non-interactive Grok 4.7 headless task from the repository root with
`--prompt-file`, pinned model/reasoning/sandbox/permission defaults, web and
subagents disabled, a fresh session, bounded task/output sizes, a timeout, and
an atomic JSON result. Grok implements; Codex orchestrates, reviews, tests, and
accepts. See `docs/maintainers/agent/grok-orchestrator.md` for the full contract and exit
codes.

`scripts/run_deepseek_task.py` remains in the tree as an unavailable legacy
runner. DeepSeek is not the active worker; do not invoke it for new tasks. See
`docs/maintainers/agent/deepseek-orchestrator.md` for the historical contract.
