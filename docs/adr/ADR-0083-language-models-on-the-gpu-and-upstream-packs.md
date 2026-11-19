# ADR-0083: Language models run on the GPU; some packs download from their upstream publisher

Status: Accepted (2026-11-16). Records D58. Amends ADR-0017's download
rules for language-model packs only.

## Context

- The owner decided that no language model runs on the NPU (D58): on the
  288V the NPU builds capped prompts at 2,048 tokens and degraded on long
  prompts, while the GPU reads a 7,500-token transcript in about 5 s.
- The owner wants several assistant models to choose from. Gemma 4 E4B and
  Qwen3.5 9B were tested on the GPU (correct answers, good Urdu) in the
  OpenVINO organisation's own INT4 exports.
- Every pack so far came from `narrivy/store`, which the owner uploads by
  hand. D57 asks that every offered model work from the installed package,
  without waiting for an upload.

## Decision

- The language-model worker tries the GPU, then the CPU; a request for the
  NPU loads nothing. Clean-up and the assistant no longer offer the NPU.
  The NPU-only builds (`qwen3.5-4b-npu`, `s1-mini-npu`,
  `speakoflow-mini-npu`) leave the catalog, also when an older live catalog
  still lists them. An installed one stays listed (so it can be removed)
  but can do nothing.
- A language-model pack may be downloaded straight from an allowed upstream
  organisation (`OpenVINO` only) when all of these hold: the whole
  repository at a pinned 40-hex commit, a plain repository name, and a
  manifest built into Narrivy (`crates/narrivy-model-manager/seed/upstream/`,
  made by `scripts/build_llm_pack.py --upstream-seed` from the downloaded
  files and checked against Hugging Face's own hashes). The catalog pins
  that manifest's size and SHA-256; the manifest pins every file. The
  manifest and the prompt profile (`narrivy-refine.json`) are written from
  the daemon itself and checked like downloaded files. Hosts stay Hugging
  Face's only, as before.
- First upstream packs: `gemma-4-e4b-openvino` (recommended for the
  assistant) and `qwen3.5-9b-openvino`, both Apache-2.0.
- Gemma 4 marks its reasoning with special tokens that decoding drops, so
  its reasoning cannot be told from its answer: for Gemma the worker keeps
  the template's thinking off and the daemon's "think it through"
  instruction asks for it within the answer.

## Consequences

- Nothing about trust changes: no byte is used until it matches a pin that
  ships inside Narrivy. What changes is who serves the bytes (OpenVINO's
  repository instead of a copy in `narrivy/store`).
- A new upstream model needs a Narrivy release (its manifest is built in);
  the live catalog cannot add one.
- If OpenVINO deletes the pinned commit, the download fails with an honest
  error; the fix is a copy in `narrivy/store` and a catalog update.
- Long transcripts: the worker frame grows to 512 KiB, the assistant's
  prompt budget to 200 KiB, a finished meeting's context to 96,000
  characters, and meeting notes condense 48,000 characters per step.
