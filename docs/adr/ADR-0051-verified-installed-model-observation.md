# ADR-0051: Verified Installed-Model Observation

- Status: Accepted for controlled Recovery 4Q-C1
- Date: 2026-09-01

## Context

Recovery 4Q-B makes the resident worker consume immutable, digest-bound model
bytes, but the portable model manager previously called a model installed when
its derived path merely existed. Its live version-1 registry is narrower than
the normative model-manifest schema, acquisition did not enforce declared
size, registry-controlled path components were insufficiently constrained,
and the model file could become visible before its metadata sidecar.

Path existence cannot establish model identity, installation completeness,
package membership, or release provenance. Conversely, a portable model
manager cannot close Linux pathname/open races; that remains the responsibility
of the immutable descriptor binder accepted in ADR-0050.

## Decision

1. On-disk presence is reported only as `PresentUnverified`.
2. `verify_installed` is the only portable operation that produces an
   `InstalledModelBinding`. It requires a validated registry entry, regular
   non-symlink model and metadata leaves, exact declared size, exact SHA-256,
   exact identity metadata, one opened descriptor per leaf, bounded exact reads
   with EOF checks, and stable descriptor metadata across the observation.
3. The binding contains path, digest, size, model ID, family, and immutable
   source revision. Its debug representation omits the path.
4. The live registry rejects unknown fields, malformed digests, unsafe path
   components, incomplete identity/license/source metadata, unsupported sizes,
   and unpermitted source schemes before storage paths or transport are used.
5. Cache roots and existing descendants used for acquisition, observation,
   cleanup, or budget enforcement must be absolute, traversal-free, and
   symlink-free at the time of their fail-closed walk. Acquisition enforces
   exact byte count and digest, creates its metadata staging leaf without
   clobber, stages the sidecar before rename, and cleans a rejected payload.
6. Acquisition returns a verified observation rather than a runnable raw path.
   Prefetch may skip acquisition only after `verify_installed` succeeds and
   repairs a present-but-unverified artifact instead of trusting presence.
7. Linux service composition must still pass the observation to the ADR-0050
   immutable descriptor binder before runtime admission. A portable observation
   is neither an open descriptor nor protection from a same-user race.

## Truth boundary

This decision does not establish a normative distribution manifest, model
signature or license approval, installed-package or SBOM membership,
release-artifact identity, native behavior, or qualification evidence. The
live version-1 registry remains a compatibility catalog and is not promoted to
release authority.

The current acquisition sequence also does not yet meet the canonical complete
directory transaction: the verified model leaf can be published before the
metadata leaf. Because presence remains explicitly unverified, this cannot be
reported as a completed installation, but atomic completed-directory
publication remains blocked.

Portable pre/post component walks reject stable symlink redirection, and exact
descriptor reads prevent unbounded verification. They cannot make a sequence
of pathname operations atomic against a hostile process already running as the
same UID. Runtime admission therefore continues to require the Linux
open/no-follow immutable snapshot boundary in ADR-0050.

## Consequences and deferred work

- Existing callers must distinguish presence from verified identity.
- Corrupt, incomplete, oversized, symlinked, or metadata-divergent installs
  fail closed and are not runnable through this binding.
- Completed-directory atomic publication, install locks and references,
  directory-descriptor/no-follow mechanics where applicable, normative
  manifest composition, signatures, license policy, archive defense, and an
  installed runtime/package manifest are deferred to later bounded work.
- No evidence, requirement status, package, manifest seal, or release claim is
  created by this ADR.
