# Writing help everywhere: one OpenVINO worker, one language model

> **Status: direction set by the owner on 2026-09-30 (D26). Scheduled
> after the release (SQ-WRITE). Nothing is implemented yet.** Builds on D21 and D25 (OpenVINO is a Store add-on,
> Vulkan is the default), D23 (Refine, local language model first, light
> noise suppression) and ADR-0018 (the transformation guard).

## What the owner asked for

1. When the OpenVINO add-on is installed, **every model stage runs through
   it**: noise suppression, speech recognition (for example Whisper
   large-v3-turbo as OpenVINO IR) and the language model that corrects
   words.
2. A **writing assistant** as an open alternative to commercial grammar
   tools: a browser extension for **Chrome and Brave** that checks, fixes,
   improves and rewrites text, using a rule-based language checker and
   the language model.
3. **One language model for everything.** The model Narrivy already has
   loaded for dictation serves the extension too. It is never loaded a
   second time, so memory use stays small and predictable.

Goal: everything the user writes on the computer, spoken or typed, can go
through Narrivy.

## One OpenVINO worker

A single resident worker process, shipped with the OpenVINO add-on, holds
one OpenVINO runtime and every OpenVINO model. It follows the pattern of
the transcribe.cpp resident worker: the daemon starts and supervises it,
talks to it over a private pipe, and it has no network access.

| Stage | Typical model | Default device | Reason |
|---|---|---|---|
| Noise suppression | small streaming denoiser (10–20 ms frames) | CPU | Per-frame work is too small to be worth sending to a GPU or NPU |
| Speech recognition | Whisper large-v3-turbo, INT8 | GPU | Heavy matrix work |
| Language model | small instruction model (1.5B–3B), INT4 | NPU, else GPU | Short bursts, low power, keeps the GPU free for speech |

- The user can change the device for each stage. Each model, device and
  driver combination runs only after it passes a self-test (the D21
  rule). A failure falls back down the order and the reason is reported
  through status.
- Stages mostly run one after another (denoise while speaking, recognition
  when speech ends, correction after that), so they rarely compete for
  a device.
- Rough memory on the reference machine: about 3 GB for all three, out of
  32 GB shared.
- NPU compilation is cached as D21 describes. On Linux the NPU also needs
  Intel's user-space NPU driver. The add-on checks for it and reports
  when it is missing.
- Without the add-on, speech recognition stays on transcribe.cpp with
  Vulkan (D25). The language model runs on:
  - Vulkan by default;
  - CUDA (add-on) on NVIDIA;
  - Metal on macOS;
  - CPU as the last resort.

  A ggml-family runtime such as llama.cpp is the candidate for these
  paths. Every path passes the same self-test.

## One shared language model

- The daemon owns exactly one loaded language model. Dictation Refine and
  the browser extension send it requests. Neither loads its own copy.
- It stays loaded while Narrivy runs. Changing the model in Refine
  replaces it for everyone, with the load-new-then-drop-old switch that
  pipeline models already use.
- Requests take turns. Dictation always goes first. An extension request
  can be cancelled, and it is dropped when the text it was about has
  changed.
- Every result passes the transformation guard (ADR-0018) before it is
  offered. The original text is kept until the user accepts a change.

## Two kinds of checking

| | Rule checker | Language model |
|---|---|---|
| Job | Spelling, grammar and style underlines while typing | Fix, improve, rewrite, change tone, on request |
| Speed | Milliseconds, on CPU | About a second, on NPU or GPU |
| When it runs | Continuously on the text being edited | Only when the user asks |
| Needs the add-on | No | No: OpenVINO, Vulkan, CUDA, Metal or CPU |

Rule-checker candidate: **Harper** (Rust, runs locally, Apache-2.0,
English). LanguageTool covers more languages but needs Java; it is a
candidate for later languages. Either goes through the usual
candidate → verified → qualified path before becoming a default.

## The browser extension

- **Chrome and Brave**, both Chromium, one Manifest V3 extension. Firefox
  can follow; its native-messaging API is almost the same.
- **Connection: native messaging only.** The browser starts a small
  Narrivy host program and talks to it over standard input and output. The
  host then talks to the daemon over the existing same-user socket.
  Narrivy does **not** open a local web port, because any web page could
  try to reach one.
- The host's manifest lists only Narrivy's published extension ID. The
  package installs it in each supported browser's native-messaging folder.
- The daemon treats the host as a separate, limited kind of client. It
  may only check and rewrite text. It cannot start dictation, change
  settings or read History.
- **Privacy:**
  - The extension reads only the editable field in focus.
  - It never reads password, payment or one-time-code fields.
  - Each site can be switched off, and sensitive categories such as
    banking are off by default.
  - Text goes nowhere except the local host. It is never logged, and it
    is never stored in History unless the user asks.
  - The extension has no network permissions of its own.
- **Interface:**
  - underlines with suggestions while typing, from the rule checker;
  - a small action bar on selected text: Fix, Improve, Rewrite and Shorter;
  - a preview before anything is replaced;
  - undo afterwards.

## Later: outside the browser

The same engine can serve any application: select text anywhere, press a
shortcut, choose an action, and the result replaces the selection through
the existing delivery ladder. This is a separate step after the extension.

## Owner answers (2026-09-30)

1. **When:** after the release, as an added feature (SQ-WRITE).
2. **Where it runs:** everywhere. Intel uses OpenVINO, other machines use
   Vulkan by default or CUDA as an add-on, and macOS uses its hardware
   acceleration. It is a global feature on every platform Narrivy
   supports.
3. **Languages:** English by default. Other languages are the user's
   choice; which ones are offered is not decided yet.
