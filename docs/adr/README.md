# ADR Index

`docs/adr/` holds **75 ADRs** (ADR-0001 … ADR-0075), 5,947 lines across the
ADR files themselves. **No index existed before this file.** Until now the
directory had no README, so "what is the current accepted architecture?" was not
answerable from the ADR set alone.

Precedence: `SOURCE_OF_TRUTH.md` ranks **accepted ADRs 6th**, below the owner's
latest instruction, `BUILD_TARGET.json`, the charter/vision and the normative
requirement catalog, the current plan in `agent/completion-plan/`, and the
security/privacy/clean-room rules. An ADR is authoritative only where it does
not conflict with a higher rank.

## Known defects in this set

Recorded, not silently fixed, because changing ADR status is a governance
action:

- **No ADR is marked Superseded.** Not one of the 74 uses that status, so the
  set cannot express "this decision no longer holds" — only the weaker
  "Accepted, with a later amendment". Exactly one supersession relation is
  stated anywhere, in ADR-0034's own status line, pointing at ADR-0035.
- **Three status vocabularies are in use**, and nothing states which is
  normative: plain `Accepted` / `Proposed` (ADR-0001–0020, ADR-0074);
  `Accepted` plus a scope/qualification caveat (ADR-0021–0066); lowercased
  `accepted for recovery` / `accepted for source composition` (ADR-0067–0073).
- **Only ADR-0008 is not Accepted** (`Proposed`, plugin sandbox). No ADR uses
  `Rejected`.
- **Two ADRs are factually overtaken by the tree and still read as current:**
  ADR-0002 (Tauri 2 shell) and ADR-0028 (GTK3 Wayland parent export). Neither
  was edited by this index. See the constraint table below.
- **Status and date below are transcribed verbatim** from each file, including
  their inconsistent casing and phrasing. Read them as what each file claims,
  not as a normalized governance state.

## Genuine architecture decisions (ADR-0001 … ADR-0020)

Dated 2026-08-20 → 2026-08-26, before the Recovery program. These set the
durable boundaries: clean-room process, Rust core, capability contracts,
daemon + IPC, privacy, module boundary, artifact trust, evidence trust.

| # | ADR | Status (verbatim from the file) | Date | Lines |
|---|---|---|---|---:|
| 0001 | [ADR-0001: Greenfield clean-room implementation](ADR-0001-greenfield-cleanroom.md) | Accepted | 2026-08-20 | 13 |
| 0002 | [ADR-0002: Rust domain core with Tauri 2 desktop shell](ADR-0002-rust-core-tauri-shell.md) | Accepted | 2026-08-20 | 13 |
| 0003 | [ADR-0003: Capability-driven platform API](ADR-0003-portable-platform-api.md) | Accepted | 2026-08-20 | 13 |
| 0004 | [ADR-0004: Long-running core service with portable local IPC](ADR-0004-daemon-ipc.md) | Accepted | 2026-08-20 | 13 |
| 0005 | [ADR-0005: UI renders capabilities, not OS branches](ADR-0005-capability-driven-ui.md) | Accepted | 2026-08-20 | 13 |
| 0006 | [ADR-0006: Declarative Mode pipeline DAG](ADR-0006-pipeline-dag.md) | Accepted | 2026-08-20 | 13 |
| 0007 | [ADR-0007: Local-first with explicit data-boundary policy](ADR-0007-local-first-privacy.md) | Accepted | 2026-08-20 | 13 |
| 0008 | [ADR-0008: Sandbox portable plugins where feasible](ADR-0008-plugin-sandbox.md) | Proposed | 2026-08-20 | 13 |
| 0009 | [ADR-0009: Separate crash journal, history and learning stores](ADR-0009-history-journal-separation.md) | Accepted | 2026-08-20 | 13 |
| 0010 | [ADR-0010: Optimize end-to-end perceived latency](ADR-0010-measure-perceived-latency.md) | Accepted | 2026-08-20 | 13 |
| 0011 | [ADR-0011: Same-user local IPC is a security boundary](ADR-0011-local-ipc-security.md) | Accepted for Linux 1.0 architecture. | not stated | 28 |
| 0012 | [ADR-0012: Daemon Lifecycle](ADR-0012-daemon-lifecycle.md) | Accepted (P1, 2026-08-21) | 2026-08-21 | 62 |
| 0013 | [ADR-0013: Persistence](ADR-0013-persistence.md) | Accepted (P1, 2026-08-21) | 2026-08-21 | 87 |
| 0014 | [ADR-0014: Modular internal contracts; no public Rust dylib ABI](ADR-0014-module-extension-boundary.md) | Accepted | 2026-08-21 | 21 |
| 0015 | [ADR-0015: Lean base install with qualified optional runtime packs](ADR-0015-runtime-packs-and-auto-qualification.md) | Accepted | 2026-08-21 | 21 |
| 0016 | [ADR-0016: First-class pipelines and trigger bindings](ADR-0016-first-class-pipelines-and-triggers.md) | Accepted | 2026-08-22 | 23 |
| 0017 | [ADR-0017: Resource registry, artifact classes, and download trust](ADR-0017-resource-registry-and-artifact-trust.md) | Accepted | 2026-08-22 | 34 |
| 0018 | [ADR-0018: Text post-processing and inference providers](ADR-0018-text-postprocessing-and-inference-providers.md) | Accepted | 2026-08-22 | 28 |
| 0019 | [ADR-0019: Portable settings import/export and future sync boundary](ADR-0019-portable-settings-and-future-sync.md) | Accepted | 2026-08-22 | 16 |
| 0020 | [ADR-0020: Evidence trust registry, verification profiles, and artifact binding](ADR-0020-evidence-trust-registry-and-artifact-binding.md) | Accepted | 2026-08-26 | 64 |

## Recovery-program records (ADR-0021 … ADR-0074)

Dated 2026-08-26 → 2026-09-28. Every status line names a recovery slice
(2A/2B/2C, 3A/3B, 4A–4Q-C5B2B2) and a non-release qualification caveat. Some do
decide a boundary (process-isolated resident worker, trusted-helper execution,
journal/History separation, the GTK4 runtime, the command policy); most record
how one slice was composed. Read them for implementation truth, not as the
current architecture.

| # | ADR | Status (verbatim from the file) | Date | Lines |
|---|---|---|---|---:|
| 0021 | [ADR-0021: Recovery 2A transactional Inbox slice](ADR-0021-recovery-2a-transactional-inbox-slice.md) | Accepted for Recovery Phase 2A (2026-08-26) | 2026-08-26 | 87 |
| 0022 | [ADR-0022: Recovery 2B truthful wtype submission and Inbox recovery](ADR-0022-recovery-2b-truthful-wtype-and-inbox-recovery.md) | Accepted for Recovery Phase 2B implementation (2026-08-27) | 2026-08-27 | 117 |
| 0023 | [ADR-0023: Permission-mediated portal/EIS delivery candidate](ADR-0023-portal-eis-delivery-candidate.md) | Accepted for the non-interactive Recovery Phase 2C eligibility, | not stated | 168 |
| 0024 | [ADR-0024: Portal keyboard permission IPC contract](ADR-0024-portal-keyboard-permission-ipc-contract.md) | Accepted; production permission composition added by ADR-0066, | not stated | 123 |
| 0025 | [ADR-0025: Controlled portal/EIS text-submission contract](ADR-0025-controlled-portal-eis-text-submission-contract.md) | Accepted for Recovery Phase 2C; implemented by the non-composed | not stated | 115 |
| 0026 | [ADR-0026: Minimal Reis EI text sender](ADR-0026-minimal-reis-ei-text-sender.md) | Accepted for a non-composed Recovery Phase 2C implementation | not stated | 122 |
| 0027 | [ADR-0027: Portal service composition seam remains startup-disabled](ADR-0027-portal-service-composition-seam.md) | Accepted for controlled Recovery Phase 2C wiring (2026-08-27) | 2026-08-27 | 96 |
| 0028 | [ADR-0028: GTK3 Wayland parent export is a narrow desktop adapter](ADR-0028-gtk3-wayland-parent-export-adapter.md) | Accepted for a startup-disabled Recovery Phase 2C foundation | not stated | 121 |
| 0029 | [ADR-0029: Desktop portal-parent coordinator remains an inert state machine](ADR-0029-inert-desktop-portal-parent-coordinator.md) | Accepted for controlled Recovery Phase 2C lifecycle modeling | not stated | 112 |
| 0030 | [ADR-0030: Repair portable transport and journal boundaries](ADR-0030-portable-transport-and-journal-boundary-repair.md) | Implemented locally by transport slice Recovery 3A and journal | not stated | 103 |
| 0031 | [ADR-0031: Linux IPC principal and endpoint-race limits](ADR-0031-linux-ipc-principal-and-endpoint-race-limits.md) | Accepted clarification for Recovery 3A (2026-08-28); command | 2026-08-28 | 66 |
| 0032 | [ADR-0032: Separate Stop from Cancel and own Linux capture](ADR-0032-stop-cancel-and-owned-linux-capture.md) | Recovery 4A controlled implementation complete (2026-08-28); no release | 2026-08-28 | 119 |
| 0033 | [ADR-0033: Process-isolated resident ASR worker](ADR-0033-process-isolated-resident-asr-worker.md) | Recovery 4B controlled implementation complete (2026-08-28); | 2026-08-28 | 125 |
| 0034 | [ADR-0034: Native model streaming-capability attestation](ADR-0034-native-model-streaming-capability-attestation.md) | Recovery 4D capability decision retained; protocol-v2 framing was | not stated | 99 |
| 0035 | [ADR-0035: Bounded resident native-stream protocol](ADR-0035-bounded-resident-native-stream-protocol.md) | Recovery 4E controlled implementation complete (2026-08-28); | 2026-08-28 | 109 |
| 0036 | [ADR-0036: Recovery streaming canary model](ADR-0036-recovery-streaming-canary-model.md) | Recovery 4F candidate accepted for controlled qualification | not stated | 78 |
| 0037 | [ADR-0037: Bounded live capture transport](ADR-0037-bounded-live-capture-transport.md) | Recovery 4G adapter boundary accepted for controlled validation | not stated | 80 |
| 0038 | [ADR-0038: Streaming recovery transaction composition](ADR-0038-streaming-recovery-transaction-composition.md) | Recovery 4H controlled composition accepted (2026-08-28); | 2026-08-28 | 105 |
| 0039 | [ADR-0039: Conservative VAD and truthful endpoint origin](ADR-0039-conservative-vad-and-endpoint-origin.md) | Recovery 4I controlled composition accepted (2026-08-28); | 2026-08-28 | 101 |
| 0040 | [ADR-0040: Volatile, identity-bound partial presentation](ADR-0040-volatile-partial-presentation.md) | Recovery 4J controlled composition accepted (2026-08-29); | 2026-08-29 | 83 |
| 0041 | [ADR-0041: Notification-driven daemon event pump](ADR-0041-notification-driven-daemon-event-pump.md) | Recovery 4K controlled composition accepted (2026-08-29); | 2026-08-29 | 68 |
| 0042 | [ADR-0042: Capture-first streaming admission](ADR-0042-capture-first-streaming-admission.md) | Recovery 4L controlled composition accepted (2026-08-29); | 2026-08-29 | 77 |
| 0043 | [ADR-0043: Supervised resident readiness and strict admission](ADR-0043-supervised-resident-readiness-and-strict-admission.md) | Recovery 4M-A controlled composition accepted (2026-08-30); | 2026-08-30 | 99 |
| 0044 | [ADR-0044: Ephemeral monotonic recovery timing](ADR-0044-ephemeral-monotonic-recovery-timing.md) | Recovery 4M-B controlled diagnostic accepted (2026-08-30); | 2026-08-30 | 92 |
| 0045 | [ADR-0045: Durable recovery state and optional History separation](ADR-0045-durable-recovery-state-and-history-separation.md) | Accepted for Recovery 4N-A | 2026-08-30 | 101 |
| 0046 | [ADR-0046: Session worker terminalization ownership](ADR-0046-session-worker-terminalization-ownership.md) | Accepted for controlled Recovery 4N-B review | 2026-08-30 | 85 |
| 0047 | [ADR-0047: Disable legacy non-strict PipeWire capture](ADR-0047-disable-legacy-non-strict-pipewire-capture.md) | Accepted for controlled Recovery 4O review | 2026-08-31 | 77 |
| 0048 | [ADR-0048: Trusted Linux system-helper execution](ADR-0048-trusted-linux-system-helper-execution.md) | Accepted for controlled Recovery 4P implementation | 2026-08-31 | 127 |
| 0049 | [ADR-0049: Digest-bound resident worker and explicit daemon shutdown](ADR-0049-digest-bound-resident-worker-and-daemon-shutdown.md) | Accepted for controlled Recovery 4Q-A implementation | 2026-09-01 | 111 |
| 0050 | [ADR-0050: Immutable resident model descriptor](ADR-0050-immutable-resident-model-descriptor.md) | Accepted for controlled non-release recovery work (2026-09-01) | 2026-09-01 | 81 |
| 0051 | [ADR-0051: Verified Installed-Model Observation](ADR-0051-verified-installed-model-observation.md) | Accepted for controlled Recovery 4Q-C1 | 2026-09-01 | 75 |
| 0052 | [ADR-0052: Installed runtime manifest binding](ADR-0052-installed-runtime-manifest-binding.md) | Accepted design for controlled Recovery 4Q-C2 (2026-09-01); | 2026-09-01 | 162 |
| 0053 | [ADR-0053: Isolated installed-runtime package staging](ADR-0053-isolated-runtime-package-staging.md) | Accepted design for controlled Recovery 4Q-C3 (2026-09-01); | 2026-09-01 | 84 |
| 0054 | [ADR-0054: Disposable Debian candidate composition](ADR-0054-disposable-debian-candidate-composition.md) | Accepted design for controlled Recovery 4Q-C4 (2026-09-01); | 2026-09-01 | 102 |
| 0055 | [ADR-0055: Complete-directory offline model import](ADR-0055-complete-directory-offline-model-import.md) | Accepted design for controlled Recovery 4Q-C5A (2026-09-02); | 2026-09-02 | 99 |
| 0056 | [ADR-0056: Truthful degraded installed-model lifecycle](ADR-0056-truthful-degraded-installed-model-lifecycle.md) | Accepted design for controlled Recovery 4Q-C5B1 (2026-09-03); | 2026-09-03 | 77 |
| 0057 | [ADR-0057: Cooperative, commit-aware offline model import](ADR-0057-cooperative-commit-aware-offline-model-import.md) | Accepted design for controlled Recovery 4Q-C5B2A (2026-09-03); | 2026-09-03 | 81 |
| 0058 | [ADR-0058: Bounded immutable resident activation](ADR-0058-bounded-immutable-resident-activation.md) | Accepted design for controlled Recovery 4Q-C5B2B0 (2026-09-03); | 2026-09-03 | 72 |
| 0059 | [ADR-0059: Daemon-owned offline model provisioning protocol](ADR-0059-daemon-owned-offline-model-provisioning-protocol.md) | Accepted wire contract for controlled Recovery 4Q-C5B2B1 | not stated | 57 |
| 0060 | [ADR-0060: Daemon-owned offline model provisioning lifecycle](ADR-0060-daemon-owned-offline-model-provisioning-lifecycle.md) | Implemented for controlled Recovery 4Q-C5B2B2 (2026-09-04); | 2026-09-04 | 71 |
| 0061 | [ADR-0061: Daemon-owned IPC command authorization and rate matrix](ADR-0061-daemon-ipc-command-policy.md) | Accepted for local implementation (2026-09-04); release evidence | 2026-09-04 | 107 |
| 0062 | [ADR-0062: Quarantine the legacy Debian builder](ADR-0062-quarantine-legacy-debian-builder.md) | Accepted recovery safety boundary (2026-09-04); no replacement release | 2026-09-04 | 57 |
| 0063 | [ADR-0063: Explicit systemd cgroup ownership for daemon descendants](ADR-0063-systemd-cgroup-helper-containment.md) | Accepted structural package prerequisite (2026-09-04); native | 2026-09-04 | 65 |
| 0064 | [ADR-0064: Provisional GTK4/WebKitGTK 6 desktop runtime](ADR-0064-gtk4-webkitgtk6-desktop-runtime.md) | Accepted additive migration (2026-09-04); overlay, portal-parent and | 2026-09-04 | 128 |
| 0065 | [ADR-0065: Absolute deadlines for client handshake/request frames](ADR-0065-absolute-client-frame-deadlines.md) | Accepted implementation prerequisite, 2026-09-05 | 2026-09-05 | 51 |
| 0066 | [ADR-0066: Production portal permission composition](ADR-0066-production-portal-permission-composition.md) | Accepted for source composition; attended behavior and delivery remain unqualified (2026-09-11) | 2026-09-11 | 49 |
| 0067 | [ADR-0067: Retained installable Debian candidate](ADR-0067-retained-installable-debian-candidate.md) | accepted for recovery; release qualification pending | 2026-09-11 | 55 |
| 0068 | [ADR-0068: Persistent resident worker spawn ownership](ADR-0068-persistent-resident-spawn-ownership.md) | accepted for recovery; release qualification pending | 2026-09-14 | 138 |
| 0069 | [ADR-0069: One bounded daemon shutdown budget](ADR-0069-one-bounded-daemon-shutdown-budget.md) | accepted for recovery; release qualification pending | 2026-09-14 | 156 |
| 0070 | [ADR-0070: Canonical built-in mode identities for the strict installed composition](ADR-0070-canonical-mode-identity-strict-composition.md) | accepted for recovery; release qualification pending | 2026-09-14 | 123 |
| 0071 | [ADR-0071: Stop-controlled bounded dictation capture and typed capture end reason](ADR-0071-stop-controlled-bounded-dictation-capture.md) | accepted for recovery; release qualification pending | 2026-09-15 | 174 |
| 0072 | [ADR-0072: Admission-bound target routed delivery composition with durable Inbox recovery](ADR-0072-admission-bound-target-routed-delivery-composition.md) | accepted for source composition; installed default unchanged; native target-app qualification pending | 2026-09-14 (revised 2026-09-15 after review) | 230 |
| 0073 | [ADR-0073: Reconcile committed crash recovery records to the Inbox at startup](ADR-0073-startup-recovery-to-inbox.md) | accepted for source composition; exact-package qualification pending | 2026-09-21 | 50 |
| 0074 | [ADR-0074: Injected Host-Fact and Cache-Protection Contracts](ADR-0074-injected-host-probe-and-cache-file-policy.md) | Accepted | 2026-09-28 | 123 |

## Constrained by later ADRs

Verified by reading both files; the "current state" column is checkable
against the tree.

| ADR | Constrained by | What the later decision actually did |
|---|---|---|
| [ADR-0002](ADR-0002-rust-core-tauri-shell.md) | [ADR-0064](ADR-0064-gtk4-webkitgtk6-desktop-runtime.md) | GTK4/WebKitGTK 6 added as an **additive** migration. ADR-0064's own removal gate keeps Tauri in the tree, so ADR-0002 is not formally superseded. Meanwhile `Cargo.toml:28` excludes `apps/narrivy-desktop/src-tauri` from the workspace, so the Tauri shell is not built by the workspace at all. **ADR-0002's `Accepted` status is misleading and should be re-decided by the owner** — this file did not change it. |
| [ADR-0028](ADR-0028-gtk3-wayland-parent-export-adapter.md) | ADR-0064 amendment, 2026-09-05 | The adapter is no longer GTK3: `adapters/desktop-parent-linux/Cargo.toml:18-21` pins `gdk4-wayland 0.11.4`, `gtk4 0.11.4`, `wayland-client 0.31.15`, `wayland-protocols 0.32.13`. ADR-0064 states this "supersedes statements above describing that adapter itself as retained GTK3", but ADR-0028's own body still says GTK3 throughout. |
| [ADR-0029](ADR-0029-inert-desktop-portal-parent-coordinator.md) | [ADR-0066](ADR-0066-production-portal-permission-composition.md) | The "inert state machine" is now composed: ADR-0066 connects the GTK4 xdg-foreign adapter, the coordinator, deadline-bound IPC workers and the daemon-owned direct-zbus portal service. |
| [ADR-0024](ADR-0024-portal-keyboard-permission-ipc-contract.md) | [ADR-0066](ADR-0066-production-portal-permission-composition.md) | Self-declared in its own status line: "production permission composition added by ADR-0066". |
| [ADR-0023](ADR-0023-portal-eis-delivery-candidate.md), [ADR-0025](ADR-0025-controlled-portal-eis-text-submission-contract.md) | [ADR-0026](ADR-0026-minimal-reis-ei-text-sender.md) | Both record the contract; ADR-0026 supplies the actual non-composed sender (minimal Reis EI), because libei 1.6 text-sender APIs are unavailable on the support host. |
| [ADR-0034](ADR-0034-native-model-streaming-capability-attestation.md) | [ADR-0035](ADR-0035-bounded-resident-native-stream-protocol.md) | The **only** supersession relation stated in the set, in ADR-0034's own status line: "protocol-v2 framing was superseded by protocol v3 in ADR-0035". Still not marked `Superseded`. |
| [ADR-0031](ADR-0031-linux-ipc-principal-and-endpoint-race-limits.md) | [ADR-0061](ADR-0061-daemon-ipc-command-policy.md) | ADR-0031 established that `SO_PEERCRED` authenticates a UID, not an executable; ADR-0061 builds the exhaustive daemon-owned command policy on that limit. |
| [ADR-0045](ADR-0045-durable-recovery-state-and-history-separation.md) | implementation, not an ADR | The *separation* still holds. Its "History is not composed" wording is stale — `services/narrivyd/src/daemon/retained_history.rs` now composes the store. A dated amendment was added to that file; the Decision text was left intact. |
| [ADR-0062](ADR-0062-quarantine-legacy-debian-builder.md) | [ADR-0067](ADR-0067-retained-installable-debian-candidate.md) | Quarantine held the immediate risk; ADR-0067 permits a retained installable candidate so the real installed application can be exercised. |
| [ADR-0059](ADR-0059-daemon-owned-offline-model-provisioning-protocol.md) | [ADR-0060](ADR-0060-daemon-owned-offline-model-provisioning-lifecycle.md) | Protocol first, then the lifecycle that activates the stages it left unavailable. |
| [ADR-0068](ADR-0068-persistent-resident-spawn-ownership.md) | [ADR-0069](ADR-0069-one-bounded-daemon-shutdown-budget.md) | One bounded shutdown budget across all resident-worker owners. |
| [ADR-0071](ADR-0071-stop-controlled-bounded-dictation-capture.md) | [ADR-0072](ADR-0072-admission-bound-target-routed-delivery-composition.md) | ADR-0072 builds admission-bound routed delivery on ADR-0071's typed capture-end reason. |

## Where the desktop runtime is actually decided

[ADR-0064](ADR-0064-gtk4-webkitgtk6-desktop-runtime.md), amended 2026-09-05 and
2026-09-11, plus the tree: the maintained Linux shell is
`apps/narrivy-desktop/src-gtk` (`narrivy-desktop-gtk`, exact `gtk4 0.11.4` /
`webkit6 0.6.1`, Rust 1.92 MSRV) and it is the workspace member named
`narrivy-desktop`. The Tauri shell is excluded from the workspace. Use ADR-0064,
not ADR-0002, to answer "what hosts the React client".

## Records after this index was written

| # | ADR | Status | Date |
|---|---|---|---|
| 0075 | [ADR-0075: Tauri 2 desktop host on every OS, keeping the daemon architecture](ADR-0075-tauri-everywhere-with-daemon.md) | Accepted | 2026-09-29 |
| 0076 | [ADR-0076: GPU acceleration in the resident worker, self-tested, with CPU fallback](ADR-0076-gpu-acceleration-in-the-resident-worker.md) | Accepted | 2026-09-30 |
| 0077 | [ADR-0077: User-requested model download through the system download tool](ADR-0077-user-requested-model-download.md) | Accepted | 2026-09-30 |
| 0078 | [ADR-0078: The OpenVINO add-on through the Store](ADR-0078-openvino-add-on-and-store.md) | Accepted | 2026-10-01 |

## Superseded

- ADR-0064 (GTK4/WebKitGTK 6 Linux shell) is superseded by ADR-0075 (Tauri 2
  desktop host on every OS, keeping the daemon; owner decision D22,
  2026-09-29).
