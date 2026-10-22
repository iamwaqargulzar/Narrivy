# Onboarding Contract

## Goal
Reach first successful real dictation quickly while explaining only permissions/actions that are currently necessary.

## Flow

1. Welcome + local-first promise in one short page.
2. Microphone selection/test and permission remediation.
3. Model choice expressed as recommended quality/resource choices; advanced runtime/device details behind disclosure.
4. Download/verify/prepare model with progress, cancel/retry, disk requirement and offline state.
5. Global trigger setup/permission. Explain PTT vs toggle only if capability forces a choice.
6. Guided test: focus a safe Narrivy test field first, then optionally an external target; speak and confirm result.
7. Success screen with trigger reminder and direct entry to Home.

## Rules

Never request broad permissions “for later”. Never force a cloud account for local dictation. If accelerator fails, offer qualified CPU fallback instead of blocking onboarding. Preserve progress across restart where safe.

## Tests

`E2E-ONBOARD-CLEAN`, `E2E-ONBOARD-DENIED-MIC`, `E2E-ONBOARD-MODEL-FAIL`, `E2E-ONBOARD-CPU-FALLBACK`, `A11Y-ONBOARD-KEYBOARD`.
