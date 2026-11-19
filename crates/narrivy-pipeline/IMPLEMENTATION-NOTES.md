# IMPLEMENTATION-NOTES — crates/narrivy-pipeline

Status: P1 pipeline-tester pass. Test coverage raised from 1 test to 52.
No production code in this crate was modified; all new coverage lives in
`tests/contracts.rs` (integration target) plus the pre-existing unit test in
`src/legacy.rs`.

## Files

| File | Change |
| --- | --- |
| `tests/contracts.rs` | ADDED. 51 integration tests over 8 contract areas. |
| `src/lib.rs`, `src/legacy.rs`, `Cargo.toml` | UNCHANGED (no bugs found that required fixes). |

## API map (as tested)

- **Slots**: `CapabilitySlot` (16 variants, dotted wire names via serde),
  `CapabilitySlot::ALL`, `wire_name()`, `Display`.
- **Singletons**: `SINGLETON_SLOTS` = audio.capture, speech.asr, speech.endpoint,
  context.provider, delivery.direct, quality.qualifier (6).
- **Streams**: `StreamKind {audio, speech, transcript, text}` (closed);
  `slot_io(slot) -> (in, out)` is total over all 16 variants.
- **Resources** (PORT-008 / ADR-0017): `ResourceKind` (kebab-case wire names);
  `ResourceRef` (`namespace` ∈ {"narrivy","upstream"}, simple version grammar
  `"x.y.z"` exact or `">=x.y.z"` minimum); `ResourceIdentity` (kind+namespace+
  name+version+dependencies+compatibility, deliberately NO url fields);
  `ResolutionHint` (carries `upstream_url`/`mirror_url`/`sha256` separately);
  `satisfies()` = kind+name match + componentwise numeric compare with
  zero-padding; unparseable versions never satisfy.
- **Triggers**: `TriggerKind` (kebab-case); `normalize_binding` grammar =
  lowercased tokens, deduped modifiers in canonical order ctrl<alt<shift<super,
  exactly one non-modifier key; `TriggerBinding::hotkey()` stores normalized;
  `canonical()` normalizes hotkeys strictly and trims/lowercases AppRule/Cli/
  WakePhrase; enabled triggers must be stored already-canonical.
- **Definition**: `PipelineDefinition` (schema_version defaults→1 on load;
  slug id `[a-z0-9-]{1,64}`; per-pipeline monotonic `version >= 1`; nodes/
  edges/triggers/perf_budget/timestamps all serde-defaulted). Node axes:
  `enabled` (user toggle) vs `optional` (skippable-on-failure) are independent.
  `validate()` order: header → node ids → resource refs → exactly-one-asr →
  singletons → edge endpoints → stream kinds → acyclicity (Kahn) → triggers
  (canonical form of enabled, uniqueness of enabled canonical bindings,
  no-enabled-trigger unless manual_only).
- **Errors**: `PipelineError` variants as exercised per test below.
- **Registry**: `insert` (validate + dup-id rejection + cross-pipeline conflict +
  timestamp stamping when zero), `update` (validate + exact current+1 bump,
  preserves created_unix_ms, refreshes updated_unix_ms), `get`, `list`,
  `is_enabled`, `set_enabled` (unknown id rejected), static
  `PipelineRegistry::validate_bindings_across`.
- **Persistence**: `save_to_dir` writes pretty `<id>.json` per pipeline +
  `registry.json` index (`REGISTRY_INDEX_FILE`) with trailing newline;
  `load_from_dir` returns `LoadReport { registry, skipped, migrated }`, skips
  corrupt/unparseable/schema-violating files with reasons, migrates missing or
  zero `schema_version` to 1 (reported in `migrated`), rejects future schemas,
  restores enabled flags from the index best-effort, errors `Io(NotFound)` on a
  missing directory.

## Coverage matrix (tests/contracts.rs)

1. **Validation matrix**: minimal legal def; full dictation chain
   (capture→denoise→vad→endpoint→asr→punct→itn); cycle (incl. self-loop);
   unknown edge endpoints (both sides); stream mismatch audio→transcript and
   text→audio; two speech.asr (exactly-one-asr fires first); zero asr; other
   singleton duplicated (two audio.capture); duplicate node ids; empty triggers
   without manual_only; all-disabled triggers without manual_only; manual_only
   accepted with none; un-normalized stored binding rejected; duplicate enabled
   binding rejected; disabled duplicate exempt; invalid slug/version/name/nodes/
   future schema_version; invalid ResourceRef namespace/name/version_req.
2. **normalize_binding**: case folding; modifier reordering ("Shift+CTRL+Space"
   → "ctrl+shift+space"); whitespace tolerance; empty/double-plus/modifier-only/
   multi-key rejection; duplicate modifier rejection with raw echo;
   `hotkey()` constructor normalization; non-hotkey canonicalization;
   `enabled_binding_keys` filters by enabled.
3. **Registry**: insert/get/list round-trip + timestamp stamping; duplicate-id
   rejection (validation runs first for fresh-but-invalid defs); update same-
   version and skip-version both rejected with exact expected/actual, exact +1
   accepted, old version preserved on failure, unknown-id rejected; created ts
   preserved across update, updated refreshed; set_enabled toggles visible in
   `list()`/`is_enabled()`, unknown id rejected.
4. **Cross-pipeline**: insert rejects same *normalized* enabled binding even
   with different raw spelling ("Ctrl+Space" vs "CTRL+SPACE"); distinct binding
   fine; static `validate_bindings_across` accepts distinct / flags shared;
   disabled bindings exempt.
5. **Persistence**: save layout (`<id>.json` + `registry.json`, trailing
   newline, schema fields present); load round-trips definitions byte-for-byte
   at the struct level (`PartialEq`) including perf_budget, optional flag,
   device_preference, timestamps, and per-pipeline enabled flags; corrupt JSON +
   valid-JSON-invalid-schema files skipped with reasons while good files load;
   legacy hand-authored file (no schema_version, omitted optional fields)
   migrates to schema 1, fills serde defaults, stamps timestamps on re-insert;
   future schema_version skipped; nonexistent dir → Io NotFound; corrupt index
   reported but does not block pipeline load.
6. **Resources**: `ResourceIdentity` serializes with NO url-ish key (checked
   against url/urls/upstream_url/mirror_url/download_url); `ResolutionHint`
   carries upstream_url/mirror_url/sha256 and its optional fields default to
   None; `satisfies()` exact/>=/kind-mismatch/name-mismatch/unparseable cases;
   zero-padded compare ("1.2" ≡ "1.2.0"); ResourceRef structural validation.
7. **Slot I/O completeness**: loop over `CapabilitySlot::ALL` — every variant
   returns a concrete `(StreamKind, StreamKind)` pair; wire_name == serde wire
   name == Display; 16 distinct slots; every slot_io pair spot-checked against
   the documented table; SINGLETON_SLOTS ⊆ ALL.
8. **Serde wire stability**: definition serialize→deserialize→equal; unknown
   top-level and nested-node fields tolerated; StreamKind lowercase and
   TriggerKind kebab-case wire names pinned; PerfBudget inline round-trip.

## Bugs found / fixed

None required code changes. Two observations recorded instead of edits:

1. **No Transcript→Text conversion slot exists.** The slot I/O table makes an
   ASR-to-delivery edge chain topologically impossible: speech.asr outputs
   Transcript, all delivery.* slots require Text input, and no slot has input
   Transcript and output Text. Tests build legal chains that stop at
   transcript-stage slots and leave delivery nodes unconnected. If end-to-end
   dictation graphs are meant to be edge-connected, a converter slot (or a
   Text-typed output on a transcript stage) needs a deliberate ADR-level change;
   out of scope for a test-only pass.
2. **Two speech.asr nodes report `MissingOrMultipleAsr{found:2}`, not
   `SingletonViolation`.** Validation checks exactly-one-asr before the general
   singleton scan. Both are correct rejections; only the error variant differs.
   Documented so callers matching on the variant are not surprised.

## Deviations from the task brief

- All tests live in the added `tests/contracts.rs` integration target rather
  than `#[cfg(test)]` modules inside `lib.rs`, because every contract type is
  already public — no production file needed to be touched.
- The test file carries a crate-level
  `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`: the
  workspace denies these lints for production code, but assertions in tests
  legitimately unwrap/panic. Lib-target clippy was already clean without it;
  the allow keeps `--all-targets` clean too.
- Extra tests beyond the brief (kept small): self-loop cycle, structural header
  validation (slug/version/name/empty-nodes/future-schema), ResourceRef
  validation rules, trigger-kind/stream-kind wire-name pinning, timestamp
  preservation semantics, corrupt-index handling.

## Verification

- `cargo test -p narrivy-pipeline` → 52 passed, 0 failed
  (51 contracts + 1 legacy::tests::legacy_plan_requires_remote_processing_still_works).
- `cargo clippy -p narrivy-pipeline --quiet` → clean (exit 0).
- `cargo clippy -p narrivy-pipeline --quiet --all-targets` → clean (exit 0).

Determinism: tests use fixed fixtures and `tempfile` scratch dirs; the only
clock use asserts monotonicity (updated >= created + slept delta), which cannot
flake under real time.
