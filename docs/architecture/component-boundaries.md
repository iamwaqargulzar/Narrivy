# Component Boundaries

## Portable crates

### narrivy-types
Pure domain value types. No platform/runtime dependencies.

### narrivy-platform-api
Traits for shortcuts, target/context access, text delivery, overlay, notifications, media control, secret store and lifecycle capabilities.

### narrivy-asr-api
ASR model/engine capability and transcription contracts.

### narrivy-policy
Privacy, permission and routing decisions. Must be deterministic and unit-testable.

### narrivy-pipeline
Mode DAG representation, stage planning, cancellation and result propagation.

### narrivy-ipc
Wire-level request/response/event types and version negotiation. Transport adapters stay outside.

### narrivy-journal-api
Recovery-journal record schema, typed storage errors, transaction-ID rules,
retention policy, and the portable storage port. Filesystem mechanics stay in
outward journal adapters.

### narrivy-diagnostics
Content-safe metrics/events/support facts.

### narrivy-compute
Device discovery, per-tuple qualification records, device-class planning and
explainable decisions. Host facts are read through the injected `HostProbe`
port (ADR-0074), never by spawning tools from the core.

### narrivy-model-manager
Model registry, verified install lifecycle, residency/invalidation tracking,
cache budget, offline import and runtime-pack suggestions. Model weights are
catalog content executed through runtimes, not product entities.

### narrivy-history
The optional retained user History store and privacy-zone evaluation. Distinct
from the recovery journal (ADR-0009, ADR-0013, ADR-0045).

### narrivy-core
Session orchestration and high-level application service. Depends on interfaces, not concrete platform/model runtimes.

## Module/adapter families
Existing directories under `adapters/` and `engines/`:

- `adapters/ipc-unix` (`narrivy-ipc-unix`) — authenticated local transports,
  endpoint security, and connection lifecycle; no product policy or
  wire-contract ownership. This is the only current member of the `ipc-*`
  family.
- `adapters/journal-unix` (`narrivy-journal-unix`) — platform persistence,
  private-path validation, bounded durable writes, corruption quarantine, and
  crash-leftover cleanup; no recovery ordering or optional-History ownership.
  Only current member of the `journal-*` family.
- The `desktop-parent-*` family is retired (ADR-0075). The former
  `adapters/desktop-parent-linux` served only the GTK4 shell, which was
  removed when the desktop moved to Tauri 2 on every OS. Portal window
  parenting, if it is needed again, belongs in the Tauri desktop crate.
- `adapters/process-linux` (`narrivy-process-linux`) — Linux trusted-helper
  resolution, generic controlled-build and descriptor-relative trust-anchor
  digest-bound immutable executable/data snapshots, scrubbed role-specific
  launch construction, process-group ownership, parent-death setup, and
  bounded reap; no capture, delivery, ASR, model-selection, or daemon policy
- `adapters/platform-linux` — owns Narrivy-specific fixed installed-runtime
  manifest policy, descriptor-relative complete-directory offline model import,
  and upgrades bundled-registry installed-model observations to Linux immutable
  descriptors; none of these operations claims package/release provenance
- `adapters/platform-macos`, `adapters/platform-windows`, `adapters/platform-null` — one
  directory per OS; there is no `platform-linux-*` subfamily
- `engines/asr-mock`, `engines/transcribe-cpp` — the only engine bindings
  present; `engines/asr-*` names the family, not a fixed member list

**Not present in the tree.** `modules/audio-*`, `modules/compute-*`,
`processors/*`, `context-providers/*`, `delivery/*` and `plugins/*` do not
exist. There is no `modules/` directory at all, and no extension-host contract
has been designed, so the plugin sandbox in ADR-0008 remains Proposed. Module
composition today is the fixed set of capability slots carried by the portable
crates listed above plus these adapters/engines; see `module-system.md` for
the intended shape and `spec/catalogs/modules.json` for the catalog vocabulary
(which describes 66 bindings that are *not* all code).

All are selected through internal capability/module registries. Do not publish a Rust dylib ABI. See `module-system.md`.

## Dependency rule

Dependencies point inward toward contracts. Core never imports concrete adapters.
One implementation adapter may depend on a narrower OS-mechanics adapter only
when that dependency is listed above and the narrower adapter owns no product
policy (for example, `platform-linux` may use `process-linux`). The mechanics
adapter must not depend back on the product adapter.
