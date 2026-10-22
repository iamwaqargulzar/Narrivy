# Shortcuts: press a button, press your keys

> **Status: design approved by the owner on 2026-09-30 (D24); implemented
> for COSMIC on 2026-09-30** (adapter `adapters/platform-linux/src/
> desktop_shortcuts.rs`, service `services/narrivyd/src/daemon/shortcuts.rs`,
> protocol minor 11). Not yet done: removing Narrivy's bindings on package
> purge, and the check that a saved key actually triggers. It follows ADR-0016 (triggers bind to pipeline ids)
> and the preference order in
> [shortcut-router.md](../../architecture/shortcut-router.md).

## What the owner asked for

Setting a shortcut should work the usual way: click a button, press the key
combination, done. One pipeline can have several. Copying a command into
the desktop's keyboard settings stays as the second option.

## What the desktop allows (checked on the reference machine)

- COSMIC has no global-shortcuts portal (`xdg-desktop-portal-cosmic
  0.1.0pop1`), so an application cannot register a global key itself.
- COSMIC keeps custom shortcuts in a small text file in the user's
  configuration, one entry per binding, each running a command on key
  **press**. The reference machine has one such entry today, for another
  dictation app on Ctrl+Space.
- So Narrivy can offer the usual capture flow and then add the binding to
  the desktop's own custom shortcuts, pointing at `narrivy-cli toggle
  --pipeline ID`. This is option 2 of the router order ("native desktop
  binding"), ahead of option 3 ("the user does it by hand").
- A press-only binding cannot do press-and-hold. Hold stays with T2.13.

## Flow

1. On a pipeline: **Add shortcut**. A small dialog says "Press the keys".
2. Keys are read only while that dialog has focus, and only to build the
   combination. Nothing else is recorded.
3. The combination is checked (see below), shown back, and saved on confirm.
4. The pipeline shows its shortcuts as key chips, each with remove. Language
   shortcuts work the same way with their language pair.
5. If the desktop offers no way to register a shortcut, the button is
   replaced by the command to copy, with the reason.

## Where each part lives

- **Frontend:** the capture dialog and the chips. It sends a combination and
  a pipeline id, and renders what the service reports. No desktop names.
- **Service:** owns the list of bindings and validation, reports
  `activation.shortcuts=managed|manual|unavailable:<reason>`.
- **Platform adapter (`adapters/platform-linux`):** the only code that knows
  how one desktop stores shortcuts. One implementation for COSMIC first;
  others (and the portal, when a desktop has it) slot in behind the same
  contract.

## Rules

- A shortcut can be one key or several (owner, 2026-09-30):
  - a key that types nothing, on its own: F1–F24, Pause, Insert, Scroll
    Lock, Print Screen or Menu;
  - Ctrl, Alt, Super or Shift on their own or together, which run when
    released without another key (the way Super opens the launcher);
  - up to four modifiers with one other key.

  A key that types text (a letter, a digit, Space or punctuation) is
  refused on its own or with only Shift, because the desktop would take it
  away from every application.
- Combinations the desktop reserves are refused with the owner's name
  ("used by the desktop for switching windows").
- A combination another entry already uses is a conflict: show what has
  it and offer both choices: pick another combination, or replace that
  binding. Replacing needs an explicit confirmation that names what will
  stop working; it is never silent. The replaced entry is kept so it can be
  restored if the Narrivy shortcut is later removed.
- Apart from a confirmed replacement, Narrivy only ever edits or removes
  bindings it created. It records which ones are its own.
- Writes are atomic (write beside, then rename), keep every other entry
  byte-for-byte, and are re-read afterwards to confirm the binding exists.
- Uninstall (purge) removes Narrivy's bindings and nothing else.

## Edge cases

| Case | Behaviour |
|---|---|
| The desktop's shortcut file has a shape Narrivy does not recognise (new desktop version) | Do not write. Report `manual`, show the command to copy |
| File changed by the desktop's settings while Narrivy edits | Re-read, merge, retry once; on a second conflict stop and ask |
| User removes the binding in desktop settings | Noticed on next read; the chip disappears, no error |
| User renames a pipeline | Bindings use the id, so they keep working |
| Pipeline deleted or disabled | Its bindings are removed (deleted) or shown as inactive (disabled) |
| Same keys asked for two pipelines | Refused; one combination starts one pipeline |
| The CLI is not at the installed path (development build) | The binding uses the path the service reports for its CLI |
| Keyboard layout change | Bindings are stored as key names, as the desktop stores them |
| Desktop does not reload the file immediately | Verify after write; if the key does not trigger within the check, say a sign-out may be needed |
| Another desktop (no adapter yet) | `manual`: command to copy |
| A portal for global shortcuts becomes available | Preferred automatically; existing bindings migrate, none are duplicated |
| Press-and-hold wanted | Not possible through a press-only binding; offered through T2.13's opt-in path |

## Owner decisions (D24, 2026-09-30)

1. Narrivy may add and remove its own entries in the desktop's custom
   shortcuts. Onboarding changes from "does not create or change desktop
   shortcuts" to "changes only the ones you set here".
2. A combination in use: explain, then offer to pick another or to replace
   it after confirmation.
3. COSMIC first; other desktops after 1.0.
