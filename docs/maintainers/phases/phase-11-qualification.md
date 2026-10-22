# P11 — Linux 1.0 Qualification & Release

## Outcome
Prove the **packaged** product (exact release artifacts, not development builds) on declared **bounded** reference environments. Introduce no broad feature set; fix blockers and collect final evidence.

## Primary requirements
`TEST-010` plus final revalidation of all Linux-1.0 requirements.

## Required environments (bounded support lanes, Part H)
Per `docs/platforms/bounded-support.md`, exact versions qualified, e.g.:
- Pop!_OS 24.04 / COSMIC Wayland (native) — the only first-release lane.

Owner decision 2026-09-28: Ubuntu 24.04 / GNOME Wayland and other distributions are post-release lanes, qualified after the Pop!_OS release is properly tested; then Windows, then macOS.
Additional lanes are experimental/unverified, not supported. At least one **physical/native** machine is required for each first-class environment for platform-sensitive scenarios (activation, mic, delivery, lifecycle, latency); VMs may supplement for packaging/upgrade/UI but may **not** satisfy native platform qualification.

## Scenario suite (via narrivy-qualify harness)
Clean install/onboarding/first dictation; PTT/toggle capability truth; short/long/Unicode/multiline (including Urdu/Arabic, emoji, CJK)/large text; Raw/Clean; mic/model/device switch; target matrix (GTK/Qt/Firefox/Chromium/Electron/terminal/LibreOffice/XWayland); delivery fallback/Inbox; transactional clipboard preservation; cancellation race; daemon/client restart recovery; suspend/resume/hotplug; audio restoration; Privacy Zone/history; support-report redaction; keyboard/screen-reader/scaling/reduced motion; Hot/Warm/Cold benchmarks per `docs/maintainers/qualification/performance-benchmark-protocol.md`; package install/upgrade/uninstall; offline after assets; corrupt cache/model recovery; security/license/SBOM checks; hardware discovery/fallback chains (qualified NPU→dGPU→iGPU→CPU, Vulkan where applicable, manual preferred/locked override), runtime-pack provisioning and benchmark-cache cleanup correctness.

## Release blockers
Any issue causing recoverable text loss, unintended insertion/action, content/privacy leakage, persistent audio-state damage, insecure local control, frequent crash/hang, broken install/upgrade or unusable primary accessibility/dictation flow on a reference environment. Do not document around a blocker — fix and regress-test it.

## Final evidence/commands
Populate `artifacts/release-evidence/linux-1.0/` with `summary.md`, `qualification.json`, `release-artifacts.json` etc., all validating the respective schemas and bound to release commit/build/package hashes. Run `python3 scripts/phase_gate.py --phase P11` then `python3 scripts/release_gate.py --target linux-1.0`. Both must exit 0. This is the normal autonomous stop condition per `BUILD_TARGET.json` (`auto_advance: true`, `stop_after_target: true`). Support matrix/release notes are derived from P11 evidence (Part H).

Release qualification JSON must include the explicit target-app matrix and performance protocol record required by `spec/schemas/linux-qualification.schema.json`; source-only narrative is insufficient.
