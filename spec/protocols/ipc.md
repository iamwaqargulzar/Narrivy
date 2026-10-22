# Narrivy Local Application Protocol — v1 design contract

P1 chooses the concrete serialization/transport implementation. This document defines semantic requirements that cannot be lost during that choice.

## Envelope fields

- protocol major/minor;
- message kind (`request`, `response`, `event`);
- request ID for request/response correlation;
- optional session/transaction ID;
- optional monotonic event sequence;
- bounded typed payload;
- typed error code/detail category without sensitive user content in generic diagnostic form.

## Handshake

Client advertises supported protocol range, build identity and client role. Daemon rejects incompatible major versions; minor feature negotiation is explicit. Transport peer authorization occurs before privileged protocol use.

The Linux principal is the kernel-authenticated effective UID. Client role,
build identity and PID are diagnostic/routing data and never authorize a
command. One daemon-generation-owned policy classifies every command as
read-only, state-changing, privacy-sensitive or destructive and applies an
exact-command rate budget shared across all connections. Reconnecting or
claiming another role does not reset the budget; an explicit daemon restart
does. Rate rejection is typed `busy`. Exact transaction/operation references
and daemon-owned lifecycle state constrain recovery reads, cancellation,
portal authority and provisioning, but are not process identity. See
ADR-0031 and ADR-0061.

## Commands required by P1/P4

- status/capabilities snapshot;
- begin/start session;
- stop/finalize session;
- cancel session;
- subscribe/unsubscribe to session/application events;
- list/select microphone/model/compute policy/mode/profile;
- safe configuration mutations with expected schema/version;
- history/Inbox metadata/content requests under policy;
- run diagnostic capability check;
- create privacy-safe support report.

## Events

Session events include transaction identity and ordering so clients can reject stale partial/final/status events. Capability/health changes are distinct events from session state.

Protocol minor 3 adds `transcript_partial` with exact session, transaction and
pipeline identity, a strictly increasing ASR sequence, and 1..=16,384 UTF-8
bytes of volatile text. The daemon sends events only after an explicit accepted
`Subscribe`; authentication/handshake alone is not subscription. Every event
envelope has a daemon-wide monotonic `event_seq`, and clients reject missing,
duplicate or reordered values within one transport epoch. Partial buffering is
latest-only/coalesced per transaction and may not displace lifecycle events.
An accepted `Subscribe` response is serialized before the first newly eligible
event. An accepted `Unsubscribe` response is a barrier after which the daemon
must emit no event frame for that connection, including an event selected from
an older connection-registry snapshot.
A connection that sends nothing for 30 s is closed, except a subscribed
connection that is between frames: a subscriber only listens, and closing it
would lose every event published before it reconnects. A subscriber that
stalls mid-frame, stops reading or goes away is still closed.
Tentative text is never a journal, history, correction, delivery, or ordinary
diagnostic-log representation.

Protocol minor 4 separates optional History from active-session recovery.
`GetHistory` and `GetHistoryEntry` refer only to the optional retained History
store and fail closed when it is disabled or not composed. They must never
list or reveal recovery-journal records. `GetRecoveryEntry { txid }` reads one
exact short-lived recovery transaction named by a session event; it has no
listing form and does not imply History retention.

`StatusDto.installed_model` is an additive, optional typed lifecycle field.
Installed-model compositions report `awaiting_model` only when the selected
bundled model is explicitly absent and `ready` only after the daemon owns the
verified installed runtime/model binding. It is absent for older peers and
non-installed compositions. Unknown lifecycle tags are preserved by clients;
capability strings remain diagnostic detail rather than the lifecycle API.

Protocol minor 5 adds asynchronous, daemon-owned offline model provisioning.
`StartOfflineModelProvision { source_path }` accepts only a local source path;
the UTF-8 wire value is 1..=4,096 bytes and contains no control characters.
The command does not accept a model id, expected digest, cache/destination
path, network URL, or deadline: those remain daemon policy. Secure path
resolution, file ownership/type checks, no-follow opening, model identity and
integrity verification are implementation responsibilities beyond this
transport-neutral shape check. `CancelOfflineModelProvision { operation_id }`
addresses exactly one daemon-generated opaque operation id. Provisioning ids
are 1..=64 ASCII bytes containing only letters, digits, `-`, and `_`.

Both commands have distinct request kinds:
`request.start_offline_model_provision` and
`request.cancel_offline_model_provision`. Clients must gate them on protocol
minor 5. Paths and operation ids are sensitive diagnostic fields and must be
redacted from generic debug output.

`StatusDto.model_provisioning` is an additive optional
`ModelProvisionStatusDto`; old status payloads without it remain valid. Start,
cancel, and status polling return the same typed status through the
`offline_model_provision` response payload. The status includes an opaque
operation id, monotonic revision, and lossless wire enums for:

- state: `importing`, `cancel_requested`, `activating`, `ready`, `cancelled`,
  `timed_out`, or `failed`;
- publication: `not_published`, `verified_installed`, or
  `published_unconfirmed`;
- cleanup: `not_required`, `confirmed`, or `failed`;
- optional content-free failure: `source_rejected`, `import_failed`,
  `cleanup_failed`, `published_unconfirmed`, `binding_failed`,
  `activation_failed`, `shutting_down`, or `worker_failed`.

Unknown values in those enums are preserved verbatim for forward
compatibility. Installed-model lifecycle additionally admits `provisioning`,
`published_unconfirmed`, and `activation_failed`; only `ready` means the
daemon owns the verified active runtime/model binding. Protocol minor 5 does
not add a provisioning event. Clients observe asynchronous progress through
status snapshots until an explicitly versioned event contract exists.

Protocol minor 6 adds the optional, content-free typed capture end reason to the
`GetRecoveryEntry` payload. `capture_end` is one closed wire tag:

- `finalization_requested`: the user requested Stop; the buffered tail is kept;
- `endpoint_detected`: independent endpointing ended the utterance;
- `safety_limit`: the documented bounded capture maximum was reached and the
  full expected frame count arrived from a recorder that exited successfully or
  with its exact budget-exit code;
- `source_ended`: the source ended before the bound.

The field is additive and optional: older peers keep parsing the payload and
simply do not render how capture ended, and an unwritten reason is absent rather
than guessed. `safety_limit` is an explicit termination, not a silent clip, and
does not mean the transaction failed: any transcript the transaction produced
remains exactly as delivered. The related status capabilities
`capture.window_seconds=…` (the effective bounded window) and
`capture.max_seconds=…` (the documented production maximum) are diagnostic
reporting of the daemon/platform capture policy; clients render them and never
define or extend an OS-side capture window themselves.

Daemon bus-to-transport dispatch is notification-driven rather than scheduled
by a fixed polling interval. Queue admission and terminal close share one
linearization boundary: the owned pump may drain events accepted before close,
and all later publications are rejected. Shutdown disconnects IPC clients
before closing the bus; reconnecting clients obtain a new state snapshot.

Protocol minor 7 adds the additive atomic activation request
`ToggleSession { pipeline_id?, activation }`, kind
`request.toggle_session`. It keeps `StartSession`/`StopSession` semantics
unchanged: a client that does not know minor 7 keeps using them, and an older
daemon answers the unknown kind with a typed rejection rather than a decode
failure.

- `pipeline_id` is the same optional selectable identity as `StartSession`
  (`raw` | `clean`) and is resolved by the daemon's one fail-closed selection
  rule.
- `activation` is a closed, typed activation source — `installed_cli_command`,
  `desktop_client`, or `portal_global_shortcuts` — never an arbitrary policy
  string. Unknown values are preserved verbatim by lossless decoding for
  forward compatibility, but execution fails closed: a request carrying an
  unknown source is a typed `rejected` payload (`bad_request`) with no capture
  side effect, and the raw tag is never echoed into diagnostics.

The daemon owns the decision, atomically, under the existing admission and
single-session rules: with no active session it performs exactly the strict
readiness/admission/start path `StartSession` performs, including the single
strict permit; with a recording session it requests graceful finalization once
and never cancellation; while capture is already finalizing, transcribing,
delivering, or being cleaned up it opens no second session. Typed outcome
`toggle` payload values are:

- `started`: one new session was admitted (`session_id`, `transaction_id`,
  `pipeline_id` echo it);
- `finalization_requested`: the active session's buffered tail is being
  finalized gracefully;
- `already_stopping`: no second session was opened and the existing session
  was already stopping.

Rejection stays typed: a busy/unavailable daemon, shutdown admission closure,
an unsupported/malformed pipeline, or an unknown activation source is a
`rejected` payload with no capture side effect. The supplied pipeline and the
known activation source are validated before the toggle decision and before any
session/control mutation, in every state (idle, recording, or already stopping).
Pipeline validation uses the same pure selection rule as `StartSession` but not
start-only readiness, so a valid toggle still stops an active recording when
models or capture helpers are unavailable. The request carries no transcript,
audio, or target content, and the installed `narrivy-cli toggle` client performs
exactly one `ToggleSession` round trip — never status-then-start/stop.

The same minor advertises three separate content-free capability facts:
`activation.atomic-command` (this daemon accepts the atomic toggle),
`activation.portal-global-shortcuts` (the direct portal interface probe), and
`activation.portal-listener` (a real owned portal listener; unavailable until
one is composed). These are distinct facts; availability of the interface or
command never implies a live listener.

Protocol minor 9 adds an optional `recognition` object to `StartSession` and
`ToggleSession`. Clients must omit non-default recognition settings when the
daemon reports an older minor because an older additive decoder could ignore
the field. The object keeps these choices separate:

- spoken language: engine default, explicit automatic detection, or one
  explicit language tag;
- optional written/output language and script;
- optional translation target language;
- optional locale;
- optional context/bias hints.

Language, script, and locale tags are 1..=32 bytes and consist of nonempty
ASCII alphanumeric subtags separated by `-` or `_`. Context bias contains
1..=8 hints; each hint is 1..=128 UTF-8 bytes and contains no NUL. Both
serialization and deserialization enforce these bounds. Context hint values are
sensitive and must not appear in generic debug output or validation errors.
The daemon converts the wire object into the portable ASR request once at
session admission and passes that same request into batch or streaming ASR;
the selected engine rejects any combination it cannot attest and apply.

Protocol minor 10 adds `DeleteHistoryEntry { txid }`, which removes one
optional-History record on explicit user request and never touches the
recovery journal.

Protocol minor 11 adds three commands for keyboard shortcuts the daemon
manages in the desktop's own shortcut settings:

- `ListShortcuts` returns `support` (`managed`, or `manual:<reason>` when the
  user must bind the command themselves), the `manual_command` to bind by
  hand, and the bindings the daemon created. Shortcuts that belong to anything
  else are never listed.
- `SetShortcut { binding, replace }` binds a key combination to a pipeline,
  optionally with a spoken and a written language. The reply's `outcome` is
  `created`, `replaced`, `conflict`, `invalid` or `unsupported`. A
  combination already in use is a `conflict` carrying `conflict_kind`
  (`shortcut` or `desktop`) and a content-free `conflict_name`; nothing is
  changed unless the client repeats the request with `replace` set, which
  requires the user's explicit confirmation.
- `RemoveShortcut { modifiers, key }` removes one shortcut the daemon created
  and puts back the custom shortcut it replaced, if any.

A combination has at most 4 modifiers (`Super`, `Ctrl`, `Alt`, `Shift`) and
one key name of at most 16 bytes from the daemon's allowlist. An empty `key`
means the modifiers alone, which the desktop runs when they are released
without another key. It is accepted as:
- modifiers alone;
- a key that types nothing, used on its own: `F1`–`F24`, `Pause`,
  `Insert`, `Scroll_Lock`, `Print` or `Menu`;
- any allowlisted key with a modifier other than Shift.

A key that types text is refused on its own or with only Shift. The command a shortcut runs is
built only from the daemon's CLI path, a canonical pipeline id and validated
language tags. Status reports `activation.shortcuts=managed` or
`activation.shortcuts=manual:<reason>`.

Protocol minor 12 adds an optional `transcribe_us` to each `GetHistory` row:
how long transcription took, from the end of capture to the raw text. It is
content-free and absent when it was not measured.

Protocol minor 13 adds `ListModels`. The reply lists every model in the
daemon's catalog with its family, variant, quantization, size, licence,
engine, languages, whether it streams, the device classes it may be
qualified on (eligibility, not a claim about this computer), its pinned source repository and
revision, and whether that exact variant is qualified. `state` is the
daemon's observation on this computer: `active` (loaded now), `installed`
(present, not loaded), `available` (published, not present) or `needs_engine`
(the engine it needs is not part of this daemon). A loaded model that is not
in the catalog is listed with the id `unlisted`.

Protocol minor 14 adds `SelectModel { id }`: load a downloaded model in
place of the one in use. One model serves every pipeline. The reply
`Accepted` means loading has started; it is refused with `busy` while a
dictation or another load is running, and with `bad_request` when the id is
unknown, not downloaded, needs an engine that is not installed, has no
pinned hash, or this installation cannot switch models. The new model is
loaded (and its bytes verified against the pinned SHA-256) before the old
one is released, so a failed load leaves the old model in use. While
loading, `ListModels` reports the model as `loading`; status reports
`model.selection=idle`, `loading` or `failed:<code>`. The chosen id is
remembered and loaded at the next start.

Protocol minor 15 gives each pipeline its own model and device.
`SetPipelineEngine { pipeline_id, model_id?, compute }` stores what that
pipeline recognises with: `model_id` absent means "whatever model is
loaded", and `compute` is `auto` (an accelerator that passes the
self-test, else the CPU), `gpu` (a GPU or the load fails), `npu` (an NPU or
the load fails; protocol minor 17) or `cpu`. Each `ListPipelines` row
carries the pipeline's `model_id` (when it chose one) and `compute`. One
model is loaded at a time. The daemon loads a pipeline's choice at once when
nothing is dictating; otherwise, and whenever another pipeline's choice is
loaded, it loads it when that pipeline starts, and only when the model or
the device actually differs from what is loaded. While such a start-time
load runs, the daemon publishes the `capability_changed` event for
`engine.loading` (`available: true`, then `false`), so a client can say a
model is loading before listening begins. If that load fails, the previous
model stays loaded and the dictation proceeds with it. A client should allow
`StartSession` and `ToggleSession` up to 100 seconds for this.

Protocol minor 16 adds downloading (ADR-0077). `DownloadModel { id }` starts
downloading a catalog model published in the Narrivy organisation; it is the
daemon's only network use and happens only on this request. `Accepted` means
the download has started; it is refused with `busy` while another download
runs and with `bad_request` when the model is unknown, already downloaded, not
published for download, or this installation cannot download models.
`CancelModelDownload { id }` stops it and keeps what arrived, so the next
`DownloadModel` resumes. `ListModels` rows gain `downloadable` and, while
`state` is `downloading` (or when an unfinished download can be resumed),
`downloaded_bytes`. Status reports `model.download=idle`, `downloading` or
`failed:<code>` with a content-free reason (`network_unreachable`, `stalled`,
`http_error`, `tls_failed`, `redirect_refused`, `integrity_mismatch`,
`too_large`, `disk_write_failed`, `downloader_unavailable`,
`models_directory_unsafe`, `download_failed`).

Status also reports, without a protocol change, the device a ready speech
worker attested its model runs on: `asr.transcribe-cpp.device=cpu`, `igpu`
(integrated GPU) or `dgpu` (discrete GPU). The token is absent while no
worker is ready. A client shows this device as where recognition runs; the
compute planner's `compute.device*` tokens describe its separate plan
(ADR-0076).

Protocol minor 17 adds the Store (ADR-0078). `ListStore` returns
`StoreItemDto` rows: `id`, `kind` (`runtime`, `module`, or `extension` for
what brings Narrivy into other apps or adds a whole area (D33); models are
listed only by `ListModels`), `category` (a runtime's family, or a
module's job such as `refine` or `denoise`),
`name`, `size_bytes`, `license`, `devices`, `state` (`available`,
`installing`, `installed`, `in_use`, `failed`), `installed_bytes` while
installing, `failure` (a reason code) when failed, `fit` (`fits`,
`needs_runtime`, `does_not_fit`, `unknown`) with a content-free
`fit_reason` (`npu_found`, `cpu_only`, `needs_runtime`,
`platform_unsupported`, `built_in`, `nvidia_found`, `no_nvidia`,
`planned`), `source` (`published`, `local` for a developer source,
`unavailable`, `built_in` for what ships with Narrivy, such as Vulkan, and
`planned` for what is coming but cannot be installed yet) and `requires`
(the runtime pack an item needs). The daemon decides every value. An item is installed with `DownloadModel { id }`,
which from this minor also takes a runtime pack's id and installs multi-file
packs: the pinned manifest first, then every file it lists, each checked by
size and SHA-256 into a private staging directory renamed into place only
when complete; files already verified are kept on retry. `CancelModelDownload`
stops it. `RemoveStoreItem { id }` removes an installed item, including an
OpenVINO model pack (the Models screen removes those); it is refused
with `busy` while the item is in use. `SetPipelineEngine` accepts
`compute: npu` (strictly the NPU) for a model that can run there, and `auto`
then tries the NPU first for such a model. The device token gains `npu`.
When the OpenVINO worker is configured, status reports
`runtime.openvino=installed`, `missing` or `invalid:<code>`.

Protocol minor 18 adds the delivery result to the terminal
`session_state_changed` event with `state` `committed`: an optional,
content-free `delivery` field, one of `inserted` (insertion verified),
`pasted` (pasted through the clipboard, unverified), `typed` (typed through
the virtual keyboard, unverified), `copied` (on the clipboard only) or
`inbox` (not inserted; kept in the Inbox). The daemon derives it from the
journaled delivery attempts. Older clients ignore it; a client that does not
recognise a value treats the event as before.

`TranscriptPartial` events numbered from `REFINE_PREVIEW_SEQ_BASE` (2^40)
carry the text being cleaned up while the session is `transforming`: first
as recognised, then as cleaned when the clean-up model's reply passed the
faithfulness check (D31). Clients show them in place of each other; a
recognition partial never carries such a number.

Protocol minor 19 adds the AI assistant extension (D33). `AssistantSend
{ turn_id, messages }` (1 to 64 `{role, content}` messages, `role` one of
`system`, `user`, `assistant`, at most 32 KiB each and 128 KiB together;
`turn_id` 1 to 64 of `[A-Za-z0-9_-]`) is accepted at once; the reply streams
as `assistant_delta { turn_id, text }` events and ends with one
`assistant_done { turn_id, outcome, text }`, `outcome` being `ok`,
`cancelled` or `failed:<code>` (for example `failed:assistant_not_configured`).
`AssistantCancel { turn_id }` stops a running reply. While the model loads,
`capability_changed` reports `assistant.loading`. The daemon keeps no
conversation and never logs its text; `AssistantSend` is rate-limited as
privacy-sensitive.

Protocol minor 20 adds `ListLanguageModels` (the language-model packs on
this computer: name, publisher, size, licence and notice, roles, languages,
which roles each is recommended for and in use for, and the clean-up model's
state) and `SelectLanguageModel { role, id }` (`role` `clean_up` or
`assistant`; no `id` selects none). The choice takes effect at once.

Protocol minor 21 adds `audio_level { session_id, level }` while a session
listens, about 20 times a second: `level` 0 to 1000 on a perceptual scale
(-60 dBFS and below is 0, -12 dBFS and above is 1000). Content-free; the
overlay's waves follow it.

Protocol minor 22 adds `AssistantUnload`, which frees the assistant model's
memory once any reply in progress ends (the next question loads it again).
`capability_changed` reports `assistant.loaded` when it loads or unloads,
and `ListLanguageModels` reports the assistant model's `assistant_state`
(`loaded:<device>` or `unloaded`).

Protocol minor 23 adds `CopyHistoryEntry { txid }`, which puts one optional
History entry's text (delivered, else corrected, else raw) on the clipboard,
so a dictation whose paste was missed can be pasted by hand. Only the id
crosses the socket; the daemon reads the text and serves the clipboard. It is
privacy-sensitive and rejected while History is not composed.

Protocol minor 24 serves the browser writing assistant (D26) through
Narrivy's native-messaging host:

- `CheckWriting { text, dialect?, rules?, words? }` returns
  `WritingFindings`: rule-checker findings with UTF-16 `start`/`end`, a
  category, the rule, a message and fixes (`replace`, `insert_after`,
  `remove`). Text is at most 64 KiB, and it is never kept or logged.
- `ListWritingRules` returns `WritingRules` for the settings page.
- `WritingAction { turn_id, action, text, instruction? }` edits text with the
  assistant model. It is accepted at once, and the reply streams as
  `AssistantDelta`. `AssistantDone` gains an optional `warning`:
  `not_faithful` when a Fix changed more than a fix should, and
  `lost_details` when a number, link or address of the original is missing.

The daemon does not trust handshake roles, so the host
(`apps/narrivy-browser-host`) is the boundary. It reaches only `GetStatus`,
`ListLanguageModels`, `Subscribe` (forwarding events of its own turns only),
`CheckWriting`, `ListWritingRules`, `WritingAction` and `AssistantCancel`.

Protocol minor 25 records and transcribes meetings (F22):

- `MeetingStart { title? }` returns `Meeting`. It records the microphone (the
  room) and the computer's sound (a call) into `meetings/<id>/` under the
  state directory.
- `MeetingStop` stops the recording, and transcribing starts at once.
- `ListMeetings` returns `Meetings`, newest first.
- `GetMeeting { id }` returns `MeetingDetail`: the meeting, its turns
  (speaker label, source, times, text, language), the speaker names, the
  folder and the installed models.
- `ProcessMeeting { id, model?, room_speakers?, call_speakers? }` transcribes
  a meeting again, with `turbo` or `large-v3`.
- `RenameMeetingSpeaker { id, speaker, name }` names a speaker; an empty name
  clears it.
- `DeleteMeeting { id }` removes the recording and the transcript.
- The `MeetingChanged { id, state, progress, stage? }` event reports
  recording, processing (progress 0..1000), ready, `failed:<code>` and
  deleted. It is content-free: it carries no title or text.

The worker that tells voices apart and transcribes is configured by
`NARRIVY_MEETING_PYTHON` and `NARRIVY_MEETING_SCRIPT` in development.

Protocol minor 26 adds meeting notes:

- `SummarizeMeeting { id, turn_id, kind, instruction? }` writes notes with
  the assistant model:
  - `notes`: a summary, the discussion, decisions, action items and open
    questions;
  - `email`: a follow-up email;
  - `custom`: an answer to `instruction`.
- A transcript longer than about 1,500 tokens is first condensed part by
  part, quietly, so a model that holds 4,096 tokens can still read an hour.
  The final notes stream as `AssistantDelta` / `AssistantDone` under
  `turn_id`, and are saved as `notes-<kind>.md`.
- `MeetingDetail` gains `notes` (by kind) and `recording`: whether
  `recording.wav`, everyone mixed into one file as a backup, is kept.
- While a meeting records, a live worker writes each turn to `live.jsonl`;
  `GetMeeting` returns those turns until the final transcript exists, and
  each new turn is announced as a `MeetingChanged` (state `recording`, or
  `processing` with stage `finish` while the last words are written).

Protocol minor 27 adds the audio options:

- `GetAudioOptions` replies `AudioOptions { echo_cancel, echo_cancel_active }`.
- `SetAudioOptions { echo_cancel }` turns removing the computer's own sound
  (what the default output plays) from the microphone on or off, and replies
  with the options. With it on and working, dictation and the meeting
  microphone record PipeWire's echo-cancelled source; otherwise the default
  microphone. It is on unless turned off.

Protocol minor 28 adds the voice filter:

- Every dictation's audio passes a voice filter before the speech model: with
  no speech nothing is recognised, silence is cut, and once the user's voice
  is trained ("Only my voice") other voices are left out. When the filter is
  unavailable or slow (5 s bound), the audio goes to the model unchanged.
- `TrainVoice` records about 20 seconds of the user reading and saves a voice
  fingerprint (not audio); `ForgetVoice` removes it. Both reply
  `AudioOptions`.
- `SetAudioOptions` fields are optional: `echo_cancel`, `only_my_voice`.
- `AudioOptions` gains `voice_filter_available`, `voice_trained`,
  `only_my_voice` and `voice_training` (`idle`, `recording`,
  `failed:<code>`).

Protocol minor 29 adds noise suppression levels: `SetAudioOptions` takes
`noise_suppression` (`off`, `standard`, `strong`) and `AudioOptions` reports
it. The level is applied by the same audio processing that removes the
computer's sound, which restarts with it.

Protocol minor 30 adds the user's vocabulary: `GetVocabulary` and
`SetVocabulary { vocabulary: { words, replacements: [{ from, to }] } }`, both
replying `Vocabulary` (as saved: tidied, bounded to 200 words and 100
replacements of at most 64 characters). Every dictation sends the words to a
model that attests context bias (after any hints the client sent), corrects
near misses of them after recognition, and applies replacements last.
Debug output carries counts only.

Protocol minor 31 adds choosing the microphone: `AudioOptions` lists
`microphones` (`{ name, description }`: sources that are not an output's
monitor), the chosen `microphone` (`""` for the system default) and
`microphone_missing` (chosen but unplugged: the default is used meanwhile,
and the choice kept). `SetAudioOptions { microphone }` chooses one. Dictation,
meetings and voice training record it, through the echo canceller when that
is on.

Protocol minor 32 adds press and hold (opt-in): `GetHoldKey` and
`SetHoldKey { key }` (a Linux key code from `keys`, or `null` for off) reply
`HoldKey { key, state, keys }`; `state` is `off`, `starting`, `reading`,
`denied` (the user is not in the `input` group) or `unavailable`. Pressing
the key sends this daemon `StartSession` (activation `hold`) through its own
socket and letting go sends `StopSession`, so press and hold takes the same
admission path as every client. A `SetShortcut` binding whose `pipeline_id`
is `cancel` binds `narrivy-cli cancel`.

Protocol minor 33 adds start and stop sounds: `SetAudioOptions { sounds }`
and `AudioOptions.sounds` (off unless turned on). The daemon plays a short
rising tone when a session starts listening and a falling one when listening
ends.

Protocol minor 34 adds History settings: `SetHistoryRetention { retention }`
(`off`, `7d`, `30d`, `forever`) applies at once, removing what a shorter
policy no longer keeps (`off` keeps existing entries and adds none), and is
saved for the next start unless `NARRIVY_HISTORY_RETENTION` is set.
`ClearHistory` removes every entry. Both reply `Accepted`; the
`history.retention` capability reports the policy.

Protocol minor 35 adds text options: `GetTextOptions` and
`SetTextOptions { numbers_as_digits }` reply `TextOptions`. With numbers as
digits on (the default), the Clean pipeline's final text writes number
phrases as digits ("twenty five" as 25, "five percent" as 5%); zero to nine
and ambiguous phrases stay words. Raw is never changed.

Protocol minor 36 adds per-pipeline Voice settings: `SetPipelineVoice
{ pipeline_id, echo_cancel, only_my_voice }` (each `null` to follow the
default) replies `AudioOptions`, whose `pipelines` lists the pipelines with
their own settings. A dictation records and filters with its pipeline's
settings; meetings and voice training use the defaults.

Protocol minor 37 adds `ImportMeeting { path, title? }`: an absolute path to
an audio or video file (wav, mp3, m4a, aac, ogg, opus, flac, webm, mp4, mkv,
mov, avi, wma; at most 8 GiB). The file is only read: FFmpeg (or GStreamer)
converts it to 16 kHz mono in the meeting's folder within 30 minutes, then it
is transcribed like a recorded meeting. Replies `Meeting` (stage `import`).
Debug output leaves the path out.

Protocol minor 38 adds the delivery method: `SetTextOptions { delivery_method }`
(`paste` or `type`) and `TextOptions.delivery_method`. Paste puts the text on
the clipboard through the Wayland data-control protocol, presses Shift+Insert
and puts the user's clipboard back; type uses the virtual keyboard. Until the
user chooses, `NARRIVY_CLIPBOARD_AUTHORIZED` decides. Undeliverable text is
kept in the Inbox either way.

Protocol minor 39 adds meeting options: `GetMeetingOptions` and
`SetMeetingOptions { auto_notes }` reply `MeetingOptions`. With `auto_notes`
on, a meeting's detailed notes are written as soon as its transcript is
ready (under turn id `auto-<meeting id>`). Meetings also keep
`transcript.srt` and `transcript.vtt` beside `transcript.md`.

Protocol minor 40 adds `cleanup_idle_minutes` to `SetTextOptions` and
`TextOptions`: the clean-up model is unloaded after that many minutes
without use (0 never; at most a day) and loaded again by the next
dictation, which meanwhile gets the deterministic clean-up.

Protocol minor 41 adds `voice_strictness` to `SetAudioOptions` and
`AudioOptions`: how alike speech must sound to the trained voice to be kept
(`relaxed`, `normal`, `strict`).

Protocol minor 42 adds `spoken_breaks` to `SetTextOptions` and `TextOptions`
(off by default): on the Clean pipeline, "new line" and "new paragraph"
become a line break and a blank line, and the next word is capitalised.

Protocol minor 43 adds `microphone` and `speaker` to `SetMeetingOptions`
(a device name, or `""` for the default) and to `MeetingOptions`, and
`call_audio` and `room_audio` (whether the output's sound and the
microphone are recorded at all; not both off), with the
devices to choose from (`microphones`, `speakers`) and the ones recording
now, or that a meeting started now would record (`microphone_in_use`,
`speaker_in_use`).

Protocol minor 44 is the assistant extension (D34). `AssistantLibrary`
(`op`, `kind`, `id`, `value`) reads and changes what the daemon keeps for
the assistant: chats, memory, personas, projects, skills, routines and
pictures (`value` is JSON; a picture is a base64 PNG); it replies with
`AssistantLibrary` JSON, and `run` `routine` runs a routine now.
`AssistantSend` gains `chat_id` (the question goes into that kept chat: the
daemon adds its history, persona, project, memory and skills, and saves the
reply), `speak` (a short spoken reply, read aloud in voice mode) and
`pictures` (picture ids for the question). `VoiceMode` (`action`: `start`,
`stop`, `settings` with `VoiceSettings`, `ptt` with `down`) runs voice
mode; what it hears and does arrives as `Voice` events (`kind`: `state`,
`partial`, `heard`, `interrupt`, `repeat`, `timing`, `error`), and
`AssistantLibraryChanged` says a routine or a reply changed a chat with no
client asking. Heard words travel only in `Voice` events, like assistant
replies; nothing is logged.

Protocol minor 45 lets meetings keep what is shown on screen (D34):
`screen` in `SetMeetingOptions` and `MeetingOptions` (with
`screen_available`: the helper and GStreamer's PipeWire plugin are
installed), `slides` in `MeetingDetail` (`file`, `at_ms`, `description`)
with `screen_error` when the screen could not be kept, and
`GetMeetingSlide` (`id`, `file`) replying `MeetingSlide` with a small PNG
as base64. The user picks the screen or window in the screen-sharing
portal when the meeting starts; pictures stay in the meeting's folder.

Protocol minor 46 lets the assistant use tools (D34). MCP servers are kept
in the assistant library (`kind` `tool`: `name`, `command`, `args`) and
switched on per chat (`tools` in the chat). A typed question in such a
chat runs in rounds: each tool call is announced as an `AssistantTool`
event (`state` `ask`) and runs only after `AssistantToolDecision`
(`call_id`, `allow`, `always` for the rest of the chat); the event follows
it through `running` to `done`, `failed` or `refused` with a short
`summary`. The reply still ends with one `AssistantDone`.

Protocol minor 47 adds the assistant's own tools and watching with the
user (D35). A chat's `tools` may name `web` (search and open public pages)
and `computer` (folders, files, commands, opening things and a look at the
screen); they run in the same rounds as MCP tools. `AssistantTool` carries
an optional `detail`, a JSON string with what the activity panel shows:
`{"kind":"ask","always":false}` when the call cannot be allowed for the
rest of the chat (writing a file, running a command, opening something),
and for finished calls the search results, the page read, the files
listed or the command's output. `MeetingStart` takes `watch` (keep the
screen often, for a video or meeting the user wants to ask about later),
returned on `MeetingDto`. `MeetingDetailDto` lists `screen_sources`
(`source`, `kind`, `width`, `height`), one per screen or window the user
picked, and `GetMeetingSlide` also returns `live-<source>.png`, a small
picture of each source refreshed every few seconds while recording. The
assistant model is loaded and kept through `AssistantLibrary` `kind`
`model` (`get`, `put` `{"autoUnload":bool}`, `run` loads it); web
research settings use `kind` `web` (`provider`, `searxng`, and a Brave
key kept in the desktop keyring, never in the file).

Protocol minor 48 makes going over a meeting pausable and stoppable (D35).
`MeetingControl` (`id`, `action`: `pause`, `resume` or `stop`) pauses the
transcription worker, describing the screen's pictures and writing notes,
resumes them, or stops them; what is done is kept. `stop` while the meeting
is still recording ends the recording without going over it (the meeting
ends `failed:meeting_stopped`, and can be transcribed again later).
`MeetingDto` carries `paused`, and `MeetingChanged` reports the stage
`paused` while it is. A chat may be `pinned` (`AssistantLibrary`, kind
`chat`), and the chat list returns it.

Protocol minor 49 splits "Watch with me" into a live screen share and a
screen recording. `MeetingStart` takes `share` (with `watch`: while it
records, the chat sharing it sees it; once it stops it is kept and gone over
like a recording, a session to give to a chat), and `microphone` and `computer_sound` for this one
start, leaving the meeting options as they are; with `watch` both may be
off (only the screen is kept, and the recording is ready without a
transcript). Without `watch` a `share` is sound only: the chat reads its
live transcript but sees no screen. `MeetingDto` carries `share`. A chat's `live` names the share
it sees: while that share records, each question in the chat (typed or
spoken) carries a copy of what each shared screen shows now, and the
instructions carry what has been said so far. While anything else is
recorded (a meeting or a screen recording), every chat's instructions carry
its live transcript, without pictures. A chat's `fullAccess` lets the
computer tools run without asking; looking at the screen never asks (the
screen-sharing dialog is the user's choice), and during the chat's own share
it is a copy of the shared screen.

Protocol minor 50 lets the screen and the sound of the meeting being
recorded be switched on and off as it goes (`MeetingSources`: `id`,
optional `screen` and `sound`; answers with the meeting, whose `screen_on`
and `sound_on` say what is kept now). Switching the screen back on reuses
the portal's choice from earlier in the same recording (a restore token
held in memory only). While the sound is off the recorders are paused; the
span is kept on the meeting as a gap, and transcript times are moved by the
gaps so they stay on the meeting's clock, in step with the screen.

Protocol minor 51 switches the microphone apart from the computer's sound
(`MeetingSources` gains optional `microphone`; from this minor `sound` is the
computer's sound only, and the meeting's `mic_on` says whether the
microphone is kept now). Each stream has its own recorder and its own gaps
(a gap names its stream, `room` or `call`; a gap without one, from minor 50,
applies to both), so switching one leaves the other's times alone. A stream
switched on for the first time mid-recording starts then, the time before it
a gap. A stream switched off ends its recorder, so the desktop no longer
shows the sound as recorded; switched back on, a new recorder adds to the
same file.

Protocol minor 52 keeps a stopped recording as it was heard
(`SetMeetingOptions` and `MeetingOptions` gain `process_on_stop`, off by
default; the meeting gains `raw`). With it off, stopping finishes the live
transcript and the meeting is `ready` with `raw` set: its transcript is the
live one and its screens are not described. `ProcessMeeting` goes over it
(transcribing again, describing the screens) and clears `raw`. Older
services always go over a recording when it stops.

Protocol minor 53 shows the words being said now while a meeting is
recorded: the live worker cuts speech into pieces of at most a few seconds
and, while a piece is still going on, transcribes it again every second or
two. `GetMeeting` on a recording meeting then ends its turns with one whose
`partial` is set (no speaker yet); it is replaced by the final turn when the
piece ends. Partial turns are never saved, exported or given to the
assistant.

## Bounds

Concrete implementation must define maximum message size, maximum collection/string lengths, request timeout/default deadline, event-buffer/backpressure behavior, concurrent request limit, and malformed-message policy. Tests cover all bounds.

The Unix transport bounds an individual pushed-event write to 50 ms. A stalled
subscriber is disconnected; it cannot hold the daemon pump for the normal
30-second connection timeout.

## Compatibility

Do not serialize internal Rust enum layout blindly and call it stable. Wire enums use stable identifiers with unknown-value handling where forward compatibility is needed. Breaking major protocol change requires migration/compatibility path for packaged daemon/client upgrade behavior.
