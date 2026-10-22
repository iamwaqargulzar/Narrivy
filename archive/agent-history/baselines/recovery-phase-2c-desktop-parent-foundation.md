# Recovery Phase 2C desktop-parent foundation — runtime inert

**Recorded:** 2026-08-27  
**HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (unchanged)  
**Status:** controlled adapter and lifecycle model independently reviewed;
production composition, native qualification, every requirement, packaging,
and release remain blocked

## Result

This increment adds the minimum desktop-owned foundation needed to parent a
future Wayland portal permission dialog without making that dialog reachable:

- `adapters/desktop-parent-linux` isolates GTK3 Wayland export/unexport in one
  reviewed FFI module. It accepts only a visible, focused, realized Wayland
  window on GTK's main thread, allows one exact ticketed lease, validates and
  redacts the opaque handle, rejects main-thread waiting, and provides a
  shutdown-only exact orphan release.
- The Linux-only desktop `portal_parent` module is a pure generic coordinator.
  It owns no GTK object, IPC client, worker, Tauri command, frontend state, or
  runtime hook. An exclusive `NativeExportAdmission` performs the synchronous
  native begin and installs the accepted ticket before returning the callback
  request, so cleanup cannot race acceptance or erase another authority state.
- Accepted-export callback failure is typed as `ParentExportFailed`, preserves
  the ticket until matching main-thread release, and is not mislabeled as user
  cancellation. Dispatch ambiguity forces cancel followed by unconditional
  revoke. Original and remote terminals are reconciled before release.
- Local parent-release uncertainty suppresses publication and replay. Remote
  cleanup uncertainty after confirmed local release may replay one bounded
  close/exit for UX but never reopens admission. Replay is explicitly armed and
  consumed; a stronger Exit racing an armed Close is queued exactly once.

The adapter is only a workspace member. The desktop does not depend on it. The
coordinator is exposed only as a compiled Linux module and is never
constructed, managed, registered, called from setup/run/window events, bridged
to the frontend, or connected to authenticated IPC. The default daemon still
injects no portal service. Therefore this increment cannot export a window,
show a prompt, open EIS, submit text, or promote a capability.

## Architecture and policy records

- ADR-0028 records the narrow GTK3 FFI boundary, exact export ownership, and
  startup-disabled status.
- ADR-0029 records the pure coordinator, exclusive admission, terminal
  reconciliation, uncertainty classes, and one-shot replay contract.
- `docs/02-architecture/component-boundaries.md` records desktop-parent
  ownership without moving platform or domain policy into the UI.
- `THIRD_PARTY_NOTICES.md`, `agent/RESEARCH_LOG.md`, `agent/DECISIONS.md`, and
  `agent/BLOCKERS.md` truthfully retain the unsuppressed GTK3 dependency risk.

## Independent review

Three independent read-only audits returned **PASS** after counterexamples were
fixed. They specifically rechecked ambiguous dispatch, Active-versus-close,
stale request/release/revoke completions, accepted-export callback failure,
local versus remote uncertainty, replay escalation/consumption, opaque-value
redaction, runtime inertness, documentation truth, and dependency policy. The
reviewers made no edits and performed no native interaction.

## Validation

| Command | Exit | Material result |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | Full workspace formatting passes. |
| `git diff --check` | 0 | No whitespace errors. |
| `cargo test -p narrivy-desktop-parent-linux --all-targets --locked` | 0 | 10 adapter/FFI ownership tests passed. |
| `cargo clippy -p narrivy-desktop-parent-linux --all-targets --no-deps --locked -- -D warnings` | 0 | Adapter production and tests are warning-free. |
| `cargo test -p narrivy-desktop --lib --locked -- portal_parent` | 0 | 20 lifecycle/race tests passed. |
| `cargo clippy -p narrivy-desktop --lib --no-deps --locked -- -D warnings` | 0 | Desktop production library is warning-free. |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | Every target and feature compiles. |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | Full Rust workspace sweep passed; the existing native portal observation remained ignored. |
| Desktop `npm test -- --run` / `npx tsc --noEmit` | 0 / 0 | 17 files and 24 tests passed; existing React `act(...)` warnings remain. |
| `python3 scripts/verify-boundaries.py` | 0 | Scanner exits zero, but independent inspection found it misses existing `std::os::unix`, Unix socket, credential, and flock coupling in protected crates; this is not treated as architecture proof. |
| `python3 scripts/verify_dependency_boundaries.py` | 1 | Target-unaware traversal reports `windows-sys` through `tempfile`; ADR-0030 records the deeper real boundary split and validator correction required. |
| Focused adapter `cargo deny` bans/licenses/sources | 0 | Policy passes with duplicate and unmatched-allowance warnings. |
| Focused adapter `cargo deny` advisories/licenses/sources | 1 | GTK3 bindings and `proc-macro-error` are unmaintained; `glib 0.18.5` has RUSTSEC-2024-0429. No advisory was suppressed. |
| Full workspace Clippy with warnings denied | 101 | Existing CLI type complexity, IPC benchmark panic helpers, core worker lint, model-manager/test lint, and recovery-test unwrap failures remain. |

The GTK3/glib/proc-macro advisory set was already present through the locked
Tauri/Wry desktop dependency graph at base HEAD. The new adapter introduces no
new affected registry package, but depending on those packages directly still
cannot be release-qualified. `B-DESKTOP-008` remains open pending a maintained
desktop-runtime decision or a separately justified containment ADR.

## Fail-closed governance and non-mutation

| Command | Exit | Material result |
|---|---:|---|
| `python3 -m unittest tests.governance.test_recovery_validators -v` | 0 | 12 passed. |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped exactly once; all 98 Linux-1.0 requirements remain blocked. |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical/quarantined schema-v1 records; zero v2 candidates. |
| `python3 scripts/validate_pack.py` | 1 | Expected dirty/unsealed manifest mismatch; read-only. |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | Expected fail-closed result with all 98 requirements blocked. |
| `python3 scripts/run_gate.py --phase P11` | 2 | Quarantined and explicitly non-mutating. |

The tracked-or-unignored workspace inventory/content, Git diff, and Git status
hashes were identical immediately before and after exactly those six commands:

- workspace inventory/content:
  `3cf4575ca07fab1748c18615407ece8b66a7b6a46b8af0389200f39a9c34e229`;
- Git diff:
  `8eec0668e6c40c76dc5fb194562d641821dc52db5541e0f3b18229a559de51ec`;
- Git status:
  `0c18fe2b4abd39b49feb7a94b392dce4dfc1e93137775226c5459e5152cfaea9`.

Protected files and governance catalogs remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- evidence index: `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Cleanup and non-claims

No `narrivyd`, `pw-record`, or `transcribe-cli` process remained, and no
top-level `/tmp/narrivy-*` or `/tmp/narrivy_*` directory remained. This
increment created no audio, transcript, target text, portal session, or runtime
payload. Cargo/npm build caches remain ordinary developer state and were not
misrepresented as payload residue.

No native GTK export, portal prompt, EIS connection, external input event,
microphone run, target window observation, evidence/index entry, requirement
status, manifest reseal, package, commit, or push occurred. All 98 Linux-1.0
requirements and release remain blocked.
