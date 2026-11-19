# Diagnostics Contract

## Job
Answer “what is preventing Narrivy from working?” without exposing private content.

Capability checks are individually runnable:
- daemon/IPC;
- microphone/PipeWire;
- global trigger/PTT release semantics;
- text delivery per available backend/test target;
- model file integrity;
- model/device inference health;
- storage/config migration health;
- package/runtime dependencies.

Each row shows status, plain-language finding, last checked time, remediation, and expandable technical metadata/error code. A `Create support report` action previews categories included and excludes transcripts/audio/context/clipboard/secrets by default.

Do not include a giant raw log dump as the primary diagnostic experience.

Tests: `UI-DIAG-INDIVIDUAL`, `SEC-SUPPORT-REDACTION`, `UI-DIAG-REMEDIATION`, `E2E-DOCTOR-PARITY`.
