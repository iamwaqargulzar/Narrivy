# WP-OSS: Make Narrivy a professional, easy-to-maintain open-source project

**Goal:** a stranger who finds the GitHub repository can, within 10 minutes,
understand what Narrivy is, install it on Pop!_OS, build it from source, run
the tests, and open a good pull request. The owner can maintain it without
reading 700 files of build-process history.

**Written:** 2026-09-28, from a measured inventory of the repository (commands
below). Line numbers and counts will drift; re-measure before acting.

**How this fits the release plan (read first):**
- **Phase A (before the WP5 freeze):** structural cleanup and truthful
  documentation. Every tracked change after the freeze restarts WP5, so Phase
  A must finish first. It runs as **WP1b**, right after WP1 and in parallel
  with WP2.
- **Phase B (at release, before the owner's first public push):** the
  public "front door": README, user docs, governance files, GitHub
  configuration and the release workflow. These are docs and config. If any
  of them touches a gate-hashed path, do it before the freeze as well (see
  "Gate coupling").
- **Phase C (after the release is out and tested):** maintainability
  refactors (splitting very large files, reorganizing `scripts/`), a docs
  site, and deeper contributor tooling. These change code or pinned paths, so
  they must never delay the release.

Owner decisions for this work are D15–D20 in `OWNER-DECISIONS.md`.

---

## Rules for this work package

1. **Archive, don't delete.** Historical material moves to `archive/`
   (see the layout below) with a one-line README per folder saying what it
   was and why it's kept. Git history keeps everything anyway; the archive
   makes the move reviewable and keeps the release audit trail intact.
   Deleting is fine only for things with zero historical value: empty
   folders, generated caches, exact duplicates.
2. **Gate coupling:** `scripts/validate_pack.py`, `BUILD-PACK-MANIFEST.json`,
   `agent/evidence/INDEX.json`, the runner SHA-256 pins in
   `spec/evidence-verification-profiles.json`, and the governance tests
   reference many paths. Every move must update those references in the
   **same commit**, then run:
   ```bash
   python3 scripts/build_manifest.py && python3 scripts/validate_pack.py
   python3 -m pytest -q tests
   cargo test --workspace --locked
   ```
   Known hard references today (`scripts/validate_pack.py:7`): the root
   files `00-AUTONOMOUS-START.md`, `IMPLEMENTATION-PLAN-v1.3.md`,
   `BUILD-PACK-CHANGELOG-v1.3.md`, `MASTER-AUTONOMOUS-BUILD-PROMPT.md`,
   `SOURCE_OF_TRUTH.md`, `BUILD_TARGET.json`, `PRODUCT_CHARTER.md` and
   `AGENTS.md`, plus `agent/AUTONOMY_POLICY.md`, `agent/STATE.json`,
   `agent/PHASES.json`, `agent/ROADMAP_PHASES.json`, `agent/METRICS.csv`,
   `docs/product/requirements.md`, `docs/architecture/README.md`,
   and `spec/catalogs/*`. Find the rest with:
   ```bash
   rg -n --no-heading -o '"(docs|agent|spec|scripts|tools|packaging)/[^"]+"' scripts tests tools .github spec | sort -u
   ```
3. **Don't touch paths the release gate pins** before the release:
   `agent/evidence/**`, `spec/evidence-verification-profiles.json` runner
   paths, `spec/requirements-traceability.csv`, `agent/phase-reports/**`,
   and the recorder scripts. Reorganizing those is Phase C.
4. **One area per commit** (root, then `docs/<section>`, then `agent/`, and
   so on). Each commit leaves every check green.
5. **Docs describe what exists.** A doc about a future feature goes under
   `docs/roadmap/` or is marked `Status: proposed` in its first line. No doc
   may claim a feature, gate or command that doesn't exist. T-OSS.12 adds a
   check for this.

---

## Target repository layout

```
narrivy/
├── README.md                 # front door (rewritten, T-OSS.7)
├── LICENSE                   # MIT (exists)
├── THIRD_PARTY_NOTICES.md    # rewritten from the licence inventory (WP6 T6.4)
├── CHANGELOG.md              # new, Keep a Changelog format
├── CONTRIBUTING.md           # rewritten (T-OSS.8)
├── CODE_OF_CONDUCT.md        # new, Contributor Covenant 2.1
├── SECURITY.md               # rewritten with a real contact (D18)
├── SUPPORT.md                # new: where to ask, what to include
├── GOVERNANCE.md             # new, short: maintainers, decisions, releases
├── AGENTS.md                 # kept: rules for AI coding agents (widely recognised file)
├── SOURCE_OF_TRUTH.md        # kept for the release; merge into docs/ in Phase C
├── BUILD_TARGET.json         # kept (gate input)
├── Cargo.toml / Cargo.lock / rust-toolchain.toml / deny.toml / .editorconfig
├── justfile                  # new: one entry point for every dev task (T-OSS.9)
├── .github/                  # CI, release, templates, CODEOWNERS, dependabot (T-OSS.10)
├── crates/  engines/  adapters/  services/  apps/   # code (unchanged layout)
├── packaging/linux/          # the one packaging tree (packaging/deb and recovery merged, WP1 T1.5)
├── scripts/                  # dev, build, release and gate scripts (legacy removed)
├── tests/                    # Python governance/packaging tests
├── spec/                     # machine-readable contracts (schemas, catalogs, traceability)
├── docs/                     # current documentation only (new structure below)
├── agent/                    # live release-process state only (see T-OSS.5, D16)
└── archive/                  # historical material, read-only, one README per folder
```

The root drops from **33 tracked files to about 17**, and every remaining
file is one a contributor expects to find.

---

## Phase A — cleanup and truth (before the freeze; runs as WP1b)

### T-OSS.1 — Root files (33 → ~17)

| File | Action | Notes |
|---|---|---|
| `README.md` | **Rewrite** (T-OSS.7) | Today it says "scaffold/specification/autonomous build package, not a completed application". |
| `LICENSE` | Keep | MIT. |
| `THIRD_PARTY_NOTICES.md` | Keep, rewrite in WP6 T6.4 | Currently wrong about the bundled model (P-8). |
| `SECURITY.md` | Rewrite (T-OSS.8) | Needs a real contact (D18). |
| `CONTRIBUTING.md` | Rewrite (T-OSS.8) | |
| `AGENTS.md` | Keep | Update its "Execution" section to point at `agent/completion-plan/` and drop references to archived files. |
| `SOURCE_OF_TRUTH.md` | Keep | Gate input. Update its precedence list to the new doc paths. |
| `BUILD_TARGET.json` | Keep | Gate input. |
| `Cargo.*`, `rust-toolchain.toml`, `deny.toml`, `.editorconfig`, `.gitignore` | Keep | |
| `CODE_STYLE.md` | **Move** to `docs/development/code-style.md` | Contributors look for it under docs. |
| `PRODUCT_CHARTER.md` + `PRODUCT_VISION.md` | **Merge** into `docs/product/vision-and-scope.md` | `validate_pack.py` requires `PRODUCT_CHARTER.md`: update it in the same commit. |
| `ROADMAP.md` | **Move** to `docs/roadmap/README.md`, rewritten from `07-post-release-side-quests.md` | |
| `REQUIREMENTS.md` | **Archive** | Duplicates `spec/requirements-traceability.csv` and `docs/product/requirements.md`. |
| `REPOSITORY_STRUCTURE.md` | **Archive** | Replaced by the layout section in `CONTRIBUTING.md` and `docs/development/architecture-tour.md`. |
| `CLEAN_ROOM_POLICY.md` (5 lines) | **Fold** into `CONTRIBUTING.md` ("Don't copy code from other dictation projects") | |
| `LICENSE_POLICY.md`, `LICENSE-TODO.md` | **Archive** | Stale: the licence is chosen (MIT). The dependency policy lives in `deny.toml` plus `docs/development/dependencies.md`. |
| `00-START-HERE.md`, `00-AUTONOMOUS-START.md`, `GIVE-THIS-TO-CODING-AGENT.md`, `MASTER-AUTONOMOUS-BUILD-PROMPT.md`, `IMPLEMENTATION-PLAN-v1.3.md`, `BUILD-PACK-CHANGELOG-v1.3.md`, `VALIDATION-REPORT.md`, `VALIDATION-REPORT-v1.3.md` | **Archive** to `archive/build-pack-v1.3/` | Remove them from `validate_pack.py`'s `required` list in the same commit. Their live rules already live in `AGENTS.md` and the completion plan. |
| `BUILD-PACK-MANIFEST.json`, `BUILD-PACK-VERSION` | Keep until the release, then decide (Phase C) | The manifest is a gate input. After release, consider replacing it with the git tree hash plus the release SBOM. |
| `CHANGELOG.md`, `CODE_OF_CONDUCT.md`, `SUPPORT.md`, `GOVERNANCE.md`, `justfile` | **New** | T-OSS.8 and T-OSS.9. |

**Check:** `git ls-files | grep -v / | wc -l` ≤ 18; `validate_pack.py`, the
governance tests and `rg -n "00-START-HERE|GIVE-THIS-TO|MASTER-AUTONOMOUS|IMPLEMENTATION-PLAN-v1.3" --glob '!archive/**'`
find nothing live.

### T-OSS.2 — New documentation structure

Today: 258 docs in 21 numbered folders, mixing user, contributor, spec,
process and history. Target: **four audiences, clearly separated**.

```
docs/
├── README.md                      # index: "I want to… → read this"
├── user/                          # people who use Narrivy
│   ├── install.md                 # Pop!_OS COSMIC .deb, requirements, verify SHA-256
│   ├── getting-started.md         # first run, model install, COSMIC shortcut setup, toggle/hold
│   ├── privacy.md                 # what is stored where, for how long, how to delete it (from the real code: Inbox, journal 15 min, History D11)
│   ├── troubleshooting.md         # daemon status, journalctl, support report, common failures
│   ├── uninstall.md               # remove vs purge, the user data left behind
│   └── faq.md                     # supported desktops, languages, offline, GPU/NPU (not yet)
├── product/
│   ├── vision-and-scope.md        # charter + vision + non-goals
│   ├── requirements.md            # = today's 01-product/product-requirements.md (validate_pack path updated)
│   └── ux/                        # experience spec, design system, accessibility standard, screen contracts (real test names)
├── architecture/                  # contributors: how it works
│   ├── overview.md                # = 00-ARCHITECTURE-MAP + system-overview, merged and current
│   ├── components.md              # crate-by-crate map (what each crate owns, what it must not depend on)
│   ├── daemon-and-ipc.md
│   ├── audio-capture.md           # the real recovery.rs pw-record path
│   ├── speech-recognition.md      # resident worker, models, model import and integrity
│   ├── text-delivery.md           # routed ladder: wtype, portal (dormant), Inbox; the "types into focused app" rule
│   ├── recovery-and-storage.md    # journal, Inbox, History, retention
│   ├── security-model.md          # threat model, IPC auth, helper isolation, supply chain
│   └── pipelines-and-modules.md   # what exists in 1.0 vs roadmap, clearly marked
├── development/                   # contributors: how to work on it
│   ├── setup.md                   # dependencies, bootstrap script, `just setup`
│   ├── building-and-testing.md    # check tiers (from scripts/README.md), `just check`, mock engine usage
│   ├── code-style.md              # moved CODE_STYLE.md
│   ├── dependencies.md            # cargo-deny policy, licences, adding a dependency
│   ├── architecture-tour.md       # 30-minute guided tour of the code for a new contributor
│   └── i18n.md                    # 14-i18n merged
├── maintainers/                   # owner and release managers
│   ├── release-process.md         # build, qualify, gate, publish (from the completion plan once it's proven)
│   ├── qualification-and-evidence.md  # what the evidence gates are and why; contributors don't need this
│   ├── platform-support.md        # bounded-support lanes, capability matrix (Pop!_OS COSMIC only for 1.0)
│   └── performance-protocol.md
├── adr/                           # every ADR, renumbered paths kept, plus an index table with status
└── roadmap/                       # side quests SQ1–SQ3, SQ-AMD, R12–R14; "proposed" designs
```

**Mapping for every current folder** (decide file by file; the defaults are):

| Current | Default action |
|---|---|
| `00-ARCHITECTURE-MAP.md` | Merge into `architecture/overview.md`. Gate-required: update `validate_pack.py`. |
| `01-product/` (6) | `product-requirements.md` → `product/requirements.md` (gate path). `non-goals`, `personas`, `success-metrics` → merge into `product/vision-and-scope.md`. `competitive-positioning` → archive (internal). `feature-catalog` → merge into requirements. |
| `02-architecture/` (30) | Keep and merge into `architecture/*` those describing **existing** code: component-boundaries, daemon-ipc, state-machine, storage, text-delivery-router, model-manager, audio-architecture, dependency-rules, api-stability, configuration-and-migrations, error-recovery. Move to `roadmap/` those describing unbuilt systems: plugin-architecture, context-fabric, resource-registry, settings-portability, history-learning, pipeline-profiles, module-system (keep one "what exists in 1.0" section), compute-scheduler (accelerators are post-1.0). Archive `coverage-boost.md` and `foundation-hardening.md` (process self-assessments). |
| `03-ui-ux/` (25) | → `product/ux/`. Replace the 36 nonexistent test IDs in `screens/*.md` with real test names (WP4 T4.7). Model-lab, profiles, pipeline-editor and dictionary screens → `roadmap/` unless they ship (D13). |
| `04-platforms/` (10) | `bounded-support`, `capability-matrix`, `cosmic`, `linux`, both Linux matrices → `maintainers/platform-support.md` (+ keep the matrices as appendices). `gnome`, `kde-wlroots`, `windows`, `macos` → `roadmap/` (SQ1–SQ3). |
| `05-ml-inference/` (19) | `asr-contract`, `model-residency`, `model-catalog` (whisper only), `text-postprocessing` (Clean/Raw, current behaviour) → `architecture/speech-recognition.md`. OpenVINO/NPU, ONNX Runtime, runtime-packs, local-llm, multilingual, streaming, compute-disruptor, module-candidate-catalog, personal-benchmarking, confidence-repair, audio-enhancement → `roadmap/`. `engine-catalog`/`module-catalog`/`runtime-catalog`: generated views of `spec/catalogs`; keep one current table. |
| `06-security-privacy/` (7) | `threat-model`, `supply-chain`, `data-retention` → `architecture/security-model.md` and `user/privacy.md`. `privacy-zones`, `plugin-permissions`, `secrets` → `roadmap/` unless shipped (D13). `resource-network-policy` (agent network policy) → `maintainers/` or archive. |
| `07-testing/` (9) | `strategy`, `release-gates`, `failure-injection`, `latency-budgets` → `development/building-and-testing.md` and `maintainers/qualification-and-evidence.md`. `performance-benchmark-protocol` → `maintainers/performance-protocol.md` (gate-referenced path: update the scripts). `reference-environments`, `platform-matrix` → `maintainers/platform-support.md` (COSMIC only). |
| `08-operations/` (11) | `release-packaging`, `release-versioning`, `reproducible-builder`, `update-policy` → `maintainers/release-process.md` (fix the nonexistent commands). `logging-diagnostics`, `support-bundle` → `user/troubleshooting.md` plus `architecture/`. `error-code-registry`, `observability`, `doctor-plan`, `update-strategy`, `ci-environments` → archive, or `roadmap/` if still wanted (P-10). |
| `09-phases/` (13), `09-phases-roadmap/` (3) | Phase docs are build-process history → `archive/phases/`. R12–R14 → `roadmap/`. `validate_pack.py`/`PHASES.json` may reference phase docs: update. |
| `10-agent/` (22) | `token-efficient-workflow.md`, `definition-of-done.md`, `code-review-checklist.md` → keep under `agent/` (live agent process). Prompts `p0–p11`, grok/deepseek orchestrators, handoff template → `archive/agent-process/`. |
| `11-adrs/` (75) | → `docs/adr/`, same filenames. Add `docs/adr/README.md` with a table: number, title, status (Accepted / Superseded by NNNN / Proposed). Mark ADR-0002 and ADR-0028 superseded by ADR-0064 (T4.7). Never rewrite an accepted ADR's body; add a dated status note. |
| `12-references/` (3) | Archive (dated upstream audits). Keep one current `maintainers/upstream-versions.md` generated from `agent/RESOLVED_VERSIONS.json`. |
| `13-management/` (13) | Archive all (audits, registers, reviews, salvage inventory) to `archive/management/`. The live risk list moves into GitHub issues or `agent/BLOCKERS.md`. |
| `14-i18n/` (2) | → `development/i18n.md`. |
| `15-autonomous-build/` (4), `16-release/` (2), `CHECKLIST_MASTER.md`, `FOLDER_STRUCTURE.md`, `INDEX.md` | Archive the autonomous-build docs. `16-release/linux-1.0-qualification.md` → `maintainers/release-process.md` (COSMIC only). The old index is replaced by `docs/README.md`. |

**Expected result:** about 60–70 current docs (plus 75 ADRs) instead of 258
mixed ones, and a `docs/README.md` that routes each reader in one click.

**Check:** the link checker (T-OSS.12) passes; `rg -n "GNOME|Tauri|AppImage|run_gate.py|P0F active" docs --glob '!docs/adr/**' --glob '!docs/roadmap/**'`
returns only deliberate mentions (for example "GNOME: planned, see roadmap").

### T-OSS.3 — Scripts and tools

| Item | Action |
|---|---|
| `scripts/run_gate.py`, `build_deb.sh`, `generate_sbom.sh`, `refresh_p0…p11_evidence.py` (12 files) | Archive or delete. They're quarantined stubs or forbidden bulk refreshers (P-6). Update owner gates first (WP4 T4.6 item 4). |
| `scripts/run_grok_task.py`, `run_deepseek_task.py` (+ tests, `agent/deepseek-v4.1-flash.patch.yml`) | Agent-orchestration tooling. Move to `archive/agent-process/` or keep under `agent/tools/` (D16). They aren't product or release tooling. |
| `scripts/check-markdown-links.md` | Replace with a real link checker (T-OSS.12). |
| `scripts/__pycache__` | Delete; add `__pycache__/` to `.gitignore` if it's tracked. |
| `tools/narrivy-qualify/` (legacy runners), `tools/p0f-probes/` | Archive after WP4 T4.6 removes every gate reference to them. None of the 86 release procedures uses them. |
| `tools/reference-models/README.md` | Merge into `architecture/speech-recognition.md`. |
| `code/README.md` (salvage notes, empty folder) | Archive. |
| `assets/benchmark-audio/README.md` (promises fixtures that were never added; the real fixture is `tests/fixtures/jfk16.wav`) | Archive, or delete the folder. Fix `docs/maintainers/qualification/performance-benchmark-protocol.md` if it points here. |
| `artifacts/quarantine-2026-09-27/` | Archive (it's fabricated, superseded release evidence, correctly quarantined). |

Leave `scripts/` flat until after release: the recorder paths are pinned by
hash in the evidence profiles. Phase C T-OSS.16 reorganizes it.

### T-OSS.4 — Dead product code (only what the release decisions allow)

Remove, with tests adjusted, only when the owner decision or finding says so:
- the legacy delivery router `delivery/router.rs` and the `PlatformRouter`
  backend (F-L2, F-M15);
- the unused FFI stub `transcribe-sys` (F-L1), if nothing builds it;
- the Tauri shell `apps/narrivy-desktop/src-tauri` (D8 = A). Move
  `portal_parent.rs`, `bridge.rs` and `session_events.rs` into the GTK crate
  first; the GTK shell compiles them through `#[path]` today;
- the audio probe code, per D14;
- the core `Store`/`persistence.rs`, if it has no production caller (B29);
- `ModelManager` eviction/residency, if the daemon doesn't use it (B34).

Each removal: `rg` shows no production use, tests pass, and the traceability
rows that pointed at it are updated by the owner decision, never silently.

### T-OSS.5 — `agent/` (the release-process state)

Keep live (gate inputs and the current process): `STATE.json`, `PHASES.json`,
`ROADMAP_PHASES.json`, `METRICS.csv`, `AUTONOMY_POLICY.md`, `BLOCKERS.md`,
`DECISIONS.md`, `DECISION_POLICY.md`, `FAILURE-RECOVERY.md`,
`RESOLVED_VERSIONS.json`, `evidence/`, `phase-reports/`, `completion-plan/`,
`history/`.

Archive: `baselines/` (44), `tasks/` (29), `audits/`, `SESSION-HANDOFF-*`,
`CODING_AGENT_UPDATE_PROMPT_*`, `RESTART-HANDOFF.md` (replace it with a
two-line pointer to the completion plan), `RESEARCH_LOG.md`,
`legacy-hardening-tracker.json`, and the templates once WP6 has real ones.

Add `agent/README.md`: "This folder is the release-qualification state of
Narrivy. Contributors don't need it; maintainers see
`docs/maintainers/qualification-and-evidence.md`."

Whether `agent/` stays in the public repository at all is **D16**.

### T-OSS.6 — Crate and package metadata

For every workspace crate:
- `description`, `license = "MIT"` (via `workspace.package`), `repository`,
  `readme`, `publish = false` (none of these crates go to crates.io; say
  so explicitly);
- a short `README.md`: what it owns, what it must not depend on (from
  `docs/architecture/component-boundaries.md`), and its main types;
- a `//!` crate doc comment that matches the README;
- one version source: `workspace.package.version`, with every crate using
  `version.workspace = true`. Add a check (T-OSS.12) that the Debian
  `Version`, `apps/narrivy-desktop/package.json` `version` and the workspace
  version agree (F-M12).

`apps/narrivy-desktop/package.json`: add `"license": "MIT"`, `"private": true`,
and `"repository"`.

---

## Phase B — the public front door (at release, before the first push)

### T-OSS.7 — README.md

Structure (about 150 lines, no build-pack language):
1. Name, one-line description, badges (CI, licence, latest release). Add a
   screenshot of the Home screen and the overlay. Use real screenshots of
   the released build, not the reference PNGs.
2. **What it does:** local, private dictation for Linux. Speak, and the text
   is typed into the focused app. Offline speech recognition, nothing leaves
   your machine.
3. **Status and support:** 1.0 supports Pop!_OS 24.04 COSMIC (Wayland) and
   English (whisper tiny.en or the D4 model). Other desktops are on the
   roadmap. Link the roadmap.
4. **Install:** download the `.deb` and `SHA256SUMS` from Releases, verify,
   `sudo apt install ./narrivy_<ver>_amd64.deb`. Link `docs/user/install.md`.
5. **Quick start:** install the model, set the COSMIC shortcut, dictate.
6. **Privacy in one paragraph:** what's stored, where, for how long, and how
   to delete it. Link `docs/user/privacy.md`.
7. **Build from source:** `just setup && just check && just deb`.
8. **Contributing, security, licence and third-party notices** links.

### T-OSS.8 — Community and governance files

- **`CONTRIBUTING.md`** (rewrite):
  - quick start: clone, `just setup`, `just check`;
  - where to start: the architecture tour, `good first issue`;
  - check tiers: what to run locally, and what CI runs;
  - how to run without a microphone (the mock engine, labelled as
    test-only);
  - commit style (Conventional Commits, already the convention);
  - DCO sign-off or not (D17);
  - PR expectations (tests, docs, privacy impact, from the current PR
    template);
  - the clean-room rule;
  - adding a dependency (cargo-deny);
  - changing a persistent schema or IPC contract (ADR plus migration);
  - "you never need to touch `agent/` or evidence".
- **`CODE_OF_CONDUCT.md`:** Contributor Covenant 2.1, with the D18 contact.
- **`SECURITY.md`:**
  - supported versions;
  - report privately through GitHub Private Vulnerability Reporting and/or
    a security email (D18);
  - response targets (acknowledge within 7 days, for example);
  - scope (microphone, typing into apps, local IPC, model integrity);
  - keep the good default-principles section, trimmed to what 1.0 actually
    does (no plugins, no cloud).
- **`SUPPORT.md`:** Discussions for questions, Issues for bugs; how to
  attach a support report (`narrivy-cli support-report`, which is
  content-free); never paste transcripts.
- **`GOVERNANCE.md`:** maintainers (the owner), how decisions are made
  (issues, then an ADR for architecture), release cadence (SemVer, who
  cuts releases), and how contributors become maintainers.
- **`CHANGELOG.md`:** Keep a Changelog format. `1.0.0` lists the
  user-visible features and known limitations. Generate later entries from
  Conventional Commits (git-cliff) in Phase C.

### T-OSS.9 — One task runner: `justfile`

`just` is in Ubuntu/Pop!_OS repositories and gives contributors one
discoverable entry point:

```
just setup      # scripts/bootstrap-linux-dev.sh (prints what it will install), npm ci
just check      # fmt --check, clippy -D warnings, cargo test --workspace, resident-feature tests, npm test, pytest, cargo deny
just fmt        # cargo fmt, prettier (after T-OSS.11)
just ui         # build the React assets used by the GTK shell
just run        # run narrivyd + desktop from the tree with the mock engine (labelled), for UI work without a mic
just deb        # scripts/build_release_candidate.sh (WP3 T3.1)
just docs-check # link check + stale-term check (T-OSS.12)
```

Add `just deps`: fetch the pinned **transcribe.cpp** source (tag v0.2.3,
commit `63a44d9239d610b3908e8a66b384924cd4a77217`), verify the
`git archive` SHA-256 recorded in `engines/transcribe-cpp/BUILD-NOTES.md`, and
build it (CPU backend only) into the gitignored `code/vendor/transcribe.cpp`.
Today this is a manual "operator-built" step (found 2026-09-29): a new
contributor, or a clean CI runner, can't build the release worker without
it. WP3 T3.1's `build_release_candidate.sh` calls the same recipe.

`just check` must equal what CI's required PR job runs, so "green locally"
means "green in CI". Document it in `CONTRIBUTING.md` and
`docs/development/building-and-testing.md`.

### T-OSS.10 — GitHub configuration

- **CI** (`.github/workflows/ci.yml`), split into:
  - `pr` (required): `just check`, the link check, `check_action_pins.py`.
    Add the resident-feature tests and the full `pytest tests` (F-H10).
  - `msrv`: after the MSRV fix (Cargo.toml says 1.85 but the code needs 1.88; review group B28).
  - `portable`: Windows/macOS compile. Keep it as informational, `continue-on-error: true`
    until SQ2/SQ3, so a public PR isn't red for a post-release platform.
- **Release workflow** (`.github/workflows/release.yml`): on a `v*` tag,
  build the `.deb` with the reproducible builder and produce
  `SHA256SUMS`, the SBOM and provenance. Attach them to a **draft** GitHub
  Release; the owner publishes it. The native qualification stays manual
  (it needs the reference machine). The release notes say which frozen
  commit and evidence commit the build corresponds to.
- **`dependabot.yml`:** weekly for cargo, npm and github-actions, grouped
  (one PR per ecosystem), with actions kept on SHA pins.
- **`CODEOWNERS`:** the owner for everything. Add `/agent/`, `/spec/` and
  `/scripts/*gate*` as owner-only paths.
- **Issue templates:** keep `bug.yml` and `feature.yml`. Add
  `platform-request.yml` (for other desktops, linking the roadmap) and
  `config.yml` (disable blank issues; link Discussions and Security). The bug
  template asks for `narrivy-cli --version`, the desktop/session and the
  support report, and warns not to paste transcripts (it already does).
- **PR template:** keep it, shortened. "Requirement IDs / phase" becomes
  optional. Add "I ran `just check`".
- **Labels:** `bug`, `enhancement`, `good first issue`, `help wanted`,
  `area/daemon`, `area/ui`, `area/asr`, `area/packaging`, `area/docs`,
  `platform/cosmic`, `platform/other`, `security`, `privacy`.
- **Repository settings** (owner, in the GitHub UI): description, topics
  (`dictation`, `speech-to-text`, `linux`, `wayland`, `cosmic`, `privacy`,
  `rust`), website, branch protection on `main` (required `pr` job, linear
  history, no force-push), Private Vulnerability Reporting on, Discussions
  on.
- Remove `.github/workflows/SECURITY-NOTE.md` or fold it into
  `docs/development/dependencies.md`, once the pins note is verified current.

### T-OSS.11 — Code formatting and linting for every language

- Rust: already `rustfmt` and `clippy -D warnings`. Keep them.
- TypeScript/React: add ESLint (typescript-eslint, react-hooks, jsx-a11y)
  and Prettier. Run them once as a single formatting commit before any
  contributor PRs arrive, so later diffs stay clean.
- Python (`scripts/`, `tests/`): add `ruff` (lint and format) with a small
  config in `pyproject.toml`, and a single formatting commit.
- Add all three to `just check` and CI.

Do the one-time formatting commits **before the WP5 freeze** (they touch
recorder files whose hashes are pinned; re-pin in the same commit), or after
the release.

### T-OSS.12 — Keep the docs honest automatically

A small `scripts/check_docs.py`, run by `just docs-check` and CI:
- every relative link and every `path:line` reference in `docs/`,
  `README.md` and `CONTRIBUTING.md` resolves;
- every command in a fenced `bash` block that names a repo script or `just`
  recipe exists;
- a stale-term list (`run_gate.py`, `Tauri` outside ADRs/archive, `AppImage`,
  `GNOME` outside roadmap/ADRs, `P0F active`, `build-deb.sh`) fails the check;
- the version agreement from T-OSS.6.

### T-OSS.13 — Public history and personal data (with P-9 and D20)

Before the first push:
- scrub current files: usernames, host names, `/home/...` paths, app names
  in evidence (P-9 list);
- the owner chooses: **squash** into one "Narrivy 1.0" commit
  (recommended; the build history is in the archive and the local repo), or
  push the full history after a filter-repo scrub.

---

## Phase C — after the release is out and tested

### T-OSS.14 — Split the very large files (no behaviour change)

| File | Lines | Suggested split |
|---|---|---|
| `services/narrivyd/src/daemon/session.rs` | ~9,000 | admission, strict worker, delivery composition, provisioning commands, status/capabilities, events; tests into `session/tests/*.rs` |
| `engines/transcribe-cpp/src/resident.rs` | ~5,800 | supervisor/spawn, protocol client, stream, readiness, tests |
| `adapters/platform-linux/src/recovery.rs` | ~4,200 | capture (pw-record), wtype submitter, Inbox delivery, tests |
| `adapters/process-linux/src/lib.rs` | ~3,700 | resolve/trust, command builder, containment, tests |
| `services/narrivyd/src/daemon/policy.rs` | ~2,900 | env parsing, capability strings, pipelines, tests |
| `services/narrivyd/src/daemon/portal.rs` | ~2,600 | service, actor, tests |

Rules: pure moves first (one commit per file, `git diff --stat` shows moves
only); then the full check. Never mix a split with a fix.

### T-OSS.15 — Docs site (optional, D19)

mdBook, built from `docs/` by CI and published to GitHub Pages. It gives
search and navigation and needs no new content. Skip it if plain GitHub
markdown is enough.

### T-OSS.16 — Reorganize `scripts/`

Into `scripts/dev/`, `scripts/build/`, `scripts/release/`, `scripts/gates/`
and `scripts/qualify/`, updating every pinned runner path and hash in one
reviewed commit, then re-record at the next release. Only after 1.0.

### T-OSS.17 — Contributor growth

- label 10–15 `good first issue`s from the Low findings in
  `00-review-findings.md` (each with a pointer to the file and the test to
  add);
- Discussions categories (Q&A, Ideas, Show and tell);
- a short architecture video or diagram in `docs/architecture/overview.md`
  (Mermaid renders on GitHub).

---

## Effort (agent time, estimates)

| Part | Effort | When |
|---|---|---|
| Phase A: T-OSS.1–6 | 2–3 days | WP1b, before the freeze |
| Phase B: T-OSS.7–13 | 2–3 days, plus owner time for screenshots, settings and decisions | Before the first push; T-OSS.11/12 before the freeze if possible |
| Phase C: T-OSS.14–17 | 3–5 days, spread out | After release |

## Definition of done (Phases A and B)

- the root has ≤ 18 tracked files, all of the expected kinds;
- `docs/README.md` routes each audience; the docs check passes; no live doc
  describes a nonexistent feature, command or platform as current;
- every crate has a description, README and crate docs; there is one version
  source;
- `just check` equals the required CI job, and both pass on a clean clone;
- the community files exist, with a real security contact;
- the release workflow produces a draft release with `.deb`, `SHA256SUMS`,
  SBOM and provenance;
- the personal-data scan is clean, and the owner has chosen the history
  option;
- every release gate still passes (Phase A ran before the freeze).
