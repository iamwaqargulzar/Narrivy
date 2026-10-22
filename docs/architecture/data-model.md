# Core Data Model

Key entities:
- `SessionId`
- `DictationSession`
- `SessionState`
- `TargetContext`
- `ApplicationIdentity`
- `AudioSourceId`
- `TranscriptSegment`
- `TranscriptVariant` (raw/corrected/delivered)
- `ConfidenceSpan`
- `ModeId` / `ModeDefinition`
- `ProfileId` / `ProfileRule`
- `ContextItem`
- `PrivacyDecision`
- `ComputeDevice`
- `EngineCapabilities`
- `ModelManifest`
- `DeliveryAttempt`
- `CapabilityHealth`
- `VocabularyEntry`
- `SnippetDefinition`

Prefer opaque IDs/newtypes instead of loosely related strings.
