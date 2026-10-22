# Screen Contract — Pipeline Editor (Advanced)

> Designed 2026-08-22 to close a real gap: the execution model (typed DAG,
> versioned pipelines, per-node contracts) was fully specified, but no editing
> experience existed. This contract defines how users compose, stack, and edit
> pipelines — including adding multiple nodes to one stage family — without
> exposing graph jargon to default users.

## Job

Let Advanced users create and edit saved Pipelines: pick a starting recipe,
swap the module behind any stage, insert/remove chainable stages (e.g. a
second enhancement node), set per-node selection/device/failure policy, bind
triggers, and export/import — with live validation that makes illegal graphs
impossible to save.

## Placement and disclosure

- Lives under **Settings → Automation → Pipelines** and from the Dictation
  screen's "Customize" action (progressive disclosure per UX-003). Default
  users normally see only Level 1.
- Nothing here is required for first successful dictation (UX-001).

## Three editing levels

**Level 1 — Recipe swap (ships P3).** A recipe picker (Fast Dictation, Clean
Mic, Smart Local, Cloud Polish, Custom) plus one row per stage: module
dropdown (qualified modules only), on/off toggle for optional stages,
hotkey binding. No graph is ever shown; language stays product-level
("Noise reduction: RNNoise ▾"). Covers the overwhelming majority of needs.

**Level 2 — Chain editor (ships P7).** An ordered stage list rendered from
the DAG's audio→speech→text→delivery spans. Users may:
- **Insert a stage** between any two adjacent nodes: the palette shows only
  modules whose typed I/O is compatible with both neighbors and allowed by
  pipeline policy (a cloud module never appears in a `local_only` pipeline).
- **Stack a family**: insert a second `audio.denoise` or `text.llm-postprocess`
  instance — permitted per the multiplicity rules
  (`docs/architecture/pipeline-engine.md` § Node multiplicity); identical
  consecutive modules are blocked; a latency/memory delta estimate appears on
  every insert and the planner warning ("+38 ms p95, no qualified benefit
  evidence") must be acknowledged before saving.
- **Reorder** within chainable spans (audio stages among audio, text among
  text); crossing a span boundary (e.g. moving VAD after ASR) is rejected
  with the reason.
- **Per-node inspector**: selection (Auto / Manual preferred / Manual locked),
  device preference chips, failure policy (fail/skip/fallback + fallback
  modules), config form generated from the module's config schema.
- **Singleton stages** (`audio.capture`, `speech.asr`, `speech.endpoint`,
  `delivery.direct`) render with a "one only" affordance: the ASR row swaps
  engines instead of adding nodes; the delivery row edits the ordered backend
  chain inside the router node.

**Level 3 — Free canvas (post-1.0).** Node-and-edge canvas for fan-out
graphs (parallel branches, shadow evaluation). The structured list of Level 2
remains the canonical accessible view; the canvas is an alternative
projection of the same validated graph, never a separate model.

## Live validation UX

- Per-edge compatibility checked as you build (stream kind, sample-rate/
  channel geometry, language); invalid insert options are simply not offered,
  with an explanatory "why not" list behind an info affordance.
- Policy badges on every node: **Local** / **LAN** / **Cloud**; a node's
  boundary can never exceed pipeline policy (PRIV-003 zones override both).
- Meters: cumulative estimated p95 latency vs. the pipeline's perf budget,
  memory/VRAM/NPU estimate, and model residency impact (Hot/Warm/Cold) —
  each insert/update updates them immediately; missing targets surface as
  warnings, never silently accepted.
- Save = validate → new **version**; running sessions keep the old version
  (frozen-graph rule); a diff view vs. the parent recipe/previous version is
  available. Import/export per UX-011 with a dependency preview listing
  missing modules/models/runtimes and download sizes before activation.

## States, keyboard, accessibility

- States: empty (no pipelines yet → recipe templates), editing, invalid
  (per-node reasons, never a bare "invalid"), saving/versioning, degraded
  (module unavailable on this machine → shown unqualified, never hidden),
  importing (dependency preview).
- Fully keyboard operable as a structured list; the canonical a11y
  representation is the ordered stage list with per-node expanders — screen
  readers announce "Stage 3 of 9: Noise reduction, RNNoise, optional, local,
  2.1 ms estimate". Validation errors surface in a live region.
- Reduced-motion: inserts/removes animate minimally or not at all.

## Privacy

The editor never touches transcript/audio content. Cloud-capable nodes
declare exactly which data classes would leave the machine
(`send_transcript_remote` / `send_context_remote` are separate, visible
toggles — PRIV-007). Export bundles exclude secrets and host caches (UX-011).

## Recovery

Edits are drafts until saved; leaving mid-edit preserves the draft. A
pipeline whose module becomes unavailable stays valid and editable; sessions
report the degraded node truthfully.

## Tests

`UI-PIPE-PICK-RECIPE`, `UI-PIPE-SWAP-MODULE`, `UI-PIPE-STACK-DENOISE`
(second instance legal, identical consecutive module blocked),
`UI-PIPE-SINGLETON-ASR` (no add option, swap only),
`UI-PIPE-INVALID-INSERT-HIDDEN` (cloud module absent under local_only),
`UI-PIPE-LATENCY-METER` (updates on insert; acknowledge required on
no-evidence stacking), `UI-PIPE-VERSION-BUMP` (running session keeps old
version), `UI-PIPE-EXPORT-DEPS` (missing dependency preview),
`A11Y-PIPE-LIST-KEYBOARD`, `E2E-PIPE-CUSTOM-RUNS` (edited pipeline dictates
end-to-end on a reference lane).

## Landing phases

- P1: schema/registry already supports instances (verified 2026-08-22:
  nodes+edges DAG with per-node capability); multiplicity rules documented in
  `pipeline-engine.md`; graph validator implements chainable/singleton
  enforcement.
- P3: Level 1 (recipe picker + module swap).
- P7: Level 2 (chain editor, stacking, per-node inspector, import/export) —
  same phase as saved-pipeline support (FUN-030/032/033, UX-011).
- Post-1.0: Level 3 canvas (only when a real graph shape needs it).
