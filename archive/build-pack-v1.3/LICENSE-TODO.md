# License decision required before public distribution

The project owner must choose and approve Narrivy's source/binary distribution license. The coding agent **must not invent this legal/business decision**.

Until resolved:
- all current Rust packages are `publish = false`;
- do not publish crates or a public release;
- model/dependency licenses can still be inventoried and checked;
- all independent engineering work may continue.

For Linux 1.0 public release, place the approved text in root `LICENSE`, generate/update `THIRD_PARTY_NOTICES.md`, verify every bundled model/dependency redistribution condition, and rerun the release gate.
