# Pipelines, Modes, and Profiles UX

## Simple surface

Normal users see outcomes, not graph jargon. The default is **Fast Dictation** and should work immediately after minimal onboarding.

Built-in friendly recipes may be presented as Raw, Clean/Fast Dictation, Smart Local, or other outcome names as they become implemented. A Mode is a friendly recipe over a Pipeline, not a separate execution engine.

## Pipeline picker

A user can choose which saved Pipeline is active and assign independent shortcuts. Advanced users can create/duplicate/edit a Pipeline graph and choose modules, model/provider, compute preference, privacy policy and fallbacks.

## Progressive disclosure

Default editor exposes only meaningful choices: hotkey semantics, microphone, language, cleanup level, privacy/local-cloud preference and Auto hardware selection. “Advanced pipeline” reveals stage details.

## App Profiles

Rule-based automatic Pipeline selection (for example editor → Coding) is post-1.0 unless promoted. The architecture must preserve the ability to show why a rule matched and allow one-click temporary override.

## Import/export

Settings supports full portable export plus selected Pipeline export/import. Secrets are excluded by default. Imported Pipelines show missing module/model/runtime dependencies before download.
