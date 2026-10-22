# Polish Disruptor — UX state that is actually enforced

Every claim below is checkable against the tree. Where a claim cannot be
checked, it is recorded as a gap rather than asserted.

**Capability-driven, not OS-branched.** The React client renders capability
tokens such as `asr.transcribe-cpp` and `asr.transcribe-cpp.cpu=…`
(`src/app/screens/Models.tsx:22`,
`src/app/components/SettingsCompute/capabilityInventory.ts:52`). There is no
`if linux` / `isLinux` / `isCosmic` / `isGNOME` product branch in
`apps/narrivy-desktop/src/`.

**Per-token i18n — one locale, and the fallback is the raw key.** All
user-visible strings resolve through `t(id)` in
`src/i18n/index.ts`, backed by `src/i18n/en.json` (284 message ids). Every
`t()` id currently used in the client resolves against that catalog, so there
is no id drift today. But **English is the only locale in the tree** and
`index.ts` falls back to the message id itself when a key is missing
(`let out = MESSAGES[id] ?? id`). There is no second language and no
raw-key detector. `docs/development/i18n/architecture.md` requires that "missing
translations fall back visibly/safely rather than rendering raw keys in release
builds" — that requirement is **not met by the current implementation**. The
earlier claim of "100% translation coverage" was wrong: what exists is a
single-locale catalog with an unchecked id, not a coverage guarantee.

**Design tokens.** `--space-1..6`, `--accent`, `--accent-strong`,
`--focus-ring` and the reduced-motion overrides are defined in
`src/theme/tokens.css`; component rules are in `src/theme/app.css`.
`src/__tests__/A11yLandmarks.test.tsx` asserts landmark roles, icon-button
labels, skip link and the `focus-visible` ring.

**Motion.** `.btn-press` uses `transform 150ms` (`app.css:126`);
`.overlay-fade-in`, `.palette-overlay` and `.pill` use `120ms`
(`app.css:128,130`); `prefers-reduced-motion: reduce` forces
`transition-duration`/`animation-duration` to `0ms !important`
(`app.css:142-144`).
`src/__tests__/MicroInteractions.test.tsx` asserts those values by reading the
stylesheet text — it does **not** observe runtime animation, so the earlier
"verified in jsdom via `matchMedia` mocks" overstated what the test proves.

**Dictation modes are pipelines, not UI presets.** Raw/Clean are built-in
pipeline identities (`services/narrivyd/src/daemon/policy.rs`, `RAW_PIPELINE_ID`
/ `CLEAN_PIPELINE_ID`) passed to `daemon_start`, not client-side text modes.

**History list is windowed.** `src/__tests__/HistoryVirtualized.test.tsx:24-47`
renders 1000 entries and asserts fewer than 50 list children. The earlier "60
fps on low-end hardware" claim has **no measurement behind it anywhere in the
tree** and is withdrawn.

**PERF-004 is not measured.** `spec/requirements-traceability.csv:70` records
`PERF-004` as `blocked`, evidence classification `mock`, disposition
`historical-quarantined`, and `agent/evidence/P6/VER-PERF-004.json` has been
reset (`"pass": false`, `"result": "historical"`). The only in-tree
`PERF-004` artefact is `src/__tests__/PerfDictationStartToListening.test.tsx`,
a jsdom test with a mocked bridge asserting the start-click → listening-visible
mark is under 100 ms in that harness. The earlier "0.014 ms backend"
measurement does not exist and is withdrawn.
