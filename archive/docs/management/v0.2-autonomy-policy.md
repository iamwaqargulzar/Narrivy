# v0.3 Autonomy / Scope Governance

## Why the build is phased this way

Earlier phases can otherwise accumulate architecture without proving a user-facing product. v0.3 establishes a mock vertical slice in P1, a real client shell in P2, and requires a **real working Linux dictation alpha in P6**. Every later Linux 1.0 phase must preserve that E2E path.

## Scope ceiling for autonomous Linux 1.0

The agent may refactor/add implementation needed to satisfy release-gated requirements. It must not delay 1.0 by autonomously promoting roadmap items such as meetings, hands-free command overlays, full plugin runtime or Windows/macOS product work.

If a roadmap capability is necessary to solve a 1.0 architectural issue, implement only the minimal reusable seam and record the decision; do not build the roadmap product surface.

## Requirement changes

A coding obstacle is not permission to delete a requirement. If a requirement proves impossible/unsafe on the selected 1.0 stack:

1. gather technical evidence;
2. attempt documented safe alternatives/fallbacks;
3. record an ADR explaining incompatibility/tradeoffs;
4. change the requirement/traceability only if the product behavior can be preserved or a genuine scope decision is required;
5. legal/business/product-scope decisions that materially reduce the promised product remain human decisions.

This prevents “autonomy” from becoming silent scope erosion.
