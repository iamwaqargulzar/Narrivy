# Reference Environment Provisioning and Evidence

## Principle

A coding host is not automatically a release test environment. The agent should provision repeatable clean environments where feasible rather than silently marking platform tests skipped.

## Required Linux 1.0 environments

1. Pop!_OS 24.04 + current supported COSMIC Wayland release (the only 1.0
   lane; owner decision 2026-09-28).

Post-release lanes (Ubuntu GNOME first) are listed in the
[roadmap](../../roadmap/README.md).

Exact image/version IDs are recorded in `agent/RESOLVED_VERSIONS.json`/release evidence at test time.

## Provisioning preference

1. Test the coding host if it exactly matches a reference environment and doing so is non-destructive.
2. Use reproducible VM images/snapshots for the other reference environment when hardware virtualization and GUI/audio integration are available.
3. Use disposable clean VMs for package install/upgrade/uninstall tests even if development occurs on the host.
4. Containers are useful for compile/package/static tests but **do not substitute** for real Wayland portal/global-shortcut/PipeWire/text-delivery E2E evidence.

The agent may install/configure a local VM/test harness autonomously when safe. It records image checksums/version, VM settings relevant to audio/display/virtualization, and test scripts.

## Human interaction

Where a desktop intentionally requires interactive user permission (for example microphone/global shortcut approval), automate everything up to the system dialog and document the exact action. If the test host permits scripted preconfiguration without bypassing security semantics, that can be used for repeat runs, but at least one first-run permission UX must be manually/visually qualified.

## Evidence quality

A passing environment report includes:
- OS/image/version/kernel;
- desktop/compositor/session type;
- portal implementation/version;
- PipeWire/WirePlumber versions;
- package artifact/build identity;
- CPU/GPU/NPU/driver/runtime information relevant to claims;
- display scale/monitor topology used;
- microphone/audio device class;
- exact scenario script/checklist and result;
- known deviations/limitations.

A headless unit test or successful package installation cannot be used as evidence for global PTT or external text insertion.
