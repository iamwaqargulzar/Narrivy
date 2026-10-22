# Audio Policy Restoration & Stream-Churn Architecture

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Audio policy changes are **session-scoped reversible mutations**, not persistent mixer preferences. The Linux adapter records only the minimum restoration metadata needed to restore the prior observable state. Normal cleanup uses owned guards/RAII; an independent recovery record/watchdog or startup reconciliation covers abrupt process death where reliable restoration APIs exist.

The adapter subscribes to playback-stream lifecycle events while policy is active. A new eligible playback stream must receive the current duck/pause/mute policy immediately and be included in restoration ownership. Removed/recreated streams must not leave stale restore operations that alter unrelated streams.

Never implement duck/mute by persistently changing master volume or rewriting long-lived application preferences. Forced-kill, PipeWire restart, stream creation/removal and concurrent session-policy tests belong in P4/P9. When a platform cannot guarantee restoration, expose the limitation and prefer the least persistent policy.
