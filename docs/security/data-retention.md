# Data Retention

Classify:
- live audio buffers: ephemeral;
- crash-recovery journal: short TTL;
- Inbox (recovered text, one private file per unconfirmed delivery in the
  runtime directory): the journal's 15-minute TTL, and cleared at logout;
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
