# Foundation Hardening — Why P0–P1 Is Industry-Leading

Narrivy's P0–P1 foundation is hardened for the failures that break most dictation apps: lost sessions, torn journals, and silent schema drift.

**Formal state machine.** `SessionMachine` is table-driven via `is_valid_transition`. Tests exhaustively reject every illegal single-step pair and every 2-step illegal sequence (10×10×states), proving rejected transitions never move state or burn `EventSeq`. A 1 000-trial manual quickcheck fuzzes random walks, and a 100-burst hammer test proves recovery after illegal storms. Terminal states reset to `Idle`.

**Crash-safe journal.** Each transaction is a file-per-entry JSON written tmp→fsync→rename→fsync-dir. Torn writes quarantine as `.corrupt`, never crash, and `recoverable_entries()` returns only TTL-fresh, non-delivered entries sorted by time. The 100-entry power-loss proof writes 100 entries, truncates 15 mid-JSON and leaves stray `.tmp` files, then proves 85 survivors stay ordered and readable after restart.

**Hashed migrations.** SQLite WAL uses `schema_version`+`migrations` log; each step is one transaction with SHA-256. Re-apply verifies hashes and aborts with an actionable error. The v1→v4 forward-compat test snapshots user data, adds additive columns and a table, and proves prior rows and hashes survive.

**DoS-proof framing.** The 4-byte LE prefix is validated before allocation; oversized frames are rejected without OOM. Encode does one `4+payload` allocation; `from_slice` borrows without extra copy. `benches/framing.rs` and the p50<50µs test show ~50 ns for control messages. `is_request()` is covered for all 11 request kinds.

**Portable safety.** Portable crates forbid `unsafe_code`; `unsafe` lives only in `adapters/platform-linux` and the two scoped `security`/`server` sites with `SAFETY` comments. No `unwrap()` or `expect()` remains in production; all failures are typed.
