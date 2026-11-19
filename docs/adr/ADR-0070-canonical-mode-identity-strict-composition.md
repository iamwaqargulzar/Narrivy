# ADR-0070: Canonical built-in mode identities for the strict installed composition

- Status: accepted for recovery; release qualification pending
- Status note (2026-10-30): the `mock-dictation` id is gone. The non-strict
  mock composition now offers only `clean` (its default) and `raw`, and
  rejects `mock-dictation` like any unknown id (F-M13).
- Date: 2026-09-14
- Phase: P11 recovery, release task 006 (real microphone Raw/Clean policy)

## Context

FUN-021 defines Raw and Clean as declarative built-in pipelines, and the P7
work registered exactly two portable built-in definitions through
`builtin_pipeline_defs` (`raw`, `clean`). The mock development lane
(`mock-dictation`) resolved those two ids to its own deterministic correction
stage.

The strict Recovery 2A/2B imported composition evolved separately. Its
pipeline identity was the composition itself:

- `select_pipeline` accepted only `linux-mic-inbox` or
  `linux-mic-wtype-inbox` and rejected `raw`/`clean` with `BadRequest`, so the
  real installed microphone/ASR composition had no real Raw mode at all.
- `begin_session` decided whether to defer `Listening` until capture readiness
  by string-matching those same two lane ids.
- The owned worker closure always constructed
  `DaemonCorrector(CorrectionStrategy::FullCorrect)`, ignoring the admitted
  pipeline's correction strategy. Combined with `begin_session` resolving the
  correction separately, one admission decision produced two independent
  correction resolutions, and they could disagree.
- Policy capability advertising published `pipeline.linux-mic-inbox=...`, and
  release qualification asserted that same lane id in `ListPipelines`.
- The desktop already consumes `ListPipelines`/status as its only mode source
  (release task 004), so its selector could only offer the lane id that strict
  admission accepted.

The result was that the packaged strict composition could not execute Raw, and
any attempt to select it failed before capture. The historical lane ids are
still necessary: they select the owned capture and delivery composition through
`NARRIVY_RECOVERY_MODE`. They are process configuration, not pipeline identity.

## Decision

1. **Canonical mode ids are the pipeline identities of the strict installed
   composition.** `raw` and `clean` are advertised, admitted, journaled,
   evented and selected exactly as those ids. The composition lane
   (`NARRIVY_RECOVERY_MODE=linux-mic-inbox|linux-mic-wtype-inbox`) selects the
   capture plus delivery path and is never a selectable or advertised pipeline
   id. `NARRIVY_DEFAULT_PIPELINE` must name `raw` or `clean` when a strict
   composition is configured; any other value is a hard startup error rather
   than a silent lane switch.
2. **One admitted value for the whole session.** `select_pipeline` resolves one
   `AdmittedPipeline` (canonical id + deterministic correction + lane) before
   any side effect. Strict single-session admission, journal admission, the
   `Listening` deferral, active-session/partial identities, batch orchestration
   and the owned worker all consume that value. The worker composes
   `strict_composition_corrector(&admitted)`, and both the batch
   `RecoverySlice` and the `StreamingRecoverySlice` receive that one
   `CorrectionPort`, so Raw is `CollapseOnly` and Clean is the existing
   deterministic `FullCorrect` on identical capture/ASR/privacy/finalization/
   delivery paths.
3. **Advertisement equals acceptance.** A strict composition advertises exactly
   the two canonical modes, enabled only under the existing static-readiness
   plus live-resident-readiness gate. Unknown ids, the historical lane ids and
   the legacy mock lane id reject with `BadRequest` before journal, capture or
   session side effects; the non-strict mock composition keeps its historical
   `mock-dictation`/`raw`/`clean` development identities and still rejects the
   lane ids.
4. **Lane history maps to Clean, never to an alias.** The strict lane's
   historical behavior was `FullCorrect`, which is the Clean mode. An id-less
   `StartSession` therefore resolves the configured canonical default (`clean`
   when unset). The lane id is not accepted as a pipeline alias, and migration
   is explicit: packaged units and qualification helpers set
   `NARRIVY_DEFAULT_PIPELINE=clean`, and release tooling requires both modes to
   be advertised with the configured default's typed `enabled` token.
5. **Capability advertising follows the same policy.** Strict capability
   strings publish `pipeline.clean=...` and `pipeline.raw=...`; the lane id no
   longer appears as a pipeline token. The lane remains visible through the
   capture/delivery capability strings that describe it.

The portable contracts are unchanged: `narrivy-core` recovery slices already
carry `pipeline_id` and an injected `CorrectionPort`, and `narrivy-pipeline`
already owns the two built-in definitions. No IPC field was added, so no
protocol minor bump is required.

## Relationship to the canonical catalogs

`spec/catalogs/pipelines.json` describes product-level v1.3 pipelines
(`fast-dictation`, `clean-mic`, ...). The daemon's built-in Raw/Clean modes are
the P7 FUN-021 implementations of that requirement and are registered in the
portable `PipelineRegistry` through `builtin_pipeline_defs`. This ADR makes the
strict installed composition execute those existing built-ins; it does not claim
the catalog's `fast-dictation` denoise/normalize composition or any other
catalog pipeline is implemented, and no requirement state changes.

## Alternatives considered

- **Keep the lane id as the pipeline identity and add a separate mode field to
  the DTO.** This needs a protocol change, and the recovery journal and partial
  events would still be unable to name the mode a session actually used.
- **Advertise the lane id plus both modes.** Three entries for two behaviors is
  untruthful to the user and would keep the `ListPipelines` contract ambiguous.
- **Silently alias the lane id to Clean.** This hides a real identity migration
  and keeps an id selectable that no longer names a pipeline; unknown and
  mismatched lanes must reject before capture.
- **Make Raw a UI-only presentation state.** It would not change correction and
  would contradict the requirement that Raw is a deterministic pipeline lane.

## Consequences

- The desktop selector needs no policy code: it renders the daemon's two real
  mode entries and starts the exact selected id (verified by focused frontend
  tests against strict-mode fixtures).
- Strict fail-closed behavior is preserved: single-session permit, delayed
  `Listening` until actual capture readiness, cancellation, bounded cleanup,
  installed-model/live-worker readiness, and Inbox recovery are untouched.
- The recovery journal, active-session status, session events, partial events
  and recovery-entry reads carry one identity (`raw` or `clean`).
- A packaged unit that still sets `NARRIVY_DEFAULT_PIPELINE=linux-mic-inbox`
  fails startup with an explicit configuration error until it is migrated to a
  canonical mode.
- The controlled composition regressions prove exact correction outcomes for
  both modes (including Unicode and multiline input), batch/streaming parity,
  recovery identity and rejection before capture. They are deterministic
  correctness tests over the portable slices, not native microphone, installed
  model, delivery or release qualification; those remain open.
