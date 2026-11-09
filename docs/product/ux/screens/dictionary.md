# Dictionary Contract

## Job
Teach Narrivy canonical words/names without requiring model retraining.

Entries have canonical form, optional spoken aliases/pronunciations, language and scope (global/app/mode/project where supported). UI makes scope visible in lists and conflict resolution predictable. Import/export uses validated versioned formats; invalid rows are reported without partially corrupting the dictionary.

Tests: `DICT-SCOPE`, `DICT-ALIAS`, `DICT-CONFLICT`, `DICT-IMPORT-ATOMIC`.
