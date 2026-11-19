# Narrivy Design System

`spec/ui/design-tokens.json` is the machine-readable initial token set. Components consume semantic tokens rather than scattered literals.

## Foundations

### Spacing
Use a 4px base rhythm with named semantic steps. Default control spacing should feel calm, not sparse. Dense layouts are reserved for Diagnostics/tables and remain keyboard/touch-target accessible.

### Typography
Use the platform/system UI font stack by default for native feel and rendering quality. Tokenize sizes/line heights/weights. Body text must scale without clipping; code/IDs use a monospace stack only where appropriate.

### Shape/elevation
Use restrained radii/elevation. Hierarchy should come primarily from spacing, typography and surface relationships rather than excessive cards/shadows.

### Color semantics
Tokens represent surface/content/border/accent/success/warning/error/info/focus/selection states with light/dark/high-contrast variants. Component code does not treat a literal hue as meaning “error”.

### Motion
Tokenize fast/standard/slow durations and easing. Reduced-motion variant removes nonessential translation/scaling while preserving necessary status changes.

## Core components

Button, icon button, split/dropdown action, toggle, checkbox/radio, segmented control, text field, search, combobox, slider, keybinding recorder, cards, list/table, tabs, breadcrumbs where needed, dialogs, sheets, popovers, tooltips, banners, toasts, progress, skeleton, empty/error/degraded states, permission card, privacy badge, capability check row, model card, Mode/Profile card, status pill, and dictation overlay primitives.

## Component contract

Every interactive component defines:

- default/hover/pressed/focus-visible/disabled/busy/error states;
- keyboard behavior;
- accessible role/name/state/description;
- minimum hit target;
- text expansion behavior;
- high-contrast and reduced-motion behavior;
- automated tests for important states.

Visual components receive typed state/callbacks; they do not call platform/runtime services directly.

## Icons

Use one coherent icon set with accessible labels supplied by context. Do not use icons as the only explanation for unfamiliar product concepts. Decorative icons are hidden from the accessibility tree.

## Density

Default UI uses comfortable density. Diagnostics may use compact density but retains readable line height/focus. Never expose a global “tiny” density that compromises accessibility.

## Theme

Support system/light/dark. High-contrast compatibility is required; do not rely on translucent surfaces that collapse under forced/system contrast modes. User theme choice persists independently of system when explicitly set.

Dark is the first-run default. Light and system remain complete selectable themes rather than reduced variants.
