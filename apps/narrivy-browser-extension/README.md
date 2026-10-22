# Narrivy Writing (browser extension)

Grammar, spelling and AI rewriting in Chrome and Brave, run by Narrivy on
this computer (D26, SQ-WRITE). The design and feature map are in
`docs/roadmap/design/writing-extension-research.md`.

- **No build step.** These files are the extension (Manifest V3).
- **Its ID is fixed** at `khkbeljpdjbgihgbedobeaoedkdffkie` by the public key
  in `manifest.json`. The native host only accepts that ID.
- **It talks only to the native host** `com.narrivy.writing`
  (`apps/narrivy-browser-host`), which talks to narrivyd. There are no
  network permissions.

## Development install

1. Install the connector:

   ```bash
   scripts/install-browser-extension.sh
   ```

2. Load the extension once:
   1. Open `brave://extensions`.
   2. Turn on Developer mode.
   3. Choose Load unpacked, and pick this folder.

After changing files here, press the reload button on the extension's card
and reload open pages.

## Files

| File | Role |
|---|---|
| `content.js` | <ul><li>Finds fields.</li><li>Draws underlines (mirror overlay for text boxes, CSS Custom Highlights for rich editors).</li><li>Shows the suggestion card, badge, panel, selection button, action menu, preview and toasts.</li></ul> |
| `highlights.css` | The only CSS added to pages (`::highlight()` underlines). |
| `background.js` | <ul><li>One native port.</li><li>Request routing.</li><li>Right-click menu.</li><li>Shortcut.</li></ul> |
| `settings.js` | Settings and the sensitive-site list (popup and settings page). |
| `popup.*`, `options.*`, `ui.css` | Toolbar popup and settings page. |
