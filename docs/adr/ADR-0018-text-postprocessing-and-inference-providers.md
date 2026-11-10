# ADR-0018 — Text post-processing and inference providers

**Status:** Accepted  
**Date:** 2026-08-22

## Decision

Text post-processing is a chain of replaceable processors after ASR and before delivery. **Deterministic correction** and **generative/LLM processing** are separate capabilities.

The baseline path is deterministic and local. LLM processing is optional and may be local or remote according to Pipeline policy.

## Processing levels

- **Faithful cleanup:** punctuation, capitalization, obvious grammar/ASR repair while preserving meaning.
- **Polish:** readability/sentence improvement with a larger allowed edit budget.
- **Rewrite:** explicitly transformative behavior such as professional tone, concise rewrite, email formatting, bullets, etc.

The original raw transcript remains recoverable until delivery commits. Generative output passes through a transformation guard before delivery; protected tokens can include numbers, dates, currencies, URLs, email addresses, file paths, commands, code identifiers, quoted text, and dictionary-protected names.

## Provider abstraction

A `text-postprocessor` does not own provider credentials or transport. It asks an `inference-provider` capability for model execution. Provider implementations can represent local OpenVINO/ONNX/llama.cpp/OpenAI-compatible servers or remote APIs. Authentication is provider-specific; Narrivy must not assume a consumer subscription token from another application can legally or technically be reused.

Remote transcript and surrounding-context permissions are distinct. Cloud access is opt-in per Pipeline/Profile/Privacy Zone and clearly surfaced.

## Compute scheduling

A local post-processing model may share an accelerator with ASR if qualification proves it beneficial. The scheduler benchmarks whole plans such as ASR on NPU + postprocessor on NPU versus NPU + iGPU/CPU; no device assignment is hard-coded merely because both models can compile there.
