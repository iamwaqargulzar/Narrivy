# History Screen Contract

## Job
Find and reuse committed dictations when retention is enabled without making History necessary for reliability.

Show timestamp, application-safe metadata where policy permits, mode and final text. Raw/corrected variants are available only according to retention policy and are treated as private content. Search is local by default. Clear/delete communicates scope and is immediately reflected in retention state.

When History is disabled, show that state and the distinction between history and temporary crash-recovery data. Do not silently turn retention back on to support another feature.

Tests: `HISTORY-RETENTION`, `HISTORY-DISABLED`, `HISTORY-DELETE`, `PRIV-HISTORY-NO-LOG`.
