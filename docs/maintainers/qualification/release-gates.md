# Release Gates

Machine-readable truth lives in `spec/requirements-traceability.csv`, `spec/acceptance/P*.json`, and release evidence.

A Linux 1.0 release is blocked by:
- any required requirement not `passed` with evidence;
- architecture/traceability/action-pin/dependency-policy failure;
- known recoverable-text loss or duplicate/unintended insertion defect;
- persistent audio duck/mute/pause restoration defect;
- content/secret/privacy leakage;
- insecure local daemon control;
- broken onboarding/primary dictation on a reference environment;
- release-critical keyboard/screen-reader/scaling defect;
- missing model/artifact integrity/provenance/licensing information;
- missing migration for changed persistent state;
- broken clean install/upgrade/uninstall;
- missing `.deb` evidence required by `OPS-002`;
- unresolved project license before public distribution.

`python3 scripts/release_gate.py --target linux-1.0` fails closed. Feature count never overrides a release gate.
