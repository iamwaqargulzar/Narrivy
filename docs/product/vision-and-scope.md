# Narrivy — Vision and Scope

This document merges the former `PRODUCT_CHARTER.md` and `PRODUCT_VISION.md`.
It is normative for product scope (see `SOURCE_OF_TRUTH.md`).

## Product

Narrivy is a premium, local-first voice interaction layer. Its first product is dependable universal voice typing on Linux. The architecture later expands into context-aware writing/editing, controlled actions, meetings/media, accessibility workflows, companion devices, extensions, and native Windows/macOS clients without rewriting the portable core.

## Linux 1.0 launch focus

- First-class: Pop!_OS 24.04+ with COSMIC/Wayland.
- First post-release lane: Ubuntu 24.04+ with GNOME/Wayland, qualified after the Pop!_OS release is properly tested (owner decision 2026-09-28).
- XWayland application compatibility where safe/practical.
- Linux expansion after 1.0 stability: KDE Plasma, wlroots desktops, Fedora/Arch/Debian-derived distributions.
- Architecture remains portable to Windows/macOS; native product work is post-Linux-1.0, in the order: other Linux distributions, then Windows, then macOS.

## Product pillars

1. **Reliability** — press/hold a shortcut, speak, text reaches the intended place or is recoverable.
2. **Perceived speed** — immediate listening feedback and aggressively measured end-to-end latency.
3. **Accuracy** — model-agnostic ASR plus safe context, vocabulary and correction layers.
4. **Privacy** — local-first defaults, explicit processing boundaries, minimal retention, content-free diagnostics.
5. **Premium UX** — simple for nontechnical users, progressive disclosure for power users, accessible and reversible.
6. **Portability** — OS/framework/model details live at adapters/engines, not in the domain core.
7. **Extensibility** — versioned contracts first; public ecosystem only after security/API maturity.

## Non-goals for Linux 1.0

- supporting every ASR model/runtime;
- full hands-free OS replacement;
- meetings/diarization as a release blocker;
- public plugin marketplace/team administration;
- Windows/macOS feature parity;
- adding cloud dependencies to make the local path work.

## North-star experience

Install Narrivy, grant understandable permissions, complete a short microphone/shortcut/model test, then dictate into ordinary applications without learning ASR or Linux-input terminology. When something cannot work, Narrivy explains the limitation, preserves the user's text, and offers a concrete remediation or safe fallback.

## v1.3 product principle — Simple by default, composable underneath

A first-time user should not need to understand runtimes, models, graph nodes or providers. Narrivy ships a lean dependable default Pipeline. Power users may replace compatible modules, create Pipelines with independent triggers, and import/export configuration without weakening reliability, privacy transparency or recovery guarantees.

## Vision

### One-line position

**Narrivy is the local-first intelligent voice layer for your computer.**

### Initial market wedge

Linux users who want reliable, polished, universal dictation — especially developers, AI-agent users, multilingual users, privacy-conscious users, and people who benefit from reduced keyboard use.

### Long-term category

Narrivy is not limited to speech-to-text. It should provide a controlled voice interaction layer that can:
- dictate;
- compose;
- edit;
- transform selected text;
- control safe application actions;
- capture meetings and system audio;
- learn vocabulary/style/context locally;
- drive developer and agent workflows;
- expose SDK/plugin APIs.

### Brand experience

The product should be calm, precise, fast, respectful of user attention, and transparent about privacy. Complexity belongs behind progressive disclosure.
