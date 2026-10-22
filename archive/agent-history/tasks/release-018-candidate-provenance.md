# Release task 018: exact candidate SBOM and provenance bundle

Implement the next real P11 blocker. Codex has researched and fixed the design
below. Work only in the listed files. Do not commit, push, access the web, use
subagents, modify tracked release evidence, promote requirements, build release
binaries/packages, install/start services, or run broad workspace/release gates.

Read:
- `AGENTS.md`
- `SOURCE_OF_TRUTH.md`
- `BUILD_TARGET.json`
- `agent/STATE.json`
- `docs/10-agent/token-efficient-workflow.md`
- `scripts/build_deb_installable.py`
- `scripts/generate_sbom.sh`
- `spec/schemas/release-artifacts.schema.json`
- `docs/08-operations/reproducible-builder.md`
- `scripts/README.md`

Allowed files:
- `scripts/build_candidate_supply_chain.py` (new)
- `tests/packaging/test_build_candidate_supply_chain.py` (new)
- `scripts/generate_sbom.sh`
- `scripts/README.md`
- `docs/08-operations/reproducible-builder.md`

Use the external-worker evidence contract in
`docs/10-agent/token-efficient-workflow.md`. Repository files and successful
tool results are the only evidence. Make the smallest correct diff. The new
implementation must use the Python standard library only.

## Problem

The retained clean rc5 package has a schema-2 embedded build identity, but the
only SBOM/provenance generator is a historical shell script that defaults to a
stub image/digest, writes directly over tracked historical artifacts, parses
Cargo.lock with regular expressions, omits a package subject from provenance,
and claims reproducibility. It cannot be used for a truthful current candidate.
External schema-3 release-artifact metadata, an exact package-bound SBOM and
package-bound provenance are still missing.

## Required behavior

1. Add a non-interactive Python CLI accepting explicit absolute paths for an
   existing candidate `.deb`, a source Git repository, and a new output
   directory; expected candidate SHA-256; and explicit nonempty builder OS,
   image, image-digest/status and toolchain strings. Accept no inferred proof
   flags. The output directory must not already exist and publishing must be
   atomic. Never overwrite tracked or untracked output.
2. Before creating output, verify with bounded argv-only calls and a scrubbed
   environment:
   - source root is an existing absolute directory and its exact Git toplevel;
   - candidate is an existing absolute regular file, not a symlink, has the
     expected digest, and is a Debian archive;
   - its exact regular, bounded, non-duplicate
     `usr/share/narrivy/build-info.json` member parses as schema 2,
     `clean-unqualified-release-candidate`, `git_dirty=false`, lowercase
     40-hex source commit, positive source epoch, and matching package version;
   - the source commit exists as a commit in the supplied repository;
   - `Cargo.lock` is read from that exact commit with bounded Git, not from the
     possibly newer working tree.
3. Read the `.deb` data tar stream without extracting it to the filesystem.
   Reject missing, duplicate, non-regular, oversized or unsafe build-info
   members. Do not use shell command strings.
4. Parse exact-commit Cargo.lock with `tomllib`. Generate deterministic,
   canonical JSON SPDX-2.3 containing Narrivy plus every exact unique lockfile
   package. Preserve registry/git source and checksum where supplied; never
   invent licenses. Use `NOASSERTION` where the lockfile cannot prove one.
5. Generate deterministic in-toto statement provenance with the exact package
   as `subject` (name plus SHA-256), and a predicate that binds version,
   source commit/epoch, Cargo.lock hash, embedded binary/worker/model identities,
   and the caller-supplied builder identity. Label it explicitly
   `unqualified-candidate-provenance`; do not claim reproducible, SLSA level,
   signing, native qualification, or release status.
6. Generate `release-artifacts.json` conforming to
   `spec/schemas/release-artifacts.schema.json` version 3. It must reference the
   exact existing candidate, generated SPDX file, and generated provenance by
   safe repository-relative paths with correct digests/sizes. Use
   `git_commit` from embedded identity, `git_dirty=false`, and truthful unsigned
   status. Reject any input/output path outside the repository because the
   schema manifest paths must be repository-relative.
7. Include a small `result.json` in the bundle with
   `classification=candidate-supply-chain-unqualified`, exact package/source
   identity and output file hashes. It is bookkeeping, not requirement
   evidence. Return the same concise summary on stdout. Emit no environment
   dump, user identity, secrets or package content.
8. Quarantine `scripts/generate_sbom.sh`: retain its path but make it exit 2
   immediately with a concise message directing callers to the new generator.
   Remove the old placeholder-producing body so it cannot be invoked by
   mistake.
9. Add focused hermetic tests for success/determinism and every fail-closed
   boundary above, including digest mismatch, symlink, wrong repo, missing and
   duplicate/nonregular/oversized build-info, dirty/default build identity,
   nonexistent source commit, malformed lockfile, existing output, path escape,
   command timeout/failure, schema-3 validation, exact artifact hashes, and no
   partial output on failure. Use temporary repositories/packages; do not rely
   on rc5 or network access.
10. Update only the relevant documentation paragraphs. State clearly that this
    produces an unqualified candidate bundle; it does not replace native
    qualification, signing, immutable-builder proof, or release-gate evidence.

## Focused checks

Run only:

```bash
python3 -m unittest tests.packaging.test_build_candidate_supply_chain -v
python3 -m py_compile scripts/build_candidate_supply_chain.py tests/packaging/test_build_candidate_supply_chain.py
bash scripts/generate_sbom.sh; test $? -eq 2
git diff --check -- scripts/build_candidate_supply_chain.py tests/packaging/test_build_candidate_supply_chain.py scripts/generate_sbom.sh scripts/README.md docs/08-operations/reproducible-builder.md
```

Finish with actual files changed, exact commands and outcomes, and unresolved
assumptions. Never report a check that did not run as passed.
