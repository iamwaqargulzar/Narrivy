# Accessibility Acceptance Standard

Narrivy 1.0 uses applicable **WCAG 2.2 AA principles** for its webview-based UI together with native desktop accessibility semantics and actual assistive-technology checks.

## Keyboard

- all controls/features operable without pointing device;
- logical focus order follows visual/task order;
- focus-visible is always perceivable;
- no keyboard traps in dialogs/popovers/keybinding recorder;
- focus returns predictably after transient surfaces close;
- global dictation binding and in-app shortcuts avoid inaccessible conflicts and can be changed.

## Semantics/screen readers

- correct roles/names/states for controls;
- form errors linked to the field and announced without repeated chatter;
- status/live regions used sparingly for dictation state and background operations;
- icon-only buttons have meaningful names;
- tables/lists expose structure;
- disabled capability controls expose the reason textually.

## Visual

- text/UI scale through at least the defined 200% acceptance scenario without loss of primary content/action;
- high-DPI and mixed-scale displays tested on reference Linux environments;
- high-contrast compatible semantic tokens;
- status never color-only;
- focus contrast/visibility tested in light/dark/high-contrast scenarios.

## Motion/audio

- reduced motion removes nonessential movement;
- no flashing status;
- auditory feedback, if offered, has visual/semantic equivalent and independent volume/toggle where appropriate.

## Voice as accessibility

Normal dictation reliability/privacy remains the foundation. Future hands-free command/grid features are roadmap and cannot weaken explicit permissions or safe action scope.

## Test evidence

P9/P11 produce an accessibility report covering automated DOM/component accessibility checks plus manual keyboard, screen-reader smoke, scaling, high-contrast and reduced-motion scenarios. Automated scanners alone cannot close `TEST-007`.
