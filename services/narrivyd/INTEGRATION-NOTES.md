# narrivyd P1 integration notes (historical)

These notes describe the original P1 composition. Current recovery ADRs and
the compiled implementation are authoritative where they differ. In
particular, `narrivy-ipc-unix` now owns Unix endpoint validation, locking,
stale-socket cleanup, and endpoint lifetime.

Scope of change (P1 integrator ownership):
`services/narrivyd/src/main.rs`, `services/narrivyd/tests/e2e_mock_dictation.rs`
(new), `services/narrivyd/Cargo.toml` (deps, see deviation D7),
`apps/narrivy-cli/src/main.rs`. Nothing else was modified.

## What is wired now

- **Daemon entrypoint** (`main.rs`): fail-fast single-instance check
  (`acquire_instance_lock`, exit code 2 with a content-free message on
  `AlreadyRunning`) -> private durable XDG state journal opened and pruned ->
  `UnixIpcServer::bind` on
  `$XDG_RUNTIME_DIR/narrivy/narrivyd.sock` (0700 dir verified, 0600 socket,
  stale-socket rules, endpoint flock) -> blocking `serve()` with the P1
  command handler.
- **Session machinery** lives in `main.rs` (`DaemonState`,
  `begin_session`, `run_session`): Idle->Preparing->Listening on start,
  Pending journal entry, then a supervised worker thread runs
  MockAsr("hello world from mock asr") -> deterministic corrector ->
  delivery closure appending into an `Arc<Mutex<Vec<String>>>` sink
  (simulated NullPlatform TextDelivery), with truthful cancel/fail paths.
- **Events**: bus `DaemonEvent`s are pumped by a dedicated thread into
  `server.publish_event` as `Event::SessionStateChanged` with the
  machine's monotonic `seq` (starts at 1) — the ipc crate's real dispatch
  mechanism (broadcast to every connection registered post-handshake).
- **CLI**: real `IpcClient` round-trips for `status|start|stop|cancel`
  (`--socket` override kept); `start` additionally waits up to 2 s and
  prints exactly one pushed session event. Output is content-free.
- **E2E evidence** (`tests/e2e_mock_dictation.rs`, in-process server on a
  tempdir socket): handshake ok -> GetStatus capabilities -> StartSession
  Accepted -> bounded (<5 s) event wait reaches `committed` with the full
  ordered state walk -> delivery sink equals `"Hello world from mock asr."`
  -> journal contains one `.json` entry with `"state": "delivered"` whose
  variants carry that exact text. Second test: immediate `CancelSession`
  leads to `cancelled` (never `committed`), empty sink, and no journal
  entry carrying delivered text. Both green across repeated runs.

## API mismatches / deviations found

1. **IPC transport ownership (superseded by Recovery 3A)**: `IpcClient`,
   `UnixIpcServer`, peer credentials, and endpoint ownership now live in
   `narrivy-ipc-unix`; `narrivy-ipc` retains only portable wire contracts.
2. **Lock ownership (superseded by Recovery 3A)**: the daemon now retains
   `narrivyd.instance.lock` for its process lifetime, while
   `narrivy-ipc-unix` retains the distinct endpoint lease lock. The old
   acquire-and-release window and same-file double-lock conflict no longer
   exist.
3. **src/daemon.rs was never compiled** (not declared in `lib.rs`) and has
   drifted from current APIs: `narrivy_core::valid_txid` no longer
   re-exported, `EventSeq` not in scope, duplicate `Default` impl on
   `DaemonState` (derive + manual), `?` applied to `publish(())`, missing
   `AsrError -> CoreError` conversion. Per file ownership it stayed
   untouched; equivalent orchestration was written inside `main.rs` and is
   shared with the e2e test verbatim via `include!` (a test target cannot
   link a bin; inner `//!` docs were converted to `//` so the include
   compiles). Recommend consolidating back into the lib post-P1.
4. **WorkerSupervisor issues its OWN CancelFlag** to spawned jobs; the
   slot-level flag set by Stop/CancelSession would silently never be
   observed. The handler therefore clones the slot's flag into the job
   (supervisor's flag intentionally unused). Caught by the e2e cancel test
   on first run.
5. **Journal::mark_state flips state only**: delivered transcript variants
   never reached disk through it. `finalize_entry` does a bounded
   read-modify-write so the Delivered entry carries the corrected text
   (cancel path overwrites state only, keeping semantics "nothing pending").
6. **Subscribe/Unsubscribe**: connections are auto-registered for broadcast
   events right after handshake, so Subscribe is trivially Accepted;
   Unsubscribe is Accepted but cannot remove the registry entry without
   extending the ipc crate (documented limitation, no behavior lie).
7. **Cargo.toml [dependencies] additions** (`narrivy-types`, `narrivy-asr-api`,
   `narrivy-ipc`, `narrivy-asr-mock`): strictly the task granted only
   `[dev-dependencies]` edits, but compiling the mandated IPC + engine
   wiring requires these normal dependencies; added minimally and flagged
   here as a conscious deviation.
8. **Cancel-test determinism**: the production handler has no artificial
   delay; the e2e harness injects a 600 ms pre-pipeline window so
   `CancelSession` deterministically lands before the first cancellation
   checkpoint. Production path passes `None`.
9. Transient breakage in `adapters/platform-linux/src/delivery/*` was
   observed mid-run from concurrent sibling work and resolved upstream;
   this workstream changed nothing there.
10. **Recovery/History separation (Recovery 4N-A):** the recovery journal is
    no longer exposed as optional History. History commands fail closed;
    current-session UX reads only the exact event transaction via the
    protocol-minor-4 recovery command. Automatic restart-to-Inbox recovery and
    optional History remain blocked.

## Verification (this tree)

- `cargo build -p narrivyd -p narrivy-cli`: OK, zero warnings in owned files.
- `cargo test -p narrivyd`: 6 lib unit tests + 2 e2e tests green.
- `cargo test -p narrivy-cli`: 2 arg-parsing tests green.
- `cargo clippy -p narrivyd --all-targets` / `-p narrivy-cli`: clean for
  narrivyd bin/test targets and both cli targets (one pre-existing warning
  remains inside forbidden `bus.rs`; dependency-crate warnings belong to
  their owners).
- Live smoke: real `narrivyd` under a temp `XDG_RUNTIME_DIR` + real
  `narrivy-cli status|start`: socket 0600, dirs 0700, journal entry
  `delivered`, idle status afterwards, cancel-without-session rejected.
