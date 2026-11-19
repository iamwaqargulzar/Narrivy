# narrivy-journal-windows

Secure Windows filesystem adapter for the recovery journal: the Windows
counterpart of `narrivy-journal-unix`, with the same public API shape
(`WindowsJournal`, `RetainedWindowsJournal`).

- **Role:** Adapter crate: OS-specific mechanics live here, behind the portable contracts. It owns no product policy.
- **Mechanics:** the journal directory gets a protected DACL (current user and
  SYSTEM only), the Windows analogue of mode 0700; every directory, entry and
  ancestor is checked for owner SID, writable-by-others ACEs and reparse
  points (symlinks, junctions). Entries are written to a temporary file,
  flushed, and moved into place with `MoveFileExW(MOVEFILE_WRITE_THROUGH)`
  (plus `MOVEFILE_REPLACE_EXISTING` for replacements), so a reader sees the
  old or the new entry, never a torn one.
- **Crate docs:** `cargo doc -p narrivy-journal-windows --no-deps --open --target x86_64-pc-windows-gnu`
- **Tests:** run on Windows: `cargo test -p narrivy-journal-windows --locked`
  (they compile, but do not run, in a Linux cross check).

See `docs/architecture/component-boundaries.md` for what this crate owns and
what it must not depend on.
