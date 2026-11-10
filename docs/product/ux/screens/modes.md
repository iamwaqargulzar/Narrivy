# Modes Screen Contract

## Job
Choose/edit declarative writing pipelines without requiring users to understand implementation stages.

Linux 1.0 prominently provides **Raw** and **Clean**. Additional 1.x modes are not presented as finished features until implemented.

Mode cards show name, short purpose, local/remote processing boundary, and active/default use. Editing uses human concepts first (formatting, vocabulary/context needs, output behavior) with an advanced pipeline inspector for power users.

Before enabling a remote stage, show what data fields leave the device and which provider/destination is used; privacy policy may forbid it. Invalid/imported pipeline schemas never execute.

Keyboard: card/list selection and edit actions are fully reachable; reordering has non-drag keyboard controls. Destructive deletion supports undo or confirmation according to reversibility.

Tests: `UI-MODE-RAW`, `UI-MODE-CLEAN`, `CONTRACT-MODE-SCHEMA`, `PRIV-MODE-REMOTE`, `A11Y-MODE-REORDER`.
