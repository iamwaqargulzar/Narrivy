# Data Retention

Classify:
- live audio buffers: ephemeral;
- crash-recovery journal: short TTL;
- Inbox (recovered text): one private 0600 file per delivery that failed or
  whose outcome is uncertain, in the 0700 state directory
  (`$XDG_STATE_HOME/narrivy/inbox`), so it survives logout and reboot. Kept
  24 hours and at most 100 files (oldest removed first), pruned at daemon
  start and after each dictation. A delivery whose typing helper accepted the
  whole text (`wtype`, portal) leaves no Inbox copy (F-M3, F-M4);
- history transcript: configurable;
- history audio: off by default unless product decision changes;
- learning data: persistent but separately clearable/exportable;
- diagnostics metrics: content-free;
- model/cache files: persistent technical artifacts.

User must be able to clear History without hunting through filesystem internals.

## Process memory: core dumps and swap

While a dictation runs, audio, the transcript and (for paste) the clipboard
are held in the memory of `narrivyd` and its speech and clean-up workers.

- **Core dumps: contained.** The service unit sets `LimitCORE=0`, and
  `narrivyd` and each Narrivy worker make themselves non-dumpable
  (`PR_SET_DUMPABLE` 0) before anything else, so a crash writes no core
  file. This also stops other programs of the same user from attaching a
  debugger or reading those processes' memory, environment or open files
  through `/proc`. Helpers Narrivy runs but did not write (`pw-record`,
  `wtype`) are covered by `LimitCORE=0` only. (D37, F-H18.)
- **Swap: documented residual.** Narrivy does not lock its memory, so the
  system may page it out to swap, and hibernation writes all memory to disk.
  If that matters to you, use encrypted swap (or swap only to compressed
  RAM, zram) and do not hibernate.
