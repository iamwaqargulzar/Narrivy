# Narrivy Desktop

The React/TypeScript client is hosted by a Tauri 2 shell in `src-tauri` on
every OS (ADR-0075, which supersedes the GTK4 shell of ADR-0064).

The shell is a client of `narrivyd`. Framework-neutral Rust owns daemon IPC,
response validation, theme persistence, and event subscription; the frontend
and the shell must not duplicate session, model, privacy, pipeline, or
platform policy.

For a development build, create the embedded frontend assets first:

```bash
npm ci
npm run build
cargo run -p narrivy-desktop
```

To work on the interface in an ordinary browser, run `npm run dev` and open
<http://localhost:1420>. Outside Tauri, development mode swaps in a preview
bridge (`src/lib/previewBridge.ts`) that replays realistic daemon state and a
dictation session. Production builds never include it.

On Linux the crate needs Rust 1.92 or newer plus the WebKitGTK 4.1, GTK 3,
gtk-layer-shell, ayatana-appindicator and librsvg development files. The
pinned project toolchain satisfies that desktop MSRV; the portable crate set
keeps its separate Rust 1.85 check.

The Linux overlay is a native GTK layer-shell card that never takes keyboard
focus; when the compositor has no layer-shell support the overlay is never
shown. Native COSMIC qualification of the Tauri shell is still required
before release evidence is recorded.
