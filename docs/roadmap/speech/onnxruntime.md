# ONNX Runtime Strategy

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

For non-CPU execution providers:
- evaluate I/O Binding/device tensors to avoid hidden CPU↔device copies;
- reuse preallocated outputs when shapes are known and beneficial;
- consider shared/global thread pools across multiple sessions;
- consider shared allocators where multiple sessions inflate memory;
- measure execution-provider-specific concurrency and stream behavior;
- keep ORT types inside engine adapters.

Every optimization needs before/after profiling on representative hardware.
