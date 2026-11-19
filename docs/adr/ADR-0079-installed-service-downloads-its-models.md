# ADR-0079: The installed service downloads and chooses its models

Status: Accepted (owner decision D40, 2026-10-31; D32 stands)

## Context

D32 says no model ships with Narrivy. The rc13 package still shipped
`whisper-tiny.en-Q8_0`. The installed composition could not get any other model:
`switchable()` was false, so it could neither download nor choose one, and the
packaged unit allowed only `AF_UNIX`, so the downloader could not connect.

## Decision

- The package ships no model. A clean install starts in the existing
  `awaiting_model` state, which ADR-0056 already defines.
- The installed composition can switch models. Its models directory is
  `$XDG_CACHE_HOME/narrivy/models`, private to the user.
- A chosen model runs on the package's verified worker. The model file is bound
  by its catalog SHA-256 and size when it loads.
- The model chosen last loads again at start. On failure the service stays as it
  was, and reports that state.
- `NARRIVY_TRANSCRIBE_MODEL_ID` stays. A model an earlier package imported into
  the cache keeps working after an upgrade.

## Consequences

- **The service can reach the network.** The unit allows `AF_INET`, `AF_INET6`
  and `AF_NETLINK`, because `curl` is the service's child and inherits the
  restriction. The address-family limit no longer proves that the service stays
  offline, so PRIV-004 and PRIV-007 need their own measurement. Only a client
  request starts a download.
- A clean install needs internet before the first dictation.
- Qualification uses the downloaded model, pinned in the qualifiers.
- The offline import path (ADR-0055, ADR-0059, ADR-0060) stays in the service.
  The lifecycle qualifier uses it, with a file it copies into the container. The
  app no longer offers it. `model_install_bundled` and its fixed source path in
  `bridge.rs` are unused, and can go once nothing depends on them.
