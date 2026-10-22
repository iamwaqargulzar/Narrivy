# Internationalization and Localization Architecture

Speech language support and application UI localization are separate capabilities. Design both from P1 even if Linux 1.0 ships with a limited translated UI set.

## UI strings

- all user-facing strings use stable message IDs through one localization layer;
- no concatenated sentence fragments that break grammar;
- support plural/select formatting and parameter interpolation through the chosen i18n library;
- diagnostics/error codes are stable identifiers; localized user messages map from them;
- product/model names and copyable technical identifiers remain appropriately unlocalized.

## Locale behavior

- system locale is default; explicit user choice overrides it;
- predictable fallback chain ending in the source locale;
- locale-aware date/time/number formatting;
- UI language can differ from dictation language/model language;
- no assumption that language tags are two-letter codes; use standard BCP 47-style tags through the chosen library/contracts.

## Layout

Components must tolerate text expansion without fixed-width clipping. Use logical start/end layout concepts so future RTL localization is possible. Icons that imply direction require RTL review/mirroring policy; brand/media icons generally do not.

## Speech languages

Model manifests declare exact recognized language/task support independently of UI translations. Automatic language detection is an ASR capability; it must not silently change the application UI locale.

## Testing

P1 introduces a pseudo-locale or expansion test. P5 tests long strings/narrow windows. Before adding RTL localization, add an RTL visual/keyboard acceptance run. Missing translations fall back visibly/safely rather than rendering raw keys in release builds.
