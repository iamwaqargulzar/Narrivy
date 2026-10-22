# Audio Pipeline — Current Linux Boundaries

This document records implemented boundaries, not a comparative performance or
release-readiness claim. Native latency, jitter, loss, thermal behavior, and
clean-machine behavior remain unqualified.

**Capture isolation.** PipeWire's actual real-time work belongs to the external
`pw-record` process. Narrivy's parser is a non-real-time worker. The legacy P4
monitor decodes into an overwrite-style pre-roll ring and is suitable for
pre-roll/diagnostic experiments, not lossless ASR transport. Its controlled
microbench and structural source check do not qualify native end-to-end
latency or prove an allocation-free PipeWire callback.

The Recovery 4G path instead decodes 16 kHz mono f32 into contiguous 80-ms
chunks and uses a fixed eight-chunk channel. It performs no inference, file IO,
network IO, or payload logging. Queue saturation is a terminal
`capture_backpressure` error; samples are never silently overwritten or
skipped. The producer observes Stop/Cancel independently of synchronous ASR
work, while the session owner boundedly reaps the recorder and joins the
parser. Recovery 4H composes this transport only for strict resident-ASR
sessions whose loaded model attests native streaming support. Final-only models
retain the established batch transaction. The composition is controlled and
unqualified: no native latency, accuracy, loss, or microphone claim follows.

Recovery 4I adds independent `speech.vad` and `speech.endpoint` contracts on
the orchestration worker. The controlled Linux candidate uses energy
hysteresis and sample-count timing (240 ms minimum contiguous speech, then
560 ms trailing silence). It observes but never trims audio: the triggering
and already-buffered chunks are still fed to ASR. Automatic endpoint, user
Stop, and Cancel retain separate typed meanings. These are controlled defaults,
not native threshold, speech/noise quality, first/last-word preservation, or
latency qualification.

Recovery 4J exposes validated streaming hypotheses through a bounded volatile
observer and protocol-minor-3 event. The daemon coalesces latest partials per
transaction without allowing them to displace lifecycle truth. Only explicitly
subscribed same-UID clients receive them; the desktop rejects stale global,
session, transaction, pipeline, and ASR sequence identity and keeps tentative
text in memory-only accessible presentation. The final transcript remains the
only representation entering correction, journal, or delivery.

Recovery 4L starts transaction-scoped capture before resident ASR admission.
`wait_ready` retains the first real 80-ms chunk; core publishes `Listening`,
opens ASR, then feeds that retained sequence-zero chunk and every queued chunk
through one exact-once continuity path. The fixed eight-chunk queue is unchanged
and saturation remains terminal `capture_backpressure`, so this closes the
previous unbounded capture-start gap without claiming continuous pre-roll or
silently trading correctness for memory. Once producer failure is known, it
preempts even the retained chunk so a partial cannot escape from a lossy stream.
Controlled ordering does not qualify native start-of-speech clipping or latency.

Recovery 4M-A makes resident worker health an admission boundary. Worker/model
startup occurs at daemon initialization or in an explicitly supervised recovery
task, never inside `open_stream` after capture has begun. A dead or uncertain
worker becomes `RestartRequired`; one generation warms outside the owner lock,
and a capability change fails closed. Strict sessions require live `Ready` and
an atomic single-session permit before creating a pending journal entry or
opening capture. Status and pipeline enablement reflect this live lifecycle,
not cached model capability alone. This does not preopen a native stream,
expand the capture queue, or qualify native partial/inference latency.

Recovery 4M-B replaces legacy epoch/string-map timing with an optional
transaction-local monotonic observer at ten fixed orchestration boundaries.
The production daemon installs no observer and writes no timing files; the
legacy protocol timing field remains absent. An explicit ignored diagnostic
can pace a hash-pinned WAV through the real resident worker, deterministic
corrector, Unix journal, and Inbox adapter in private temporary storage. This
is useful for locating controlled backpressure, but the substituted source and
unannotated speech onset cannot qualify microphone, perceived, UI, percentile,
or PERF-005 latency.

**Per-PID journal.** Every `wpctl` duck follows snapshot → mutate → journal append with `owner_pid` and `applied_at_ms`. Crash recovery treats a PID gone (including zombies) as orphaned and restores only dead owners. `verify_restoration_idempotent()` proves second restore is no-op — no double-restore. `AudioPolicyJournal::verify_restoration_idempotent()` proves clearing twice leaves empty.

**Virtual-device exclusion.** `MicDevice::is_virtual()` drops `narrivy_test`/`remap` sources from ranking unless `NARRIVY_ALLOW_VIRTUAL_MIC` is set, preventing test harnesses from polluting user machines. Ranking stays deterministic: default → ≥16 kHz → name → id.

**Jitter observation.** The legacy monitor's sliding-window variance can emit a
content-free `jitter_high` health fact. It is not an adaptive jitter buffer and
does not repair discontinuities.

**Delivery probes.** `DeliveryRouter::probe_all()` has controlled parallelism
tests. Those tests do not establish current-host or clean-machine delivery
latency.

Native VAD/endpointer calibration, continuous pre-roll, device hotplug/fallback,
native partial/repaint latency, hypothesis stability, and native performance
qualification remain separate work.
