> Recovery ruling 2026-08-30: History and the short-lived recovery journal are
> distinct. Live session UX uses the exact transaction-scoped recovery request
> and never scans recovery records.
>
> Amended 2026-09-27: the daemon now composes a real optional History store
> (`services/narrivyd/src/daemon/retained_history.rs`). History reads remain
> optional and capability-gated — the client must render History only when
> `history.optional=enabled` is advertised, and must keep failing closed when it
> is absent or when the store reports a storage error. A rejected History
> request must never be presented as an empty list.

# P3 Desktop Client — Shared Contract (binding for all P3 agents)

## Stack

React 18 + TypeScript + Vite, hosted by the provisional GTK4/WebKitGTK 6
Linux shell. The Tauri 2 shell remains temporarily as a migration parity
oracle and is not the selected release runtime. The frontend is a CLIENT of
narrivyd (P1 IPC) — it never owns domain policy. No frontend
`if linux/cosmic/gnome` branching anywhere; capability strings drive UI state.

## Rust client and shell contract

Daemon IPC, response validation, event subscription/reconnect/sequence
handling, theme persistence, and pipeline import/export are framework-neutral.
Only `tauri_commands.rs` and the shell composition in `lib.rs` translate them
to Tauri commands and the `session-event` Tauri event (ADR-0075:
Tauri 2 is the desktop host on every OS).

- `daemon_status() -> StatusSnapshot` (proxies narrivy-ipc GetStatus:
  capabilities[], active_session{state,pipeline_id}, daemon_build)
- `daemon_start(pipeline_id: Option<String>)`, `daemon_stop()`, `daemon_cancel()`
- `daemon_list_pipelines() -> Vec<PipelineSummary>`
- `daemon_history() -> Vec<JournalEntryDto>` (optional History only; rejected
  while the store is disabled/not composed)
- `daemon_recovery_entry(txid) -> RecoveryEntryDetailDto` (one exact
  short-lived transaction reported by a session event; never a listing)
- `daemon_capability_check(id: String) -> CapabilityCheckResult` (new narrivyd
  command; runs ONE isolated check: mic | delivery | shortcuts | theme-store)
- Events: `session-event` (SessionStateChanged seq/state), pushed from a
  socket-subscriber thread.

## Frontend layout
src/app/screens/{Home,Dictation,Models,Applications,History,Settings,Diagnostics,About}.tsx
src/ui/primitives/* (Button, Card, StatusPill, DisclosureSection, Toast)
src/theme/{tokens.css,ThemeProvider.tsx} — System/Light/Dark, persisted
src/i18n/{index.ts,en.json} — message-ID architecture from day one
src/lib/bridge.ts — the ONLY module importing `@tauri-apps/api`; typed
Tauri/WebKit transport selection and wrapper

## Rules
- Deferred features (Snippets, Integrations, cloud anything) are NOT rendered
  as working; either absent or explicitly labeled "Post-1.0".
- Every automatic decision shown as resolved value ("Auto → …") with Reset.
- Progressive disclosure: Settings sections collapse; Advanced last.
- Overlay: separate non-focus-stealing native window (see
  docs/product/ux/overlay.md), Light/Dark/System aware, only
  listening/processing/error states shown. The current Tauri implementation is
  the parity reference; the GTK4 overlay is still release-blocking work.
- Errors always carry a recovery action (UX-007).
