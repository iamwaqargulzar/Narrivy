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
- Pipelines — advertised pipelines. Versioned import/export is planned,
  not in 1.0: the buttons are shown disabled with a Planned label (F-H13). Each
  pipeline's Transcribe stage is where the model is chosen (from the
  downloaded ones) and where its options live. The Refine stage holds the
  deterministic rules and the clean-up model choice (D38; there is no
  Refine page).
- Models — the catalogue: what is downloaded and what can be downloaded.
  A model is not chosen here (owner direction, 2026-09-30).
- Store — add-ons for this computer: accelerator runtimes (OpenVINO first)
  and the models made for them, each with the service's "fits this
  computer" verdict, install progress and removal (D23, D25, ADR-0078).
- Assistant — chat with a language model on this computer, by typing or
  voice (D33).
- Meetings — record and transcribe a meeting file.
- Applications — per-application rules, scheduled after 1.0 and shown as such.
- Settings — every feature in one searchable map (`src/lib/featureMap.ts`)
  under top tabs; each row says whether it works now, is read from the
  service, is scheduled (with its completion-plan task) or is proposed.
- Diagnostics
- About (sidebar footer), with the version and update state beside it

There is no Dictation page (removed in rc12). Start, Stop and Cancel are on
Home; the mode and the dictation language are on Pipelines (Transcribe
stage) and in the tray.

Use a command/search palette to jump to settings, modes, actions and diagnostics without making every capability permanent navigation chrome.
