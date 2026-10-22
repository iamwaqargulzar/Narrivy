# Legal & Release Owner Blocker — Early Milestone (legacy planning Part O)

**Do not leave license and third-party-notice resolution until final qualification.** This is an explicit owner blocker/milestone **before** release candidate creation.

## Blocker B-LEGAL-007 — License & Third-Party Notices Review

- **Phase:** P0 (created) → rechecked at P10 pre-RC and P11 release_gate
- **Why agent cannot resolve:** Project license selection is a legal/business decision; third-party notices require human review of dependency/model licenses.
- **Human action required (before RC):**
  1. Confirm `LICENSE` content (currently MIT, Copyright (c) 2026 Narrivy) is final or provide SPDX alternative; see `LICENSE_POLICY.md`, `LICENSE-TODO.md`.
  2. Review `THIRD_PARTY_NOTICES.md` (generated from `cargo deny` + model licenses) for completeness, especially after adding Vulkan, ONNX Runtime EPs, and model artifacts.
  3. If signing is desired for the `.deb`, provide signing credentials (or confirm “unsigned for 1.0” as per `docs/maintainers/operations/update-policy.md` and B-SIGN-005).
  4. If distribution accounts are needed (GitHub Releases, apt repo, Hugging Face `narrivy` org), ensure they exist and tokens are available (see B-AUTH-006 for HF).
- **How to verify unblock:**
  ```bash
  cat LICENSE | wc -c          # >=200, no TODO/PLACEHOLDER
  cat THIRD_PARTY_NOTICES.md | wc -c  # >=80, no TODO
  python3 scripts/release_gate.py --target linux-1.0  # legal metadata part passes
  ```
- **Status:** **OPEN** — `LICENSE` (MIT 1064 chars) and `THIRD_PARTY_NOTICES.md` (1482 chars) currently pass the placeholder check, but human review is still required before RC. Full release_gate still fails on P1–P11 evidence (expected). This blocker is **non-blocking for engineering** — continue P1–P10 while awaiting owner review.

## Timeline

- **Before P10 RC:** owner reviews LICENSE/NOTICES and confirms signing/distribution account plan.
- **At P11:** `scripts/release_gate.py` re-verifies `LICENSE` and `THIRD_PARTY_NOTICES.md` are resolved (length + no TODO/PLACEHOLDER).

## Independent work

Continue all non-blocked engineering (P1–P10) while this awaits owner action. Do not disable the legal gate to make CI green.
