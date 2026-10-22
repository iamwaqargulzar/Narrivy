# Information Architecture

## Linux 1.0 main navigation

The shell follows the owner-approved design in
`spec/ui/reference-images/narrivy-home-dark-approved.png`: a top bar (brand,
Local · Private, live status, search, Light/Dark switch), a sidebar and a
live status strip along the bottom.

Sidebar destinations:

- Home — readiness and Start dictation, the active pipeline as five stages
  (Capture, Clean, Transcribe, Refine, Deliver), the recent transcript with
  Copy/Edit/Paste again, the privacy note, tips, and runtime metrics. Metrics
  appear only when the daemon reports them.
- History
- Pipelines — advertised pipelines and versioned import/export. Each
  pipeline's Transcribe stage is where the model is chosen (from the
  downloaded ones) and where its options live.
- Models — the catalogue: what is downloaded and what can be downloaded.
  A model is not chosen here (owner direction, 2026-09-30).
- Store — add-ons for this computer: accelerator runtimes (OpenVINO first)
  and the models made for them, each with the service's "fits this
  computer" verdict, install progress and removal (D23, D25, ADR-0078).
- Refine — the deterministic rules and the optional local language model
  (owner decision D23).
- Dictation — the session, modes, how dictation is started, recognition.
- Applications — per-application rules, scheduled after 1.0 and shown as such.
- Settings — every feature in one searchable map (`src/lib/featureMap.ts`)
  under top tabs; each row says whether it works now, is read from the
  service, is scheduled (with its completion-plan task) or is proposed.
- Diagnostics
- About (sidebar footer), with the version and update state beside it

Use a command/search palette to jump to settings, modes, actions and diagnostics without making every capability permanent navigation chrome.
