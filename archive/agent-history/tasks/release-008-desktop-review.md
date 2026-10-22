# Release task 008: desktop review regressions

DeepSeek V4.1 Flash at provider reasoning=max implements; Codex independently verifies. Use apply_patch, authoritative rules, no commits/pushes/host changes/secrets or fake evidence. Continue task 004 correctness only, no cosmetic redesign.

Codex corrected the real wire contract: StatusDto.installed_model is a string enum (Rust serialization regression verifies it), not {state}. Shared bridge type and existing Models, pipeline helper/provider and fixtures migrated; the existing bundled-model frontend test passes. Preserve this correction and migrate all new consumers/fixtures (Home was still accessing .state mid-implementation). Do not change the Rust wire format to fit a wrong fixture.

Review and meaningfully regress selection/freshness races: PipelinesProvider.resolveStartId currently captures one pending promise; a newer refresh can begin while it waits. It must await the latest advertisement, reject unresolved/failed/stale data for Start, and never substitute a different ID during an in-flight start. Preserve user's choice if still usable; if removed, surface selection change and require an intentional next start. Readiness refresh key must include relevant actual capabilities, not only unchanged provision revision/model state, since resident warming->ready/restart can change enabled. Status refreshes need generation/ownership protection so an older awaiting-model request cannot overwrite a newer ready response; do not publish after unmount. Bound active-provision polling by one operation deadline and surface exhaustion with retry rather than silently freezing.

Stop and Cancel must remain actionable when Start/listing is waiting, or explain and safely retain a pending cancellation intent; an enabled Cancel button that immediately returns false due to the shared inFlight ref is not working cancellation. Preserve separate authenticated IPC path for cancelling a long request, daemon acceptance/admission truth and existing recovered-text behavior. Do not permit unintended late Start after user cancellation.

Run deterministic promise-controlled UI regressions for overlapping refreshes, stale status response, listing failure, selection disappearance, Start/Cancel race and model provision refresh, plus full frontend tests and production build. Report exact results and remaining native acceptance gaps, then stop.

Codex independently added `src/__tests__/DictationCancellationRace.test.tsx`:
promise-controlled listing keeps Start pending, the hook advertises Cancel,
Cancel is called, then listing resolves. Current implementation rejects Cancel
and issues an unintended late Start. Preserve this behavioral regression and
make it pass; do not weaken the asserted cancellation acceptance or absence of
late Start. This is a known pre-existing release blocker, outside task006's
mode implementation scope.

Codex also independently added `src/__tests__/StatusResponseRace.test.tsx`:
initial status request waits, a newer explicit reload returns ready, then the
old request returns awaiting_model. Current provider regresses to the stale
snapshot. Preserve this deterministic ordering regression and make it pass
using request/lifetime ownership; do not alter the real string enum fixtures.

Final-review regression added to DictationCancellationRace: separate Start
and Cancel IPC connections can be admitted out of order. A first cancellation
can acknowledge while Start is still awaiting admission; when Start later
acknowledges, the user's cancellation intent must still leave that session
inactive and Start must not report successful recording. The controlled
remote-state fixture asserts actual final inactivity, not a particular
implementation or call count. Preserve this case; latching intent only before
pipeline resolution is insufficient once bridge.start has already been sent.
