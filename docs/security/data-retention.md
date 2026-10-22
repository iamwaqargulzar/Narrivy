# Data Retention

Classify:
- live audio buffers: ephemeral;
- crash-recovery journal: short TTL;
- history transcript: configurable;
- history audio: off by default unless product decision changes;
- learning data: persistent but separately clearable/exportable;
- diagnostics metrics: content-free;
- model/cache files: persistent technical artifacts.

User must be able to clear History without hunting through filesystem internals.
