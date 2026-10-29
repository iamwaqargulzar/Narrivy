# External Blockers

## B-LICENSE-001 — Owner license choice [RESOLVED 2026-08-21]
- Date: 2026-08-21
- Phase/Requirement: P0/P11 release_gate legal metadata (LICENSE, THIRD_PARTY_NOTICES.md)
- Why agent cannot resolve: Legal/business decision must not be invented per the owner-decision rules (archived `LICENSE_POLICY.md`; the licence is now MIT)
- Human action required: Choose SPDX (MIT/Apache-2.0 etc) + holder → agent writes LICENSE
- How to verify unblock: `ls LICENSE THIRD_PARTY_NOTICES.md` both exist, >=200/>=80 chars, no TODO, `python3 scripts/release_gate.py --target linux-1.0` license part passes
- Status: **RESOLVED** — Owner chose MIT (time being), Copyright (c) 2026 Narrivy, LICENSE 1064 chars, THIRD_PARTY_NOTICES 1482 chars, validate_pack OK (306 files). Full gate still fails on P1-P11 planned (expected).
- Independent work continued: P0 passed, P1-P10 proceed without license block

## B-PERM-002 — Interactive OS permission (portal) [PENDING — plan confirmed 2026-08-22: pre-grant via portal permission store where scriptable + short owner-present permission windows; agent degrades truthfully if owner unavailable; not blocking now]
- Date: 2026-08-21
- Phase/Requirement: P3 FUN-001/FUN-004 (global shortcut, text delivery), P4 FUN-002/011 (mic capture), P8 SEC/FUN-061 (secret store, context)
- Why agent cannot resolve: Wayland portal prompts require physical user click Allow; agent cannot grant permission autonomously per MASTER human blockers (interactive OS permission)
- Human action required: When Narrivy first probes capability on COSMIC/GNOME Wayland, click Allow on portal dialogs for: Global Shortcut, Screencast/PipeWire mic, Secret Service, Input delivery. One-time per session.
- How to verify unblock: Diagnostics screen shows capability `available` not `permission_required`; `Dial` in `capability-matrix.md` moves to `supported`
- Independent work continued: P1-P2 portable/core/IPC/mock do not need permission; P3 will implement probing and degrade gracefully to `permission_required` if not granted, gate will show degraded not failure

## B-HW-003 — Second reference env: Ubuntu 24.04/GNOME Wayland [POST-RELEASE — owner decision 2026-09-28: the first release is Pop!_OS COSMIC only; GNOME is the first post-release lane. Earlier "deferred to P11" text below is superseded.]
- Date: 2026-08-21 (deferred per owner 2026-08-21)
- Phase/Requirement: P11 TEST-010, release_gate qualification.json
- Why deferred: Owner confirmed delay to later stage — P1-P10 proceed on Pop!_OS/COSMIC host (wayland-1, PipeWire 1.5.85, Lunar Lake NPU). Agent will keep COSMIC E2E green now, and attempt GNOME VM provisioning autonomously in P11 only when owner signals ready; or owner provides GNOME box later.
- Human action required (later): When ready, either allow VM (20GB+KVM) or provide Ubuntu GNOME Wayland machine to run E2E evidence. No action now.
- How to verify unblock (later): `qualification.json` has both `cosmic-wayland e2e_pass:true` and `gnome-wayland e2e_pass:true`
- Independent work continued: P1-P10 fully autonomous on COSMIC; GNOME deferred, no gate failure now.
- **Superseded 2026-09-28 (owner):** the GNOME lane is post-release (SQ1).
  P11 does not provision or qualify GNOME, and the release gate requires only
  `cosmic-wayland`.

## B-HW-004 — AMD NPU hardware (8700G) [POST-RELEASE SIDE QUEST — owner decision 2026-09-28: not part of Linux 1.0; start only after Linux 1.0 is released and fully tested. Linux 1.0 claims no AMD accelerator; `accelerator_claims` stays empty for AMD. The P5/P11 plan lines below are historical and superseded.]
- Date: 2026-08-21 (deferred same day per owner)
- Phase/Requirement: P5/P11 additional runtime MIGraphX/Vitis AI NPU (AMD XDNA) — extra beyond Intel Lunar Lake NPU on current host (00:0b.0 NPU present)
- Why deferred: Owner will provide Ryzen 7 8700G (Phoenix, Zen4 + RDNA3 780M + XDNA NPU up to 16 TOPS) at later stage. 8700G **is sufficient** — has XDNA NPU (16 TOPS per techpowerup.com), amdxdna driver in Linux 6.14+ and Ryzen AI Software/Vitis AI EP support Phoenix. XDNA1 (8700G) LLM support more limited than XDNA2 (Strix Point) but Whisper/Parakeet NPU and Vulkan/MIGraphX GPU will qualify. No immediate need — Linux 1.0 passes via Intel NPU + GPU pool → CPU fallback (CPU fallback mandatory, AMD NPU claim optional).
- Why agent cannot resolve now: Needs AMD hardware + Linux 6.14 amdxdna + XRT SHIM + Ryzen AI 1.8. Agent will keep AMD NPU as deferred candidate, health `unqualified` until hardware arrives, fallback to GPU/CPU ensures UX.
- Human action required (later): Provide 8700G machine with Ubuntu 25.04+/Linux 6.14+ (or Pop!_OS with 6.14), install `amdxdna` + `xrt`, then agent will run `qualification.json` NPU health for `amd-npu` and `vulkan/migraphx` GPU on same box. No action now.
- How to verify unblock (later): `artifacts/release-evidence/linux-1.0/qualification.json accelerator_claims[]` for `amd-npu` shows `pass:true` with `device: Ryzen 7 8700G XDNA`, evidence file + `vulkaninfo` + `MIGraphX` health
- Independent work continued: P1-P11 proceed on Intel host; AMD 8700G qualification planned for P5 extra-Runtime + P11 optional claim. Hierarchy already `NPU pool [Intel OpenVINO + AMD XDNA] → GPU pool → CPU`, AMD slot stays deferred.
- Plan: Keep `B-HW-004` deferred, no gate failure. When hardware arrives, rerun `P5` health + `P11` dual-env (COSMIC + GNOME + AMD 8700G NPU) and update `RESOLVED_VERSIONS.json` with AMD driver versions.
- **Superseded 2026-09-28 (owner):** AMD qualification is a post-release side
  quest (SQ-AMD in `agent/completion-plan/07-post-release-side-quests.md`).
  Nothing here is a P11 task.

## B-SIGN-005 — Private signing key for .deb/AppImage [REMOVED 2026-08-21 per owner — optional for distribution]
- Date: 2026-08-21 (removed same day)
- Phase/Requirement: P10 release engineering
- Why removed: Owner confirmed signing is optional for Linux 1.0 distribution; `release_gate.py` checks `sha256` only, not signature. Unsigned `.deb`/`AppImage` with checksums fully passes gate and is distributable via GitHub Releases. Signing can be added post-1.0 if apt repo trust needed.
- Status: **REMOVED** — will ship unsigned artifacts, no human action needed now.
- Independent work: P10 will build unsigned .deb/AppImage with `sha256` in `release-artifacts.json`

## B-AUTH-006 — Hugging Face org `narrivy` publish token [IN PROGRESS 2026-08-22 — owner creating the org/repo now (guided); HF CLI already installed and verified (hf 1.28.0; note: `huggingface-cli` alias is deprecated — use `hf`). Agent expects `hf auth login` (write token, narrivy org) at the P5 owner window via secure input (never in repo/env files); local-cache fallback keeps P5 autonomous until then]
- Date: 2026-08-21
- Phase/Requirement: P5 model manager — `huggingface.co/narrivy` hosts qualified OpenVINO IR/ONNX/GGML (owner wants own library)
- Why agent cannot resolve: HF push needs `HF_TOKEN` with write to `narrivy` org — mandatory account authentication per MASTER human blockers
- Human action required: Create HF org `narrivy` (if not exists) and provide token via `huggingface-cli login` or `HF_TOKEN` env; or allow agent to keep models local `~/.cache/narrivy` and defer HF push until you run `huggingface-cli`
- How to verify unblock: `huggingface.co/narrivy/whisper-large-v3-openvino-int8` visible with model-manifest + sha256
- Independent work continued: P5 can download from upstream `openai/whisper` pinned revision, convert locally, qualify on NPU, keep models in local cache; HF push is publish step only

## Summary
- Historical account/permission/hardware decisions above retain their recorded
  status. The active release-blocking recovery register is B-RECOVERY-001,
  B-LEGAL-007, B-DESKTOP-008, B-ARCH-009, B-SUPPLY-010, and B-IPC-012.
- None may be closed from mock-only, self-reported, or manifest-resealed
  evidence. All 98 Linux-1.0 requirements remain blocked.
- **Current verification (2026-09-29):** the release-gated set is now **93**,
  after the owner moved four plugin requirements and PORT-001 to post-1.0.
  The traceability CSV marks 39 as `passed` and 54 as `blocked`. All 39
  "passed" rows hold evidence recorded at older commits, so
  `release_gate.py` accepts **0 of 93** (exit 1; 273 error lines: 140
  unmeasured assertions, 54 blocked, 39 commit mismatches, 3 missing release
  files, and the P11 report). The first release is **Pop!_OS COSMIC only**;
  GNOME is post-release. The plan to clear this register is
  `agent/completion-plan/`.

## B-RECOVERY-001 — Release evidence requalification [OPEN]

- Date: 2026-08-26
- Phase/Requirement: All Linux 1.0 release-gated requirements; P11 TEST-010 and release gate.
- Why blocked: All 148 existing schema-v1 evidence records are historical/quarantined. The previous P9/P11 runner paths can pass stubs, unavailable lanes, proxies, or ignored child failures. No clean schema-v2 evidence is registered.
- Unblock criteria: Reviewed qualification procedures emit clean schema-v2 records with an available lane, no substitutes or ignored child failures, and matching candidate artifact SHA-256 bindings. Native package-artifact E2E must then be independently recorded for COSMIC and GNOME. *(2026-09-29: COSMIC only; GNOME is post-release per the owner decision of 2026-09-28. Schema v3 with measured assertions is now required; v2 is readable but cannot qualify.)*
- Current verification: `python3 scripts/release_gate.py --target linux-1.0` exits 1; no product or release claim is marked passed.
- Current verification (2026-09-29): still exit 1; see the Summary. The note
  above ("no claim is marked passed") is out of date: 39 rows are marked
  passed in the CSV, but none is accepted at HEAD.
- Scope note: Recovery Phase 1 changes governance only; it does not alter product behavior or evidence contents.
- Safety follow-on 2026-09-04: the overlooked P10 qualification and refresh
  entry points are now import-safe quarantines matching P9/P11. They cannot
  invoke the legacy package builder, reuse the retained stale `.deb`, write
  schema-v1 evidence, or reseal release metadata. This prevents laundering but
  does not provide a replacement schema-v2 qualification procedure.


## B-LEGAL-007 — License & Third-Party Notices Review [NARROWED 2026-08-22 — MIT confirmed FINAL for 1.0 by owner; remaining action: THIRD_PARTY_NOTICES review before RC]

- Date: 2026-08-21 (legacy planning Part O)
- Phase/Requirement: P0 → P10 pre-RC → P11 release_gate legal metadata (LICENSE, THIRD_PARTY_NOTICES.md, signing, distribution accounts)
- Why agent cannot resolve: License choice and third-party-notice review are legal/business decisions per the owner-decision rules (archived `archive/build-pack-v1.3/LICENSE_POLICY.md`); agent cannot invent license or approve notices.
- Human action required (before RC): Confirm LICENSE (currently MIT 1070 chars) is final or provide SPDX alternative; review THIRD_PARTY_NOTICES.md (1200+ chars) after adding Vulkan/ONNX/model artifacts; if signing desired, provide credentials (B-SIGN-005) or confirm unsigned for 1.0; ensure distribution accounts (GH Releases, apt, HF `narrivy` org) exist if needed.
- How to verify unblock: `ls LICENSE THIRD_PARTY_NOTICES.md` both exist, >=200/>=80 chars, no TODO, `python3 scripts/release_gate.py --target linux-1.0` legal part passes, and human review is recorded (e.g., signed-off).
- Status: **OPEN** — placeholder check currently passes (LICENSE 1070, NOTICES 1200+), but human review still required before RC. Full gate still blocked on P1–P11 (expected).
- Independent work continued: P1–P10 proceed while awaiting owner review; do not disable legal gate.

## B-DESKTOP-008 — Maintained Linux desktop runtime and terminal portal cleanup [SUPERSEDED 2026-09-29]

- Superseded by ADR-0075 (owner decision D22): Tauri 2 is the desktop host on
  every OS. The GTK4 shell and `adapters/desktop-parent-linux` were removed in
  WP1 T1.4 step 6. Portal window parenting is dormant with portal-eis, so the
  cancel/revoke/unexport ordering no longer applies to a shipped path. Native
  COSMIC qualification of the Tauri shell (T1.4 step 7) is still open.
  History below is kept for reference.


- Date: 2026-08-27
- Phase/Requirement: Recovery Phase 2C desktop permission ownership; dependency
  policy; Linux release packaging.
- Why blocked: the retained Tauri 2.11.5/Wry 0.55.1 parity shell and GTK3
  desktop-parent adapter use the unmaintained GTK3 Rust binding family.
  `cargo deny` also reports `glib 0.18.5` under RUSTSEC-2024-0429. The additive
  GTK4/WebKitGTK 6 shell removes those dependencies from its own graph, but the
  old graph cannot be removed before behavioral parity.
  Separately, the current blocking IPC request requires cancel on a second
  authenticated connection, unconditional revoke, and the original terminal
  reply before a GTK parent can be unexported truthfully; restart bypasses
  ordinary exit prevention.
- Unblock criteria: an ADR selects and validates a maintained desktop runtime
  or reviewed containment without suppressing security gates; the desktop
  owner deterministically proves cancel/revoke/original-terminal/unexport
  ordering, window-close and exit behavior, restart prohibition or
  disconnect-owned cleanup, zero leaked handles/workers, and no handle in
  TypeScript/logs/disk.
- Current safe progress: `adapters/desktop-parent-linux` is independently
  reviewed and startup-disabled. It adds no new affected registry package,
  command, portal prompt, daemon composition, or release claim.
- Migration prerequisite verified 2026-09-04: desktop IPC/pipeline client
  behavior, theme persistence, and event subscription/reconnect/sequence
  handling are now framework-neutral, with Tauri macros isolated in thin
  wrappers and source guards preventing framework coupling from returning.
  The event worker has explicit shutdown/join ownership. Focused and full
  sequential regressions pass. This does not select or prove a replacement
  runtime and does not reduce the 17 advisory findings.
- Provisional replacement implemented 2026-09-04: ADR-0064 selects an additive
  GTK4 0.11.4/WebKit6 0.6.1 shell with an honest desktop Rust 1.92 MSRV,
  bounded embedded assets, deny-network WebKit policy, exact reply/event bridge,
  and explicitly joined bounded workers. A content-free native COSMIC smoke
  mounted React and completed one bridge reply. Main-window rendering is now a
  proven development prerequisite, but overlay parity, GTK4 xdg-foreign parent
  export and cancel/revoke/terminal/unexport ordering, exit behavior, clean
  packaging and native release qualification remain open.
- Independent work continued: portable/core, daemon lifecycle, Inbox recovery,
  and controlled portal/EI contracts remain testable while this release block
  is resolved.

## B-ARCH-009 — Portable IPC transport and journal boundaries [LOCALLY RESOLVED]

- Date: 2026-08-28
- Phase/Requirement: canonical component boundaries; PORT-001/002/004,
  SEC-005, TEST-002/003, REL-006.
- Why blocked: Recovery 3A moved Unix transport into `adapters/ipc-unix` and
  Recovery 3B moved Unix journal persistence into `adapters/journal-unix`.
  Local validators and Linux/MSRV checks pass, but Windows/macOS portable
  builds have not been observed on their native runners.
- Unblock criteria: implement ADR-0030 as reviewed transport and journal
  adapter slices; preserve real same-user authentication, 0700/0600 modes,
  atomic journal behavior, stale-endpoint safety, and wire compatibility; add
  validator positive/negative fixtures and actual portable compile checks.
- Current verification: the journal API is included in both boundary-policy
  protection and the native/MSRV CI package lists. Direct-dependency and
  source validators pass locally, as do Linux workspace and installed Rust
  1.85 checks. No uninstalled target or native CI result was simulated.

## B-STORAGE-013 — Recovery journal durability location and same-UID path races [PARTIALLY RESOLVED]

- Date: 2026-08-28
- Phase/Requirement: ADR-0013, FUN-005, PRIV-007, REL-002/005, SEC-006.
- Why still blocked: Recovery 4N-A moved daemon composition into a private
  durable XDG state/data root and added continuous TTL/count enforcement,
  OS-CSPRNG transaction identities, and atomic no-clobber admission. The Unix
  adapter still uses path-based checks, which cannot eliminate check/use
  substitution by a hostile process already running as the same UID.
- Unblock criteria: a reviewed storage-path/migration ADR selects the durable
  location and cleanup/upgrade behavior; a follow-on filesystem ADR either
  adopts directory-FD-relative operations (`openat`/equivalent) or explicitly
  accepts and threat-models the same-principal limitation. Native crash,
  logout/reboot, upgrade, disk/full/read-only, and recovery UX tests must pass.
- Current safe state: 0700 parent/directory and 0600 owner/type/link checks are
  fail-closed, writes/reads and scans are bounded, new transactions cannot
  overwrite an existing identity, atomic replacement and fsync failures are
  typed, crash tempfiles are reaped after validation, and corrupt fragments
  have bounded retention. Qualification daemons use isolated durable state.
  Native reboot/upgrade/disk-failure recovery remains unqualified. This is not
  release evidence.

## B-SUPPLY-010 — Unsuppressed Rust dependency advisories [OWNER-ACCEPTED 2026-09-29]

- Resolution (ADR-0075, D22): after upgrading plist 1.10.1, quick-xml 0.42.0
  and time 0.3.55, `cargo deny check` reports zero vulnerabilities. The 8
  remaining unmaintained notices (GTK3 bindings, `proc-macro-error`,
  `unic-*`) are listed by ID with reasons in `deny.toml`, accepted by the
  owner, and re-evaluated at every Tauri release. Drop them when Tauri ships
  GTK4 / WebKitGTK 6 on Linux. History below is kept for reference.
- 2026-10-26 (dependency refresh): Tauri 2.12 no longer pulls `unic-*`, so
  those five ignores are removed (3 remain). New advisories since, not
  ignored, so `cargo deny check` fails until the owner decides: zip 8.6
  (RUSTSEC-2026-0367/0368/0369, extraction; fixed only in zip 9), bincode
  and paste (unmaintained), all through `harper-core 2.13.0`, the newest
  release; zbus_names/zvariant/zvariant_derive (unmaintained, merged into
  zbus 6), through zbus 5, which ashpd 0.13 and
  tauri-plugin-single-instance 2.5 still require.


- Date: 2026-08-28
- Phase/Requirement: dependency policy, SEC-007, P10/P11 release gates.
- Why blocked: cargo-deny still reports the GTK3 binding advisory family,
  `glib 0.18.5` unsoundness, and unmaintained `proc-macro-error`/`unic-*`.
  The GTK3 family is shared by Tauri/Wry and the isolated desktop-parent
  adapter; `unic-*` enters through Tauri 2.11.5's `tauri-utils ->
  urlpattern 0.3` constraint. No safe compatible update exists in the current
  desktop runtime graph.
- Unblock criteria: a reviewed desktop-runtime/dependency/MSRV ADR selects a
  maintained graph, all affected paths are rebuilt and tested, and cargo-deny
  passes without ignores. No advisory may be waived merely to make CI green.
- Current verification: `plist` was upgraded from 1.8.0 to 1.10.0, which
  upgraded `quick-xml` from 0.38.4 to 0.41.0 and `time` from 0.3.45 to 0.3.55.
  This removed both 2026 quick-xml denial-of-service advisories and the time
  stack-exhaustion advisory without an ignore. The project toolchain workspace
  check and the exact portable-package Rust 1.85 check both pass. Cargo-deny
  now reports 17 remaining desktop-runtime advisories (all unmaintained except
  the GTK3 `glib` unsoundness); bans, licenses, and sources pass.
- Maintained-graph progress 2026-09-04: the additive GTK shell resolves
  `gtk4/gdk4 0.11.4`, `webkit6 0.6.1`, and `glib 0.22.9` without a new advisory.
  Governance fixes the exact dependencies/MSRV, network/asset/command
  boundaries, and CI prerequisites. B-SUPPLY-010 stays open until overlay and
  portal-parent parity permit complete removal of Tauri and
  `desktop-parent-linux`, after which `cargo deny check` must pass with zero
  ignores.

## B-HYGIENE-011 — Tracked nested build outputs [RESOLVED 2026-09-04]

- Date: 2026-08-28
- Phase/Requirement: repository hygiene, reproducibility, clean source
  checkout, P10 packaging.
- Resolution: `.gitignore` now uses `**/target/`, and the exact generated
  `apps/narrivy-desktop/src-tauri/target/` tree was moved to the desktop Trash.
  Its 4,955 tracked deletions remain recoverable from Git; no source file was
  inside the removed tree.
- Verification: a fresh, isolated `CARGO_TARGET_DIR` built
  `narrivy-desktop` from source with all features and the locked dependency
  graph, producing an x86-64 ELF. The build did not recreate the nested target,
  `git check-ignore` resolves the new rule, repository manifest enumeration
  already excludes every path component named `target`, and no source or
  packaging path consumes the removed nested output.
- Scope note: the isolated build directory was also moved to Trash after
  inspection. No manifest reseal, package, evidence, requirement transition,
  commit, or push followed from this cleanup.

## B-IPC-012 — Privileged IPC command authorization and same-UID limit [LOCALLY RESOLVED 2026-09-04; RELEASE EVIDENCE PENDING]

- Date: 2026-08-28
- Phase/Requirement: SEC-005, privacy-sensitive daemon commands, Linux threat
  model, release qualification.
- Resolution: ADR-0061 adds a daemon-generation-owned policy that exhaustively
  classifies all 17 commands as read-only, state-changing, privacy-sensitive,
  or destructive and applies exact-command fixed-window budgets shared across
  all authenticated connections. The policy never receives the self-declared
  role or PID. Typed `busy` reaches clients through the normal rejection path.
- Lifecycle/user-intent boundary: session controls require daemon-owned active
  session state; recovery reads require one exact transaction; portal grants
  remain attended system decisions with exact-operation cancellation; model
  provisioning retains selected bundled identity, awaiting/single-flight/
  cooldown state, and exact-operation commit-aware cancellation. History
  remains disabled and cannot read recovery storage.
- Verification: unit tests enumerate every command/class, prove foreign UID
  denial does not spend same-user capacity, prove exact-command isolation, and
  prove a new daemon policy generation resets state. A real Unix-socket test
  exhausts the provisioning-start budget across reconnects, changes the
  claimed role to `admin`, receives the same typed `busy`, restarts the daemon,
  and observes a fresh budget. Focused default and all-feature suites and
  warning-denying Clippy pass.
- Residual boundary: a malicious process already running as the same UID is
  explicitly outside per-process identity/confidentiality scope and can still
  consume shared budgets or control the user's daemon. Stronger protection
  requires a separately protected broker/capability architecture, not role,
  PID, or executable-path checks. Release requirements remain blocked until
  clean schema-v2 package-artifact evidence qualifies this behaviour.

## B-PROCESS-014 — Resident/helper artifact provenance and complete containment [OPEN]

- Date: 2026-08-31
- Phase/Requirement: Recovery Phases 4P/4Q-A; SEC-007, REL-002/006, P10/P11.
- Why blocked: strict system `pw-record` and `wtype` use a root-owned,
  pathname-revalidated boundary, and the controlled resident worker now uses
  exact digest/size validation plus an immutable sealed executable snapshot.
  Neither proves installed-package/SBOM/release provenance. The controlled
  resident model is now exact digest/size bound to a sealed descriptor, but
  that descriptor is not resolved from an installed model/package manifest.
  Process groups and parent-death signaling do not contain a hostile helper which
  creates a new session, and abrupt daemon death is not cgroup-qualified.
- Unblock criteria: extend the controlled worker/model bindings to installed
  package/model-manifest identity and the release artifact; migrate or remove
  remaining production helper families; retain bounded explicit daemon
  shutdown; and qualify abrupt death/escaped descendants on the supported
  packaged platform without weakening dependency or release gates.
- Current safe state: strict capture and wtype search only reviewed `/usr/bin`
  and `/bin`, revalidate root-owned alias/target identity at one-shot spawn,
  clear inherited environment to exact role allowlists, create an owned process
  group, and treat residual descendants or uncertain cleanup as typed failure.
  The resident executable is copied into a digest/size-verified sealed memfd,
  runs with an empty environment and no-new-privileges, and is explicitly
  closed/reaped during daemon shutdown. The model is copied once into a
  digest/size-verified mode-0400 sealed memfd, inherited by exact descriptor,
  and independently attested by every worker generation. Recovery 4Q-C1 now
  distinguishes model presence from a registry-observed exact
  `InstalledModelBinding`, but that portable observation is not an open
  descriptor and the live registry is not package or release authority.
  Recovery 4Q-C2 now implements a fixed root-owned installed-runtime manifest
  observation and composes it with the C1 registry-observed model through
  immutable descriptor snapshots. This proves only local manifest-to-worker
  and registry-to-model byte consistency. Package database membership, shipped
  artifact membership, SBOM/signature/release provenance, clean-machine
  installation, and containment qualification remain unavailable and are
  reported separately.
  ADR-0063 now makes systemd user-service cgroup ownership explicit and keeps
  all three package/recovery unit templates byte-identical: service lifetime
  follows the cgroup, stop/restart targets the whole cgroup, graceful shutdown
  is bounded to three seconds, and timeout permits final forced cleanup.
  `systemd-analyze verify` and structural negative tests pass. No unit was
  installed or started, so escaped-session/crash/logout cleanup still requires
  native packaged observation.
  This controlled state creates no release evidence and leaves all 98 Linux
  requirements blocked.

## B-PACKAGE-015 — Legacy Debian composition is untrusted [OPEN]

- Date: 2026-09-01
- Phase/Requirement: Recovery 4Q-C3 follow-on; REL-002/003/006, P10/P11.
- Why blocked: C3 can deterministically stage the fixed resident worker and
  matching installed-runtime manifest, but the legacy builder still permits
  missing/debug substitutes, stub supply-chain metadata, protected-artifact
  overwrite, and a service composition that does not select the installed
  runtime/model. The protected historical `.deb` contains neither C2 runtime
  leaf and has inconsistent embedded build identity.
- Unblock criteria: replace or quarantine the legacy builder; resolve model
  provisioning/license authority and canonical package metadata; then qualify install,
  activation, upgrade, rollback, removal, and clean-machine behavior without
  promoting local staging to provenance.
- Current safe state: C4 composes explicit release-profile inputs and the exact
  C3 runtime subtree into a deliberately non-installable temporary Debian
  candidate, independently inspects exact archive membership/modes/root
  ownership, requires two-build determinism, then deletes the archive. The
  strict service and empty-cache daemon probe fail closed. This is local
  structural composition only: no retained package, install, model, SBOM,
  provenance, evidence, requirement transition, or release claim exists.
  ADR-0062 now quarantines `scripts/build_deb.sh` before any filesystem action;
  a governance test proves the retained historical artifact and build log keep
  their exact digests. P10 runner/refresh writers are quarantined too, so the
  stale artifact cannot be promoted through legacy evidence. The replacement
  release builder and clean-machine lifecycle are still absent.
