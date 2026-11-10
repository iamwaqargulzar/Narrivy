# Architecture decision records

Each ADR records one decision: its context, what was decided and the consequences. A later ADR may refine or replace an earlier one; where two disagree, the later one applies. The [engineering rules](../development/engineering-rules.md) and the requirement catalog (`spec/requirements-traceability.csv`) take precedence over any ADR.

A new ADR takes the next free number and is named `ADR-NNNN-short-title.md`.

| ADR | Decision | Status |
|---|---|---|
| 0001 | [Greenfield clean-room implementation](ADR-0001-greenfield-cleanroom.md) | Accepted |
| 0002 | [Rust domain core with Tauri 2 desktop shell](ADR-0002-rust-core-tauri-shell.md) | Accepted |
| 0003 | [Capability-driven platform API](ADR-0003-portable-platform-api.md) | Accepted |
| 0004 | [Long-running core service with portable local IPC](ADR-0004-daemon-ipc.md) | Accepted |
| 0005 | [UI renders capabilities, not OS branches](ADR-0005-capability-driven-ui.md) | Accepted |
| 0006 | [Declarative Mode pipeline DAG](ADR-0006-pipeline-dag.md) | Accepted |
| 0007 | [Local-first with explicit data-boundary policy](ADR-0007-local-first-privacy.md) | Accepted |
| 0008 | [Sandbox portable plugins where feasible](ADR-0008-plugin-sandbox.md) | Proposed |
| 0009 | [Separate crash journal, history and learning stores](ADR-0009-history-journal-separation.md) | Accepted |
| 0010 | [Optimize end-to-end perceived latency](ADR-0010-measure-perceived-latency.md) | Accepted |
| 0011 | [Same-user local IPC is a security boundary](ADR-0011-local-ipc-security.md) | Accepted |
| 0012 | [Daemon Lifecycle](ADR-0012-daemon-lifecycle.md) | Accepted |
| 0013 | [Persistence](ADR-0013-persistence.md) | Accepted |
| 0014 | [Modular internal contracts; no public Rust dylib ABI](ADR-0014-module-extension-boundary.md) | Accepted |
| 0015 | [Lean base install with qualified optional runtime packs](ADR-0015-runtime-packs-and-auto-qualification.md) | Accepted |
| 0016 | [First-class pipelines and trigger bindings](ADR-0016-first-class-pipelines-and-triggers.md) | Accepted |
| 0017 | [Resource registry, artifact classes, and download trust](ADR-0017-resource-registry-and-artifact-trust.md) | Accepted |
| 0018 | [Text post-processing and inference providers](ADR-0018-text-postprocessing-and-inference-providers.md) | Accepted |
| 0019 | [Portable settings import/export and future sync boundary](ADR-0019-portable-settings-and-future-sync.md) | Accepted |
| 0020 | [Evidence trust registry, verification profiles, and artifact binding](ADR-0020-evidence-trust-registry-and-artifact-binding.md) | Accepted |
| 0021 | [Recovery 2A transactional Inbox slice](ADR-0021-recovery-2a-transactional-inbox-slice.md) | Accepted |
| 0022 | [Recovery 2B truthful wtype submission and Inbox recovery](ADR-0022-recovery-2b-truthful-wtype-and-inbox-recovery.md) | Accepted |
| 0023 | [Permission-mediated portal/EIS delivery candidate](ADR-0023-portal-eis-delivery-candidate.md) | Accepted |
| 0024 | [Portal keyboard permission IPC contract](ADR-0024-portal-keyboard-permission-ipc-contract.md) | Accepted |
| 0025 | [Controlled portal/EIS text-submission contract](ADR-0025-controlled-portal-eis-text-submission-contract.md) | Accepted |
| 0026 | [Minimal Reis EI text sender](ADR-0026-minimal-reis-ei-text-sender.md) | Accepted |
| 0027 | [Portal service composition seam remains startup-disabled](ADR-0027-portal-service-composition-seam.md) | Accepted |
| 0028 | [GTK3 Wayland parent export is a narrow desktop adapter](ADR-0028-gtk3-wayland-parent-export-adapter.md) | Accepted |
| 0029 | [Desktop portal-parent coordinator remains an inert state machine](ADR-0029-inert-desktop-portal-parent-coordinator.md) | Accepted |
| 0030 | [Repair portable transport and journal boundaries](ADR-0030-portable-transport-and-journal-boundary-repair.md) | Accepted |
| 0031 | [Linux IPC principal and endpoint-race limits](ADR-0031-linux-ipc-principal-and-endpoint-race-limits.md) | Accepted |
| 0032 | [Separate Stop from Cancel and own Linux capture](ADR-0032-stop-cancel-and-owned-linux-capture.md) | Accepted |
| 0033 | [Process-isolated resident ASR worker](ADR-0033-process-isolated-resident-asr-worker.md) | Accepted |
| 0034 | [Native model streaming-capability attestation](ADR-0034-native-model-streaming-capability-attestation.md) | Superseded by 0035 |
| 0035 | [Bounded resident native-stream protocol](ADR-0035-bounded-resident-native-stream-protocol.md) | Accepted |
| 0036 | [Recovery streaming canary model](ADR-0036-recovery-streaming-canary-model.md) | Accepted |
| 0037 | [Bounded live capture transport](ADR-0037-bounded-live-capture-transport.md) | Accepted |
| 0038 | [Streaming recovery transaction composition](ADR-0038-streaming-recovery-transaction-composition.md) | Accepted |
| 0039 | [Conservative VAD and truthful endpoint origin](ADR-0039-conservative-vad-and-endpoint-origin.md) | Accepted |
| 0040 | [Volatile, identity-bound partial presentation](ADR-0040-volatile-partial-presentation.md) | Accepted |
| 0041 | [Notification-driven daemon event pump](ADR-0041-notification-driven-daemon-event-pump.md) | Accepted |
| 0042 | [Capture-first streaming admission](ADR-0042-capture-first-streaming-admission.md) | Accepted |
| 0043 | [Supervised resident readiness and strict admission](ADR-0043-supervised-resident-readiness-and-strict-admission.md) | Accepted |
| 0044 | [Ephemeral monotonic recovery timing](ADR-0044-ephemeral-monotonic-recovery-timing.md) | Accepted |
| 0045 | [Durable recovery state and optional History separation](ADR-0045-durable-recovery-state-and-history-separation.md) | Accepted |
| 0046 | [Session worker terminalization ownership](ADR-0046-session-worker-terminalization-ownership.md) | Accepted |
| 0047 | [Disable legacy non-strict PipeWire capture](ADR-0047-disable-legacy-non-strict-pipewire-capture.md) | Accepted |
| 0048 | [Trusted Linux system-helper execution](ADR-0048-trusted-linux-system-helper-execution.md) | Accepted |
| 0049 | [Digest-bound resident worker and explicit daemon shutdown](ADR-0049-digest-bound-resident-worker-and-daemon-shutdown.md) | Accepted |
| 0050 | [Immutable resident model descriptor](ADR-0050-immutable-resident-model-descriptor.md) | Accepted |
| 0051 | [Verified Installed-Model Observation](ADR-0051-verified-installed-model-observation.md) | Accepted |
| 0052 | [Installed runtime manifest binding](ADR-0052-installed-runtime-manifest-binding.md) | Accepted |
| 0053 | [Isolated installed-runtime package staging](ADR-0053-isolated-runtime-package-staging.md) | Accepted |
| 0054 | [Disposable Debian candidate composition](ADR-0054-disposable-debian-candidate-composition.md) | Accepted |
| 0055 | [Complete-directory offline model import](ADR-0055-complete-directory-offline-model-import.md) | Accepted |
| 0056 | [Truthful degraded installed-model lifecycle](ADR-0056-truthful-degraded-installed-model-lifecycle.md) | Accepted |
| 0057 | [Cooperative, commit-aware offline model import](ADR-0057-cooperative-commit-aware-offline-model-import.md) | Accepted |
| 0058 | [Bounded immutable resident activation](ADR-0058-bounded-immutable-resident-activation.md) | Accepted |
| 0059 | [Daemon-owned offline model provisioning protocol](ADR-0059-daemon-owned-offline-model-provisioning-protocol.md) | Accepted |
| 0060 | [Daemon-owned offline model provisioning lifecycle](ADR-0060-daemon-owned-offline-model-provisioning-lifecycle.md) | Accepted |
| 0061 | [Daemon-owned IPC command authorization and rate matrix](ADR-0061-daemon-ipc-command-policy.md) | Accepted |
| 0062 | [Quarantine the legacy Debian builder](ADR-0062-quarantine-legacy-debian-builder.md) | Accepted |
| 0063 | [Explicit systemd cgroup ownership for daemon descendants](ADR-0063-systemd-cgroup-helper-containment.md) | Accepted |
| 0064 | [Provisional GTK4/WebKitGTK 6 desktop runtime](ADR-0064-gtk4-webkitgtk6-desktop-runtime.md) | Superseded by 0075 |
| 0065 | [Absolute deadlines for client handshake/request frames](ADR-0065-absolute-client-frame-deadlines.md) | Accepted |
| 0066 | [Production portal permission composition](ADR-0066-production-portal-permission-composition.md) | Accepted |
| 0067 | [Retained installable Debian candidate](ADR-0067-retained-installable-debian-candidate.md) | Accepted |
| 0068 | [Persistent resident worker spawn ownership](ADR-0068-persistent-resident-spawn-ownership.md) | Accepted |
| 0069 | [One bounded daemon shutdown budget](ADR-0069-one-bounded-daemon-shutdown-budget.md) | Accepted |
| 0070 | [Canonical built-in mode identities for the strict installed composition](ADR-0070-canonical-mode-identity-strict-composition.md) | Accepted |
| 0071 | [Stop-controlled bounded dictation capture and typed capture end reason](ADR-0071-stop-controlled-bounded-dictation-capture.md) | Accepted |
| 0072 | [Admission-bound target routed delivery composition with durable Inbox recovery](ADR-0072-admission-bound-target-routed-delivery-composition.md) | Accepted |
| 0073 | [Reconcile committed crash recovery records to the Inbox at startup](ADR-0073-startup-recovery-to-inbox.md) | Accepted |
| 0074 | [Injected Host-Fact and Cache-Protection Contracts](ADR-0074-injected-host-probe-and-cache-file-policy.md) | Accepted |
| 0075 | [Tauri 2 desktop host on every OS, keeping the daemon architecture](ADR-0075-tauri-everywhere-with-daemon.md) | Accepted |
| 0076 | [GPU acceleration in the resident worker, self-tested, with CPU fallback](ADR-0076-gpu-acceleration-in-the-resident-worker.md) | Accepted |
| 0077 | [User-requested model download through the system download tool](ADR-0077-user-requested-model-download.md) | Accepted |
| 0078 | [The OpenVINO add-on through the Store](ADR-0078-openvino-add-on-and-store.md) | Accepted |
| 0079 | [The installed service downloads and chooses its models](ADR-0079-installed-service-downloads-its-models.md) | Accepted |
| 0080 | [The model list comes from the Store repository](ADR-0080-live-model-list-from-the-store.md) | Accepted |
| 0081 | [Voice mode listens and speaks in its own Rust worker](ADR-0081-voice-mode-worker-in-rust.md) | Accepted |
| 0082 | [The Urdu voice runs in a separate GPL program](ADR-0082-urdu-voice-as-a-separate-gpl-program.md) | Accepted |
