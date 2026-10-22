# Phase Acceptance Specifications

Acceptance files are deliberately fail-closed. `implemented: false` means the phase has not yet wired its real commands/artifact requirements; the coding agent changes it only while implementing that phase. Setting `implemented: true` without real checks/evidence is a gate violation. Primary requirements must also be `passed` in traceability with an existing evidence path.

## Current contract fields

- `commands`: deterministic phase checks run after evidence/owner minima validate.
- `required_artifacts`: repository paths that must exist.
- `required_artifact_hashes` *(optional)*: map of required path → SHA-256 when an acceptance artifact is intended to be immutable at gate time.
- `smoke_commands`: required and non-empty for any enabled phase whose `agent/PHASES.json` entry has `working_app_required: true`. These must exercise a real non-mock runnable application path appropriate to the phase.

Owner `minimum_checks` are additional non-relaxable acceptance. They are not satisfied merely because the acceptance file omits them.

## v1.3

Acceptance primary requirements now include first-class Pipeline/resource contracts, trigger-to-pipeline binding, trusted resource downloads, guarded text post-processing, settings/Pipeline portability and split remote transcript/context privacy.

