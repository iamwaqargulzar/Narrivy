# Threat Model

Assets:
- microphone audio;
- transcripts/history;
- selected/surrounding text;
- clipboard;
- project/repository context;
- API credentials;
- models/plugins;
- application control capabilities.

Threats:
- accidental cloud exfiltration;
- over-privileged Linux input/accessibility permissions;
- malicious plugin/model package;
- log/support-bundle leakage;
- prompt injection through surrounding text/context;
- prompt injection via transcribed speech content into optional LLM post-processing (spoken instructions attempting to redirect the corrector): mitigated by the semantic guard's protected spans, bounded edit budget/levels, deterministic-baseline fallback, and raw-transcript recoverability; never grant the post-processor delivery, network, or context permissions beyond its declared data class (ADR-0018);
- unsafe voice action interpretation;
- compromised remote inference endpoint;
- insecure IPC client impersonation;
- stale cache/model substitution.

Mitigations include least privilege, local-first defaults, capability checks, context labeling, plugin sandbox/permissions, safe typed actions, checksum/signature verification, redaction, local IPC authorization and privacy zones.

## Model download (ADR-0077)

The daemon uses the network only when the user asks it to download a model.
Threats and mitigations:

- a download used to reach arbitrary hosts: only `https` to Hugging Face
  hosts, every redirect checked before it is contacted, URL built from
  pinned catalog fields only;
- a substituted or corrupted model: accepted only when size and SHA-256
  match the catalog pins, otherwise deleted; the engine verifies the bytes
  again when it loads them;
- a hostile environment steering the download tool (proxy, configuration,
  credentials): the system `curl` is a root-owned, revalidated helper run
  with an empty environment and its configuration file disabled;
- a partial file swapped for a link or a shared file: partial files are
  private (0600), regular and replaced otherwise, in a directory nobody
  else can write;
- disclosure: requests carry no user data; reports carry only the model id,
  byte counts and reason codes.

## Store packs and the OpenVINO runtime (ADR-0078)

A runtime pack is native code, so it is held to more than a model:

- **Substituted libraries.** The catalog pins the runtime manifest's size
  and SHA-256, and the manifest pins every file. Each file is checked on
  install, into a private staging directory that becomes the pack only when
  complete. Manifest paths are plain names and links are refused.
- **Code in the daemon.** OpenVINO is loaded only by the separate, digest-
  bound `narrivy-openvino-worker`, never by the daemon. It is loaded by
  absolute path from the pack, so no library search path can redirect it.
- **A pack altered after install.** Status checks the pinned manifest and
  every file's size on each report. The worker re-verifies a model pack
  against the bound manifest before every load.
- **The developer source** (`NARRIVY_STORE_LOCAL_DIR`) is development-only
  configuration. Its files pass the same pins, so it cannot introduce
  anything the catalog does not name.

## Linux local-process boundary

The Unix endpoint rejects other OS users using kernel peer credentials and
strict endpoint ownership/modes. Within the endpoint owner's UID, command
classification, exact-operation lifecycle checks and daemon-global rate
budgets limit accidents, stale clients and resource abuse. Handshake roles,
PIDs and executable paths are not application identity. A malicious process
already executing as the same UID is therefore out of scope for per-process
confidentiality/authorization in Linux 1.0; stronger isolation requires a
separately protected principal or trusted broker, not additional role/path
checks. See ADR-0031 and ADR-0061.
