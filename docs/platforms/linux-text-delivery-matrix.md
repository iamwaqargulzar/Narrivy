# Linux Text Delivery Qualification Matrix

System-wide text delivery is a routing problem. No single backend is assumed to work in every Wayland/X11/application combination.

## Backend classes

| Backend class | Typical strength | Main risks | Policy |
|---|---|---|---|
| input-method/native text commit | correct text semantics when available | integration/desktop/input-method specificity | preferred after qualification |
| desktop/accessibility/automation protocol | can address broader targets | permissions, compositor/version support | allowed with least privilege + evidence |
| virtual-keyboard/keyboard-emulation mechanism | broad fallback in some sessions | layouts, Unicode, permissions, compositor restrictions | bounded fallback only |
| clipboard + paste chord | pragmatic compatibility | clipboard ownership/history, paste shortcut variance, sensitive clipboard | final active-target fallback before Inbox when policy permits |
| Inbox | cannot insert into target but never loses final text | requires user copy/paste | mandatory safe sink |

Specific candidates (for example input-method integrations, libei/compositor mechanisms, or audited helper utilities) must be researched and qualified during P2. Do not encode an implementation choice in core.

## Router input

- current session/desktop capabilities;
- target application/window identity when available;
- target compatibility observations;
- requested operation (insert vs replace);
- content shape (Unicode, multiline, size);
- user/privacy policy;
- backend health/recent failure;
- required permissions.

## Router output

A `DeliveryOutcome` records transaction ID, final result (`committed`, `inbox`, `cancelled`, `failed`), attempted backend IDs, bounded failure reasons, verification confidence, clipboard restore outcome if applicable, and target metadata safe for diagnostics. It never contains transcript content in ordinary diagnostics.

## Clipboard fallback invariants

- preserve prior clipboard data where the desktop protocol makes this safe and predictable;
- do not overwrite clipboard before final text is durable in the recovery journal;
- restore only if ownership/version checks show Narrivy still owns the temporary clipboard value; never clobber a newer user clipboard change;
- bounded timeout and explicit Inbox fallback;
- document interaction with clipboard managers.

## Target-app qualification table

P3/P11 must populate actual rows; placeholders are not support claims.

| Environment | Target class | Insert | Unicode | Multiline | Replace selection | Backend | Evidence |
|---|---|---|---|---|---|---|---|
| COSMIC Wayland | COSMIC Terminal | unqualified | unqualified | unqualified | unqualified | TBD | |
| COSMIC Wayland | Firefox | unqualified | unqualified | unqualified | unqualified | TBD | |
| COSMIC Wayland | Chromium/Chrome | unqualified | unqualified | unqualified | unqualified | TBD | |
| COSMIC Wayland | VS Code/Electron | unqualified | unqualified | unqualified | unqualified | TBD | |
| COSMIC Wayland | LibreOffice | unqualified | unqualified | unqualified | unqualified | TBD | |
| GNOME Wayland | terminal | unqualified | unqualified | unqualified | unqualified | TBD | |
| GNOME Wayland | Firefox | unqualified | unqualified | unqualified | unqualified | TBD | |
| GNOME Wayland | Chromium/Chrome | unqualified | unqualified | unqualified | unqualified | TBD | |
| GNOME Wayland | VS Code/Electron | unqualified | unqualified | unqualified | unqualified | TBD | |
| GNOME Wayland | LibreOffice | unqualified | unqualified | unqualified | unqualified | TBD | |
