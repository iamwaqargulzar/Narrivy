# Fresh Pack Initial State

The distribution is a build **execution pack**, not a falsely pre-completed P0 repository.

On first extraction, these are expected work items rather than specification defects:

- `Cargo.lock` may be absent because this pack's creation environment did not contain Cargo; P0 must generate it with the pinned toolchain and commit it.
- `cargo-deny` may not be installed on the coding host; P0 installs/uses it according to policy.
- P0 traceability rows start `planned`, so the P0 phase gate intentionally cannot pass until the coding agent runs the checks and adds evidence.
- P1–P11 acceptance descriptors start `implemented:false`; each active phase replaces the placeholder with its actual tests/artifacts before passing.
- `LICENSE` is intentionally absent until the project owner selects a license. This blocks **public release**, not ordinary implementation.

These expected initial failures are described so an autonomous agent fixes them rather than treating them as an instruction to stop or weaken a gate.
