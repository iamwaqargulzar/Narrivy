# Greenfield / Clean-Room Policy

Narrivy is an independent implementation, not a rebrand/source continuation of Handy, Handy-Pop-OS or any competing dictation product. Reference products may be used to learn public behavior, interoperability problems, usability expectations and public APIs/protocols. Do **not** copy or mechanically translate their source code, tests, comments, documentation prose, UI layouts, assets/icons, internal identifiers, prompts or implementation-specific structure.

The user's prior Handy-Pop-OS fork is a special case, amended by owner decision on 2026-08-22: the **owner-authored NPU work** (isolated OpenVINO NPU worker, Eddy/Parakeet integration, Qwen3-ASR conversion tooling, pinned model catalogue/runtime packaging, gate experiments — extracted to `code/handy-npu/`) is a confirmed code-salvage source under the same adapt-don't-transplant rules as `code/narrivy-v0.4/`. No code from the reference product itself is retained in this repository. Describe desired behavior in neutral product terms and implement it through Narrivy's architecture. Third-party libraries/models may be used only after provenance/license review and remain attributable under their licenses.

> Note (2026-09-29): the `code/` salvage folders referenced above are not in
> this repository. Salvage provenance is recorded in `agent/RESEARCH_LOG.md`.
