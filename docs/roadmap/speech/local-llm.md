# Local and Remote Post-processing / LLM

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

LLM processing is optional and purpose-specific. The canonical design is in `text-postprocessing.md` and ADR-0018.

Use deterministic processors for punctuation, spacing, aliases, canonical numbers/dates, simple commands and target formatting. A small local model may be used for faithful cleanup when measured benefit justifies latency/memory. Larger local/remote models are for explicit Polish/Rewrite/Compose-style transforms.

Provider transport/auth is a separate `inference-provider` capability. Remote use must pass Pipeline/Profile/Privacy-Zone policy and visibly identify the destination. Do not feed more context than the Pipeline requests.

A local OpenVINO-capable postprocessor may coexist with OpenVINO ASR; qualification decides whether both use NPU or workloads are split across NPU/dGPU/iGPU/CPU.
