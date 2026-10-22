# Confidence and Repair

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Use the cheapest reliable correction path:
1. ASR hotword/context biasing.
2. deterministic dictionary/normalization.
3. low-confidence span resolver using local context.
4. stronger second-pass ASR if justified.
5. LLM rewrite only when the Mode needs semantic transformation.

Preserve raw transcript so repair can always be inspected/reverted.
