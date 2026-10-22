You are the implementation worker. Codex has already researched the boundary and will independently review and verify the result. The working tree contains intentional uncommitted recognition groundwork; preserve it.

Objective: complete the desktop recognition-settings slice for FUN-028/FUN-029. The daemon and native bridges already accept an optional typed RecognitionSettings payload, `src/lib/recognition.ts` already parses model-attested capabilities and builds the payload, and the recognition keys already exist in DEFAULT_SETTINGS.

Implement only this bounded behavior:
1. On the ASR settings tab, render a polished accessible recognition-settings card driven only by `recognitionOptions(status)`:
   - spoken language: Auto, Auto detect only when advertised, then advertised `asr.languages` values;
   - written/output language: Auto plus advertised languages;
   - output script: Auto plus advertised scripts;
   - translation target: Auto plus advertised translation targets;
   - locale: Auto plus advertised locales;
   - context hints: optional comma/newline text, enabled only when context bias is advertised.
   Persist through the existing `NarrivySettingsEnvelope`/`saveSettings` mechanism. When recognition settings are unavailable, show a clear status and disable explicit controls. Do not infer options from OS names or model names.
2. In `useDictationActions`, read the current daemon status and saved settings immediately before Start, call `buildRecognitionSettings`, and pass a non-null payload to `bridge.start`. When the builder returns null, retain the existing one-argument `bridge.start(pipelineId)` call so existing behavior and tests remain stable. Builder errors must flow through the existing error handler and prevent Start.
3. Add concise English i18n strings and only the CSS needed for the existing visual system. Avoid inline implementation/debug text.
4. Add focused tests proving: capability parsing/payload construction keeps spoken/output/script/translation/locale/context fields distinct; unsupported stale values fail closed; and Start passes explicit saved recognition settings while the all-Auto path keeps the one-argument call. Prefer focused pure tests plus one hook test; do not duplicate implementation details.

Allowed files:
- apps/narrivy-desktop/src/lib/recognition.ts
- apps/narrivy-desktop/src/app/components/SettingsCompute/RecognitionSettingsCard.tsx (new)
- apps/narrivy-desktop/src/app/components/SettingsCompute/SettingsCompute.css
- apps/narrivy-desktop/src/app/screens/Settings.tsx
- apps/narrivy-desktop/src/hooks/useDictationActions.ts
- apps/narrivy-desktop/src/i18n/en.json
- apps/narrivy-desktop/src/__tests__/RecognitionSettings.test.tsx (new)
You may make the minimum necessary adjustment to an existing frontend test only if the intended new Start argument requires it. Do not edit Rust, bridge contracts, settings.ts, unrelated UI, evidence/state, or package metadata. Do not commit or push.

Focused verification from apps/narrivy-desktop:
- `npm test -- --run src/__tests__/RecognitionSettings.test.tsx`
- `npm run build`
- If those pass, run `npm test` once to catch existing Start-contract regressions.
A command passes only with exit zero. Do not weaken assertions, use snapshots as the sole evidence, or claim native qualification. Stop immediately after checks pass; if a pre-existing failure blocks a check, identify it precisely without expanding scope.
