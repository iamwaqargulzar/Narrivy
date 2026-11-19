# ADR-0052: Installed runtime manifest binding

Status: Accepted design for controlled Recovery 4Q-C2 (2026-09-01);
non-release and not package qualification

## Context

ADR-0050 makes controlled worker and model bytes immutable after the daemon
binds them, and ADR-0051 adds a verified portable observation of an installed
model. Neither decision identifies an installed transcribe.cpp worker. A path,
an environment variable, or a digest written beside a binary by the same
unprivileged user is not an installed-runtime authority.

The process adapter distinguishes controlled-build bindings from generic
trust-anchor digest bindings. Promoting either a self-authored JSON file or a
generic trust-anchor observation to package provenance would conflate four different facts: a root-controlled local manifest,
installed package membership, model-pack identity, and externally verifiable
release provenance. These facts must remain separate.

The ASR engine may retain only the generic trust-anchor classification. It
must not relabel a generic process-adapter handle as manifest- or
registry-authorized. The daemon may publish the stronger local labels only at
the composition seam that consumes the Linux adapter's opaque
`InstalledRuntimeBinding` and `InstalledModelRuntimeBinding` together.

## Decision

Linux installed composition uses exactly these production paths:

- manifest: `/usr/share/narrivy/installed-runtime-manifest.json`;
- worker: `/usr/lib/narrivy/narrivy-transcribe-worker`.

Neither path is configurable through an environment variable, the manifest,
the user cache, or IPC. Test-only roots may be injected only into controlled
fixtures and must not alter production resolution.

The version-1 manifest is deliberately minimal. It contains only
`schema_version: 1`, `runtime_id: "transcribe.cpp"`, and a worker object with
the worker's lowercase SHA-256 and exact byte size. It cannot select a path,
model, engine, command-line argument, environment variable, package, or
release artifact. The normative shape is
`spec/schemas/installed-runtime-manifest.schema.json`; unknown fields and
non-canonical values are rejected.

The resolver must fail closed unless all of the following hold:

1. Every existing component of both fixed paths is observed without following
   symbolic links. The relevant directories and leaves are root-owned and not
   group- or world-writable.
2. The manifest is a bounded, root-owned regular file, is read exactly once
   from one descriptor, satisfies the version-1 schema, and has no trailing or
   alternate document.
3. The worker is a root-owned, non-set-ID, non-group/world-writable regular
   executable at the fixed worker path. Its exact size and SHA-256 match the
   manifest and it satisfies the existing native-ELF admission policy.
4. Descriptor identity and metadata remain stable across observation. The
   worker bytes are copied into the sealed immutable descriptor boundary from
   ADR-0050 before strict runtime admission; later path changes cannot change
   the admitted generation.
5. Any missing path, malformed or oversized input, unknown schema version,
   ownership or mode mismatch, symlink or special file, short/extra read,
   identity change, digest/size mismatch, or snapshot failure returns a typed,
   content-free unavailable state. It must not select a controlled-build
   worker, sidecar, mock, alternate runtime, or user-provided path.

Controlled development composition remains an explicit and separately named
mode. Its environment-selected artifact may produce only the controlled-build
provenance defined by ADR-0050. It cannot be upgraded to an installed state by
placing a manifest in a user-controlled test root.

The Linux implementation independently opens and validates a fresh descriptor
for the same fixed trust root when resolving the manifest and worker. Each
leaf is resolved beneath its descriptor with `openat2` using `RESOLVE_BENEATH`,
`RESOLVE_NO_SYMLINKS`, and `RESOLVE_NO_MAGICLINKS`. Reopening does not create a
configurable path or weaker pathname fallback; root or kernel compromise is
outside this local observation's threat model. Lack of the kernel primitive is
an unavailable outcome, not permission to fall back to pathname-only
resolution.

## Trust semantics

A successful resolution establishes only these local runtime facts:

- a root-controlled manifest at the fixed installed location declared one
  transcribe.cpp worker digest and size;
- the fixed worker's observed bytes matched that declaration; and
- the admitted worker generation consumes an immutable snapshot of those
  matching bytes.

This state must be named and reported as an installed-manifest binding, not as
installed-package or release provenance. Capabilities must expose the
distinction rather than collapsing it into a generic `installed`, `verified`,
or `release-ready` label.

Root ownership is a local administrative trust boundary, not an external
attestation. This resolver does not prove that dpkg installed or still owns the
files, that they came from a particular `.deb`, that a signed repository
authorized them, or that they are members of an SBOM or release provenance
statement. It also does not defend against a compromised root account or
kernel.

## Separate model identity

The runtime manifest does not contain a model path or model digest. A selected
model remains governed by the model catalog/registry and the
`InstalledModelBinding` contract in ADR-0051. Linux composition must then bind
that exact observed model through the immutable data-descriptor boundary in
ADR-0050. Worker installed-manifest state must never imply model installation,
model-pack membership, license approval, model suitability, or model release
provenance, and the inverse implication is equally forbidden.

The resident lane may become available only when the independently resolved
worker and selected-model bindings are both present, mutually compatible, and
successfully loaded. Their identities remain distinct in internal state and
content-free capability reporting.

Installed composition is an explicit process-start choice:
`NARRIVY_TRANSCRIBE_RUNTIME=installed` plus one bounded
`NARRIVY_TRANSCRIBE_MODEL_ID` from the bundled registry, with
`NARRIVY_ASR_ENGINE=transcribe-cpp`. In this mode raw worker or model path,
digest, and size environment variables are rejected. The cache
root is the standard `$XDG_CACHE_HOME/narrivy` location (or
`$HOME/.cache/narrivy` when XDG_CACHE_HOME is absent), and it must pass the
model manager's safe-root checks before the Linux descriptor binding. The
native worker handshake and model load must complete before IPC serving.

## Deferred package and release provenance

Package membership requires a later package adapter that independently
observes the system package database and binds its exact package name,
version, architecture, installed file membership, and integrity metadata to
the same worker bytes. Local package metadata alone is not a signature and is
not external release provenance.

Release provenance requires a separate, authenticated authority linking a
specific release artifact and its contents to source/build identity, SBOM,
signing policy, and immutable artifact digests. No field is reserved in the
version-1 installed runtime manifest to shortcut that work. Packaging the
manifest and worker, clean-machine installation, upgrade/rollback behavior,
package-database reconciliation, signature verification, and release
qualification are all deferred.

Until that authority is composed, status reports
`asr.transcribe-cpp.release-provenance=unavailable:not-composed` for missing,
controlled, and installed-manifest resident states. Neither an installed
suffix nor successful native startup substitutes for this explicit negative
fact.

## Consequences

- Installed runtime resolution is deterministic and has no path-selection
  surface.
- A manifest can bind the fixed worker's bytes without manufacturing package
  or release provenance.
- Installed strict readiness remains unavailable until the manifest and worker
  are actually shipped and the resolver succeeds; controlled recovery runs do
  not satisfy it.
- Errors and ordinary logs remain content-free and must not include manifest
  contents, filesystem metadata dumps, environment values, audio, or
  transcripts.
- This decision creates no evidence, requirement pass, package qualification,
  manifest reseal, or release claim.
