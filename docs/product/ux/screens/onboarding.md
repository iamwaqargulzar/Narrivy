# Onboarding Contract

## Goal
Reach first successful real dictation quickly while explaining only permissions/actions that are currently necessary.

## In 1.0

Only part of this contract is built. See `../onboarding.md` for the three
1.0 steps. Items marked *(roadmap)* below are not in 1.0.

## Flow

1. Welcome + local-first promise in one short page.
2. Microphone selection/test and permission remediation. *(1.0: the test only; selection and remediation are roadmap.)*
3. Model choice expressed as recommended quality/resource choices; advanced runtime/device details behind disclosure. *(Roadmap. 1.0 offers only the bundled model.)*
4. Download/verify/prepare model with progress, cancel/retry, disk requirement and offline state. *(1.0: the bundled model install with its size, progress, cancel and retry; offline state is roadmap.)*
5. Global trigger setup/permission. Explain PTT vs toggle only if capability forces a choice. *(1.0: set the shortcut, or show the command to bind.)*
6. Guided test: focus a safe Narrivy test field first, then optionally an external target; speak and confirm result. *(Roadmap. 1.0 has Start and Stop only.)*
7. Success screen with trigger reminder and direct entry to Home. *(Roadmap. 1.0 ends with Finish.)*

## Rules

Never request broad permissions “for later”. Never force a cloud account for local dictation. If accelerator fails, offer qualified CPU fallback instead of blocking onboarding. Preserve progress across restart where safe *(roadmap)*.

## Tests

`E2E-ONBOARD-CLEAN`, `E2E-ONBOARD-DENIED-MIC`, `E2E-ONBOARD-MODEL-FAIL`, `E2E-ONBOARD-CPU-FALLBACK`, `A11Y-ONBOARD-KEYBOARD`.
