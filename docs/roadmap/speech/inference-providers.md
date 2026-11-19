# Inference Provider Architecture

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

## Separation of concerns

A text/audio processor defines **what transformation is requested**. An `inference-provider` defines **where/how model inference is executed**. Pipelines refer to provider capabilities rather than embedding provider-specific HTTP/auth logic in text processors.

Provider categories can include:

- local OpenVINO/ONNX/llama.cpp-style runtimes;
- local OpenAI-compatible servers;
- direct remote model APIs;
- routing/gateway providers.

## Authentication

Authentication is provider-specific and can be none, API key, OAuth/PKCE, device code, browser login or externally managed. Credentials are references to the secure secret store, never plaintext Pipeline/config values. Do not scrape or reuse another application's consumer subscription/session credentials unless that provider explicitly documents third-party use.

## Data permissions

Every provider declares which data classes it can receive. Pipeline policy grants a subset. Transcript, surrounding context, selected text, app identity and audio are distinct classes. A text postprocessor must not gain audio/context access merely because it has transcript permission.

## Routing

Advanced users may pin a provider/model. Auto may rank qualified providers by locality/privacy, latency, measured quality, cost policy, current availability and user preference. “Free tier” is volatile and must be discovered/verified rather than hard-coded as permanent product truth.

## Failure

Provider failure follows explicit Pipeline fallback. The normal dictation path must retain deterministic local correction and delivery even when every optional inference provider is unavailable.
