# ADR-0055: Complete-directory offline model import

Status: Accepted design for controlled Recovery 4Q-C5A (2026-09-02);
non-release, Linux-only, and not installation or distribution qualification

## Context

ADR-0051 can verify an already installed model, but the legacy acquisition path
publishes `model.gguf` before its metadata sidecar. A crash can therefore leave
an incomplete destination. The current Debian candidate deliberately contains
no model, and neither a network acquisition policy nor a licensed model-pack
distribution decision has been approved.

The smallest safe predecessor to installed-package testing is a local-only
transaction that can populate the existing registry-addressed cache without
giving a caller authority to invent model identity or silently repair a
divergent installation.

## Decision

Recovery 4Q-C5A adds `platform-linux::model_import::import_bundled_model`.
Linux path policy and mutation remain entirely in the Linux platform adapter.
The portable model manager exposes only `BundledModelIdentity`, canonical
metadata/digest verification, and `BundledInstalledModelBinding`. These opaque
types can originate only from `BundledModelRegistry`, the registry compiled
into the exact build. Caller-parsed `ModelRegistry`, caller-constructed
`RegistryEntry`, and generic `InstalledModelBinding` values cannot enter the
public import or installed-runtime binding APIs. No transport is invoked.

The Linux transaction:

1. opens a pre-existing dedicated absolute cache root through a no-symlink
   `openat2` walk and requires exact current-user ownership and mode 0700; it
   never chmods that root or any pre-existing ancestor;
2. opens one absolute, current-user-owned regular source file through a
   no-symlink descriptor walk and rejects group/world write bits, set-ID bits,
   or size mismatch;
3. creates and durability-syncs only missing, transaction-owned mode-0700
   descendants through retained parent descriptors, then creates a uniquely
   owned staging directory the same way;
4. creates only `model.gguf` and `meta.json` through the retained staging
   descriptor with exclusive/no-follow opens and exact mode 0600;
5. copies while checking the registry byte count and SHA-256, fsyncs each leaf,
   then rechecks the source descriptor identity and change metadata;
6. fsyncs the complete staging directory and publishes it with descriptor-
   relative `RENAME_NOREPLACE` into the registry-derived digest directory;
7. fsyncs both the source staging parent and destination parent, verifies the
   exact two-leaf result through retained descriptors, and returns only after
   the portable bundled installed-model verifier independently succeeds.

An exact existing installation is idempotent. An incomplete, unsafe, or
divergent destination is preserved and reported as a typed failure; it is
never overwritten. Concurrent importers converge on the one exact winner.
Failed-transaction cleanup removes only the two fixed leaves through the
retained staging descriptor and then its exact owned directory name. It never
recursively deletes a replaceable pathname. A deliberately raced nonempty
replacement cannot be removed and may remain for later reconciliation; this is
safer than traversing or forcing deletion of an identity no longer owned.

Import-specific failures disclose only a typed cause, not the raw caller model
string, source path, cache path, or content. The API exists only in the Linux
adapter; there is no portable or non-Linux check-then-rename fallback.

The bundled OpenAI Whisper-derived registry entries and matching non-Distil
Whisper entries in the transcribe.cpp catalog are corrected to `Apache-2.0`,
consistent with the independently reviewed upstream and conversion model
cards. Distil-Whisper and unrelated families retain their existing declarations.
Historical repository-wide notices are not rewritten by this narrow decision;
any remaining notice or redistribution inconsistency stays release-blocking.

## Trust semantics

A successful import proves only that one local source file matched the exact
compiled registry size and digest and was atomically published as a complete,
locally verified cache directory. It does not prove:

- source authenticity, signature, external provenance, or package membership;
- owner/legal approval to download, redistribute, bundle, or ship model bytes;
- SBOM or notice completeness;
- streaming capability, native transcription quality, latency, or hardware
  behavior;
- installation, upgrade, rollback, removal, or clean-machine behavior; or
- any evidence, requirement, package, manifest, or release transition.

The portable observation is still not the immutable runtime descriptor from
ADR-0050. Installed daemon composition must continue to bind and attest the
verified file independently before execution.

## Consequences

- A later isolated install test can provision an exact pre-authorized local
  model without using the incomplete legacy network acquisition transaction.
- The CLI remains a client and receives no model-manager ownership or bootstrap
  command in C5A.
- Network acquisition, resumable downloads, registry signing, model-pack
  construction, package lifecycle, and release licensing remain separate,
  blocked work.
- C5A performs no real-cache import, model download, package build/install,
  evidence creation, requirement change, manifest reseal, commit, or push.
