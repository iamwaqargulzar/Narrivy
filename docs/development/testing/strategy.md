# Verification Strategy

Narrivy uses evidence layers rather than one giant test suite.

1. **Unit/domain:** state machine, policy, normalization, precedence, migrations.
2. **Contract:** platform/ASR/delivery/context interfaces with deterministic fakes.
3. **Schema/property:** manifests/modes/profiles/privacy and boundary/path/size invariants.
4. **Service integration:** daemon + core + mock/controlled adapters, IPC reconnect/cancel/version behavior.
5. **Real Linux integration:** portal/shortcut, PipeWire lifecycle, text-delivery backends, suspend/hotplug.
6. **ASR/device qualification:** fixed corpus, model/runtime/device health, fallback.
7. **Failure injection:** process/device/storage/cache/backend/race faults.
8. **Desktop E2E:** onboarding through real external target insertion.
9. **Accessibility/manual perceptual:** keyboard/screen reader/scaling/contrast/reduced motion.
10. **Performance:** trigger→commit stage timing, memory/residency regression.
11. **Packaging/release:** clean install, upgrade, uninstall, source-independent E2E.

## Evidence rule

A release-gated requirement has one traceability row and a named `VER-*` identifier. `passed` means the referenced evidence exists. Environment-dependent requirements cannot be closed solely by mocks/CI.

## Determinism

Unit/contract tests do not depend on microphone hardware, user desktop state, Internet, or wall-clock timing. Use injected clocks/randomness/fixtures. Real environment tests are explicitly tagged and record environment metadata.

## Privacy

Fixtures are redistributable synthetic/project-owned data. CI logs never print private user audio/transcripts/context. Test failures that include transcript fixtures must use non-sensitive fixture text only.
