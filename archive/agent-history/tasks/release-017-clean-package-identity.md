# Release task 017: clean-source exact package identity

Implement one real P11 blocker. Codex has already researched the scope. Work only in the files listed below. Do not commit, push, access the web, build release binaries, build a real `.deb`, install/start services, modify release evidence, promote requirements, or run broad workspace/release gates.

Read:
- `AGENTS.md`
- `SOURCE_OF_TRUTH.md`
- `BUILD_TARGET.json`
- `agent/STATE.json`
- `docs/10-agent/token-efficient-workflow.md`
- `scripts/build_deb_installable.py`
- `tests/packaging/test_build_deb_installable.py`
- `scripts/README.md` packaging section
- `packaging/README.md`
- `packaging/deb/README.md`
- `docs/16-release/linux-1.0-qualification.md`
- `spec/schemas/release-artifacts.schema.json`

Use the external-worker evidence contract in `docs/10-agent/token-efficient-workflow.md`. Repository files and successful tool results are the only evidence. Make the smallest correct diff; use Python standard library only.

## Problem

`scripts/build_deb_installable.py` always embeds `candidate-build-info.json` with `classification=installable-unqualified-candidate`, `git_dirty=true`, and schema version 1. That mode remains useful for dirty diagnostic builds, but it cannot truthfully identify the clean exact package that P11 must qualify. External schema-v3 release provenance remains a later task and must not be fabricated here.

## Required behavior

1. Preserve the existing default unqualified-candidate behavior and CLI compatibility exactly.
2. Add an explicit CLI clean-source mode requiring a source repository root. Choose concise flag names and document them. Clean mode must fail closed before archive construction unless all of these are proven by bounded, argv-only Git calls:
   - source root is an existing absolute directory and the exact Git toplevel;
   - `HEAD` is a lowercase 40-hex commit equal to `--source-head`;
   - tracked, untracked, staged, and unstaged state is empty using porcelain output;
   - Git commands complete within a small fixed timeout and use a scrubbed deterministic environment.
3. Do not accept a caller-supplied boolean as proof of cleanliness. Direct library calls requesting clean mode must perform the same validation.
4. In clean mode only, embed canonical JSON at `usr/share/narrivy/build-info.json` with:
   - `schema_version: 2`;
   - `classification: clean-unqualified-release-candidate`;
   - `git_dirty: false`;
   - exact `source_head`, `source_date_epoch`, package version, and the existing binary/worker/model input hashes and sizes.
5. Default mode must continue embedding `usr/share/narrivy/candidate-build-info.json` with its current schema/classification/dirty truth. The two paths must never coexist in one package.
6. The returned summary must carry the matching classification and `git_dirty` boolean. Do not label either mode release-qualified and do not create schema-v3 release artifacts or provenance.
7. Add focused tests covering clean success, dirty tracked state, untracked state, HEAD mismatch, non-repository/wrong toplevel, Git timeout/failure, direct-library validation, exact clean/default build-info payload/path, and unchanged default behavior. Prefer testing extracted pure helpers plus temporary real Git repositories; do not mock a cleanliness pass.
8. Update only the relevant paragraphs in `scripts/README.md`, `packaging/README.md`, and `packaging/deb/README.md`. State that clean mode creates an exact unqualified release candidate; native qualification and external schema-v3 provenance remain required.

## Focused checks

Run only:

```bash
python3 -m unittest tests.packaging.test_build_deb_installable -v
python3 -m py_compile scripts/build_deb_installable.py tests/packaging/test_build_deb_installable.py
git diff --check -- scripts/build_deb_installable.py tests/packaging/test_build_deb_installable.py scripts/README.md packaging/README.md packaging/deb/README.md
```

Finish with actual files changed, exact commands and outcomes, and unresolved assumptions. Never report a check that did not run as passed.
