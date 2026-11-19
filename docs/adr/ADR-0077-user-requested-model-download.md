# ADR-0077: User-requested model download through the system download tool

Status: Accepted (2026-09-30; implements T2.17 items 2 and 3 for a daemon
started with an explicit model, under the locked strategy ADR-0017 / D21).

## Context

- The locked strategy delivers models from the Narrivy Hugging Face
  organisation at pinned revisions, verified by size and SHA-256, downloaded
  only when the user asks, with no telemetry. Until now the daemon had no
  network client (`NoNetworkTransport`).
- The owner asked for downloading to be properly implemented, choosing the
  model per pipeline (D25 context).
- A Rust HTTPS client would bring a TLS stack and its supply chain into the
  daemon, which holds the microphone, the transcripts and the delivery
  helpers. The daemon already runs reviewed system tools (PipeWire's
  recorder, `wtype`) as root-owned, revalidated helpers with a scrubbed
  environment.

## Decision

1. **The daemon has no network code.** A download runs the system `curl`,
   resolved from the system directories as a root-owned, revalidated helper,
   with an empty environment (no proxy, credential or configuration
   variable), its configuration file disabled (`-q`), `https` only,
   TLS 1.2 or newer, and no redirect followed on its own.
2. **What may be downloaded** is decided in `narrivy-model-manager`
   (`download.rs`): single-file catalog models published in the `narrivy`
   organisation at a pinned 40-hex revision with a pinned size and SHA-256.
   The URL is built from those fields only.
3. **Where the computer connects:** `huggingface.co` and Hugging Face
   content hosts (`*.huggingface.co`, `*.hf.co`). Redirects on
   `huggingface.co` are resolved one at a time with `HEAD` requests and each
   target is checked before it is contacted; a content host must serve the
   file itself.
4. **What is accepted:** the file grows as `<name>.part` (mode 0600) beside
   the loaded model, in a directory nobody else can write. It is renamed
   into place only after its size and SHA-256 match the pins; otherwise it
   is deleted. A stopped or interrupted download keeps its partial file and
   the next download resumes it; a server that will not resume starts it
   again once.
5. **Only on request.** `DownloadModel` (IPC minor 16) is the only path to
   the network. One download runs at a time on a supervised worker, so
   daemon shutdown stops it. A connection slower than 1 KiB/s for a minute
   is treated as stalled. Progress (bytes on disk) and content-free failure
   reasons are reported; no URL, path or header is.
6. The package depends on `curl`.

## Consequences

- The daemon's attack surface for downloads is the argument list it builds
  and the file it verifies; TLS, HTTP and DNS run in a separate, short-lived
  process with nothing inherited.
- The host rule limits where the computer connects. Integrity rests on the
  pinned SHA-256, not on the host.
- `User-Agent` is `narrivy`; nothing identifying the user or the computer is
  sent beyond what any HTTPS request carries.

## Not yet done

- Downloads in the packaged composition (its registry-bound installed model).
- Multi-file packs (the OpenVINO models) and runtime packs (T2.16, T2.19).
- Deleting a downloaded model from the Models page.
- Proxy support: a user behind a mandatory proxy cannot download yet.
