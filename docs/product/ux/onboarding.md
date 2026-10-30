# First-run Onboarding

Goal: first successful dictation without teaching the user ML terminology.

## In 1.0

The first-run wizard (`apps/narrivy-desktop/src/app/Onboarding.tsx`) has three
steps. The user can skip at every step.

1. Welcome. When the service manages shortcuts, the user can set the
   dictation shortcut here. Otherwise the wizard shows the command to bind in
   the desktop's own keyboard settings, labelled as a suggestion.
2. Microphone check. Runs the service's microphone check and shows pass or
   fail.
3. First dictation. If no speech model is installed, offers the bundled model
   install with its size,
   progress, cancel, retry and the failure reason. Then Start and Stop.

## Roadmap (not in 1.0)

The full flow below is the target. Steps marked roadmap are not built.

1. Welcome + privacy summary. *(1.0 has the welcome, not the privacy summary.)*
2. Microphone permission/test. *(1.0 has the test, not microphone choice.)*
3. Capability scan: desktop/session/shortcut/output/audio/compute. *(Roadmap.)*
4. Recommended processing profile and model download size. *(Roadmap. 1.0
   offers only the bundled model.)*
5. Shortcut binding/test. *(1.0 sets the shortcut; there is no test step.)*
6. Dictation sandbox: user speaks, sees result, retries if needed. *(Roadmap.
   1.0 has Start and Stop only.)*
7. Optional privacy/history choice. *(Roadmap.)*
8. “Narrivy is ready.” *(Roadmap. 1.0 ends with Finish.)*

Advanced setup is deferred.

On Linux, if the preferred shortcut/output path is unavailable, explain the fallback in human terms and offer a fix without asking the user to decode compositor jargon.
