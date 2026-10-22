# code/ — Salvage Code Archive

Reference-only source archive for the v1.3 Narrivy build. Nothing in this
folder is part of the shipping product, the workspace, or the pack manifest;
material enters the product only through the phase that implements it, adapted
to v1.3 contracts. Mapping and rules: `archive/docs/management/salvage-inventory.md`.

## Contents

### narrivy-v0.4/ — previous Narrivy implementation (archived 2026-08-22)
- Cleaned snapshot of the v0.4 attempt (commit `5168619`): 17 GB of build
  artifacts, `node_modules`, dist, stale release binaries, and the git history
  removed; source, docs, tests, spec, and small evidence retained.
- Narrivy's own code, MIT. Free to salvage with adaptation. What maps where:
  `archive/docs/management/salvage-inventory.md`.

### handy-npu/ — owner-authored NPU work (extracted 2026-08-22)
- Owner-authored OpenVINO NPU material extracted from the owner's public
  Handy-Pop-OS fork (NPU commit range `016b175..c366fbb`, 2026-08-13). Per
  owner decision, only owner-authored files are retained; the fork tree and
  its git history were removed — the public fork remains the authorship
  record.
- Contents: isolated OpenVINO NPU worker (C++) with the vendored Apache-2.0
  Eddy Parakeet decoder (its LICENSE stays — a functional third-party
  library), the Rust worker-supervision manager, design/gate docs, gate
  experiments, and the cumulative NPU work patch.
- Verified facts and recipes from this work are recorded in
  `agent/RESEARCH_LOG.md` (2026-08-22 entries) and
  `archive/docs/references/upstream-version-audit-2026-08-22.md`.
- See `handy-npu/README.md` for the salvage-eligibility boundary (owner files
  vs. the cumulative patch touching pre-existing code).
