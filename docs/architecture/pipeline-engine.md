# Pipeline Engine

Narrivy executes **first-class Pipeline objects**: typed, versioned DAGs selected by triggers/profiles/policy. Modes are friendly built-in recipes over this mechanism, not a separate execution path.

Example Smart Local pipeline:

```text
Trigger → Capture → Denoise → VAD → ASR → Deterministic cleanup → Local LLM → Semantic guard → Delivery
```

A second Pipeline can omit the LLM entirely, use a different ASR or denoiser, or bind to another hotkey.

## Stage properties

Each node declares:
- capability and implementation selection (`auto`, `manual-preferred`, `manual-locked`);
- typed input/output and format/channel constraints;
- cancellation/statefulness and whether it may be skipped;
- local/cloud boundary and permissions;
- compute/runtime/model dependencies and device preference;
- estimated latency/lookahead/resource cost;
- bounded failure/fallback behavior;
- sensitivity of data handled.

## Planning and optimization

The planner validates graph compatibility before a session, resolves qualified implementations, warms required resources when policy permits, skips redundant stages, and can score a **whole graph** rather than independently choosing the fastest implementation for every node. This matters when ASR and a post-processing model contend for the same accelerator.

## Session consistency

Do not switch enhancement/ASR/post-processing implementations in the middle of an utterance by default. Runtime adaptation is applied between sessions unless a node contract explicitly proves safe hot reconfiguration.

See `pipeline-profiles.md`, `module-system.md`, and ADR-0016.

## Node multiplicity (stacking rules)

A Pipeline is a typed DAG of node *instances*; the schema does not limit how
many nodes share a capability. Multiplicity is a property of the capability
family, enforced by graph validation:

- **Chainable families** — may appear as multiple sequential instances, each a
  distinct node with its own selection/device/failure policy:
  `audio.normalize`, `audio.dsp`, `audio.aec`, `audio.dereverb`,
  `audio.beamform`, `audio.denoise`, `audio.separate`, `audio.target-speaker`,
  `audio.agc`, `text.punctuation`, `text.itn`, `text.spell`, `text.grammar`,
  `text.translate`, `text.llm-postprocess`, `text.semantic-guard`.
  Every adjacent pair is validated for typed I/O compatibility (stream kind,
  sample-rate/channel geometry, language/script) exactly like any other edge.
- **Singleton families** — exactly one instance per Pipeline:
  `audio.capture` (one stream in, one pre-roll owner), `speech.asr`
  (comparing multiple ASRs belongs to Shadow-Pipeline evaluation, FUN-124,
  not to a user graph), `speech.endpoint`, `context.provider`,
  `delivery.direct` (the delivery *router* is one node; its bounded backend
  chain lives in `fallback_modules`, not in extra nodes), `quality.qualifier`.
- **Guardrails for stacking**: identical module instances chained directly are
  rejected; the planner surfaces cumulative estimated latency/memory and warns
  when a stacked stage adds cost without qualification evidence of benefit;
  optional stacked stages that fail use their own failure policy (typically
  `skip`) without affecting the rest of the chain.
