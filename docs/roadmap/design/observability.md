# Local Observability

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Narrivy needs internal performance/health observability even when external telemetry is disabled.

Maintain local counters/histograms for:
- latency stages;
- backend success/failure;
- fallback events;
- model load/cache hits;
- memory/worker lifecycle;
- session recovery;
- audio device changes.

Any optional product telemetry must be separately designed, consented and privacy-reviewed; it is not assumed by this architecture.
