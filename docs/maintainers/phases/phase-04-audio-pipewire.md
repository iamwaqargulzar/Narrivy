# P4 — Audio & PipeWire Reliability

## Outcome
Deliver robust low-latency microphone capture and session audio policy with hotplug, stream churn and crash-safe restoration. Only the audio policies actually included in 1.0 are required.

## Primary requirements
`FUN-002`, `FUN-006`, `FUN-007` (scoped), `FUN-011`, `FUN-012`, `REL-004`, `REL-005`, `REL-009`, `PERF-006`.

## Linux Core 1.0 audio-policy scope (legacy planning Part C)
- `leave` must work.
- Add at least one independently proven, crash-safe useful policy such as `duck` or `pause` with restoration ownership and abrupt-failure tests.
- Other policies (`mute`, `smart`, remaining of the five) may ship only if they meet the same safety/recovery standard; otherwise move them post-1.0 (R12). Do not require all five for 1.0 merely because the old requirement listed them.

## Required work
1. PipeWire capture adapter + canonical bounded audio pipeline; no inference/disk/network work on real-time callback/thread (PERF-006).
2. Bounded pre-roll ring buffer; format conversion/resampling outside critical callback; optional level/VAD/endpoint signals must not clip first/last speech (FUN-006).
3. Deterministic microphone ranking, persisted user override, device enumeration, hotplug/fallback and clean termination when fallback is impossible (FUN-011).
4. `leave` + at least one of `duck`/`pause` with reversible per-session mutations; MPRIS/media control only when capability/policy says appropriate (FUN-007 scoped).
5. Observe PipeWire/desktop stream events so playback streams created during recording immediately receive the active policy without polling races (FUN-012).
6. Restoration uses normal RAII/cleanup plus independent minimal restoration journal/watchdog/recovery path for abrupt process death; never persistently rewrite master/app volume as shortcut (REL-005, REL-009). Restoration metadata is crash-safe.
7. Suspend/resume, PipeWire restart, stream churn, forced-kill and microphone unplug harnesses (REL-004).

## Exit evidence
Capture remains stable under representative inference load; pre-roll fixture passes; new stream inherits active policy; forced-kill restoration test passes where platform claims support; mic unplug is recoverable or cleanly terminates without corrupt state. Schema-valid evidence for all P4 verification IDs. `python3 scripts/phase_gate.py --phase P4` exits 0.

## Modular audio requirement

Capture, VAD, resampling/preprocessing and noise suppression are separate replaceable internal contracts. Ship a conservative built-in chain first; third-party external loading is not required for 1.0. A processor failure must bypass/degrade safely without crashing the capture owner or losing the session.
