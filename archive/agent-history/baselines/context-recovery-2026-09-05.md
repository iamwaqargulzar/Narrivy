# Context recovery — 2026-09-05

## Authority and repository identity

Owner requested a project recheck and restart of the autonomous process after
context loss. Resume the existing working tree; do not reset or replace it.
The configured target remains Linux 1.0 / P11, with automatic phase advancement
only after trustworthy gates. HEAD is ff6d864012adb1468d02c6b5f80b9ce92258fa04.
Recovery implementation is uncommitted. The 4,955 tracked nested-target
deletions are the documented B-HYGIENE-011 cleanup, not missing source.
No commit or push was performed during this recheck.

Read SOURCE_OF_TRUTH.md, BUILD_TARGET.json, MASTER-AUTONOMOUS-BUILD-PROMPT.md,
AGENTS.md, IMPLEMENTATION-PLAN-v1.3.md, PRODUCT_CHARTER.md, the architecture map,
autonomy/failure policies, P11 spec/prompt/acceptance, state, blockers, recent
research/decisions, ADR-0064, and recovery evidence policy. Older scaffold
startup instructions are historical where they conflict with recovery policy.

## Reconstructed implementation map

- Portable domain/contracts: core, pipeline, policy, types and API crates.
- Native mechanisms: Unix IPC/journal, Linux process and desktop-parent adapters.
- Service: daemon-owned capture, resident-ASR lifecycle, durable recovery,
  provisioning and command policy; clients remain policy consumers.
- ASR: process-isolated transcribe.cpp worker with immutable executable/model
  bindings and bounded streaming/cancellation/restart work recorded in 4A–4Q.
- Recovery storage is separate from optional History. Native correctness and
  package lifecycle claims still require new acceptance observations.
- Desktop: additive GTK4/WebKit6 shell uses shared bridge/theme/event logic
  and React assets. Tauri/GTK3 stays in the parity graph and blocks dependency
  acceptance. No production replacement qualification has occurred.
- Packaging: historical builder and legacy evidence refreshers are quarantined.
  Disposable composition tests are structural checks, not installable artifacts.

## Newly recovered unfinished work and repair

The disk contains overlay.rs integrated into the GTK shell, although STATE and
P11 report described the overlay as absent. Treat it as an unfinished prototype.
It has no native focus/placement/portal parity qualification.

Fixed partial admission after leaving Listening: preparing, processing and
terminal states now reject late partials. Processing delay clears the previous
partial immediately; hiding the window also clears its label. Added two
regressions covering all processing/terminal states and preparing-to-listening.
No transcript content was processed in this recheck; tests use fixed fixtures.
This is local source/test validation only, not evidence-index registration.

## Fresh checks

- Doctor: exit 0; COSMIC/Wayland, Rust/Cargo 1.97.1, Python 3.12.3,
  Node 24.19.0, portal/PipeWire/Secret Service available.
- GTK tests: 23 pass; GTK all-target Clippy with warnings denied passes.
- Frontend: 30 tests across 18 files pass.
- Governance: 36 tests pass. Packaging: 27 tests pass.
- Source and dependency boundaries: all nine protected packages pass.
- Traceability: 136 requirements mapped; 98 Linux release rows remain blocked.
- cargo deny: advisories fail; bans/licenses/sources pass. No ignore added.
- Legacy run_gate P10: exit 2, correctly quarantined. No previous passing phase
  exists after the evidence reset; do not reinterpret historical P10 as passing.
- Pack/release checks: fail on stale/unlisted manifest entries, blocked rows,
  historical evidence and mismatched release identity. No manifest was resealed.

## Acceptance backlog and resume order

1. Complete review of the recovered overlay: ordering across reconnects,
   transaction identity, bounded timer/window lifetime, accessibility, native
   non-focus behavior and truthful Wayland placement limits.
2. Implement/review GTK4 portal-parent ownership, retaining deterministic
   cancel → revoke → original terminal → unexport semantics; finish close/exit
   parity before removing the legacy graph under ADR-0064.
3. Close the pending independent provisioning integration review and disposable
   real-model activation observation described in the C5B2B2 baseline before
   considering provisioning accepted or adding provisioning UX.
4. Resolve all dependency advisories without exceptions, then installed
   runtime/model provenance, cgroup containment and reproducible package lifecycle.
5. Establish reviewed schema-v2 procedures and clean artifact-bound qualification
   on native COSMIC and GNOME, plus CPU fallback and the required target matrix.

No qualified evidence exists in INDEX.json. P11 and all 98 Linux requirements
remain blocked. A passing local test suite does not change those claims.

Workspace verification completed: `cargo test --workspace --locked` exits 0;
1035 tests pass, 2 are explicitly ignored. This default-feature run does not
replace all-feature or native qualification.
