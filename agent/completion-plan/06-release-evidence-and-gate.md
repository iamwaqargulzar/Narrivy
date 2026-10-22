# WP6 — Release evidence files and the final gate

**Goal:** `python3 scripts/phase_gate.py --phase P11` and
`python3 scripts/release_gate.py --target linux-1.0` both exit 0 at
`FREEZE_COMMIT`. Then the evidence is preserved in git.

Still frozen: no tracked source, script or profile file may change. Generator
scripts needed here must be written and committed **before** the freeze (do
that as part of WP4). Running them now only writes evidence outputs.

Read `scripts/release_gate.py` steps 3–7 before starting; each file below is
checked there.

**Do not reuse anything under `artifacts/quarantine-2026-09-27/`.** Those are
earlier release files quarantined as fabricated. The README there explains
why.

---

## T6.0 — Harden the gates before the freeze (P-5) — do this in WP4, not after the freeze

These are gate-script changes, so they must be committed **before** WP5
freezes. They're listed here because WP6 depends on them.

1. `scripts/release_gate.py`:
   - check live cleanliness itself (`git status --porcelain`, allowing only
     the evidence and release-evidence outputs); don't trust `git_dirty` in
     `release-artifacts.json` (line 192);
   - bind `cpu_fallback_pass`, `accessibility_pass` and
     `privacy_security_pass` (line 259) to specific verification IDs and
     re-check that evidence, instead of reading bare booleans;
   - make the P11 JSON report mandatory (lines 168-180);
   - tie `summary.md` to the commit.
2. `scripts/phase_gate.py`:
   - no Markdown fallback (line 471);
   - count `native_seen` only after a full `verify_evidence(...,
     native_required=True)` passes (line 409);
   - parse blocker status structurally (line 205); the `[OPEN]` P11 blocker
     at `agent/BLOCKERS.md:60-66` is currently ignored;
   - put timeouts and process-group cleanup on every command (line 217).
3. `scripts/validate_pack.py`:
   - run `verify_architecture_map.py` (line 292);
   - make the "implemented phase without commands" check an error
     (line 252).
4. `scripts/evidence_policy.py`:
   - reject NaN and Infinity (line 100);
   - validate `started_at`/`ended_at` and their order in `assess_evidence`
     (line 429).
5. Smaller hardening, if cheap:
   - `build_manifest.py`/`verify_manifest.py`: use `lstat`, record modes,
     and verify against the git index;
   - `check_log_redaction.py:74,87`: accept only qualified Narrivy redactor
     calls, and mask only `#[cfg(test)]` modules;
   - `verify-boundaries.py:153`: use Cargo target `src_path`;
   - `boundary_policy.py:118`: add process-spawning patterns;
   - `amendment_gate.py:199`: check the RESOLVED marker per blocker.
6. Each change needs a negative test in `tests/governance`. Then run
   `python3 -m pytest -q tests`.

## T6.1 — `artifacts/release-evidence/linux-1.0/release-artifacts.json`

- Must validate `spec/schemas/release-artifacts.schema.json`.
- `git_commit` = `FREEZE_COMMIT`, `git_dirty` = `false`.
- `artifacts[]` must include kinds `deb`, `sbom` and `provenance`, each with a
  path that exists, an exact `sha256` and `size_bytes`. `sbom_path` and
  `provenance_path` must point at those entries.
- `signing` must be truthful. Signing was removed by the owner (B-SIGN-005), so
  use `signed: false` with a non-`signed` status.
- **Source:** the WP3 supply-chain bundle for the frozen package
  (`scripts/build_candidate_supply_chain.py` output). Copy the SBOM
  (`spdx.json`) and provenance into the tracked `artifacts/sbom/`, so they
  survive in git. The `.deb` stays at its ignored path; the owner keeps an
  archival copy.
- Write this file with a small reviewed generator committed before the freeze
  (for example `scripts/generate_release_artifacts.py`). Never type hashes by
  hand.

## T6.2 — TEST-010 evidence and `qualification.json`

1. Record **TEST-010** with its WP4 procedure. It is a composite: it checks
   that the COSMIC end-to-end evidence and the CPU performance evidence
   exist, are `passed`, and name the same `package_sha256`. Promote it with
   `mark_requirement.py`.
2. Generate `artifacts/release-evidence/linux-1.0/qualification.json` (again
   with a pre-freeze generator, for example
   `scripts/generate_qualification.py`). It must validate
   `spec/schemas/linux-qualification.schema.json` and satisfy the gate:
   - `environments[]` has one passing `cosmic-wayland` entry (the only lane
     `release_gate.py` requires since 2026-09-28): `native_or_vm: native`,
     `session: wayland`. Its `evidence` points to the end-to-end record (validated with
     `expected_lane`), and `package_sha256` is the frozen `.deb` hash. Fill
     `os`, `os_version`, `kernel`, `desktop`, `cpu` and `microphone` from
     values the recorders measured on that machine.
   - `cpu_fallback_pass`, `accessibility_pass` and `privacy_security_pass` may
     be `true` **only** if the matching requirements are `passed`. The
     generator must derive them; never hard-code them.
   - `accelerator_claims`: leave it empty unless an accelerator was actually
     qualified. The 1.0 default is the CPU path.
   - `target_matrix`: one row per class (all 8), from the T4.3 matrix
     evidence.
   - `performance`: `pass`, `protocol:
     "docs/maintainers/qualification/performance-benchmark-protocol.md"`, the `evidence` path
     of the CPU performance record, and `metrics` with p50/p95.

## T6.3 — `summary.md` and release notes (OPS-005)

- Generate `artifacts/release-evidence/linux-1.0/summary.md` with the WP4
  generator (`scripts/generate_release_notes.py`). It builds the text **only**
  from passed evidence:
  - the supported environment with exact versions. It must name **Pop!_OS**
    or the gate fails, and must not claim Ubuntu/GNOME or any other distro;
  - verified capabilities, and the target-app results per class;
  - known limitations: every `degraded`/`unsupported` class, anything the
    owner waived (for example D6 = B; toggle-only activation **only** if the owner amended D2), CPU-only
    inference, and the need to bind the shortcut manually;
  - how to bind the shortcut on COSMIC.
- It must be at least 300 characters and contain no `TODO` or `REPLACE`.
- Record **OPS-005** with its procedure, which checks that every claim in the
  summary maps to a passed record, and promote it.

## T6.4 — Legal check (D9)

`THIRD_PARTY_NOTICES.md` must exist, have no `TODO`, and have been reviewed by
the owner (D9). If it needs changes, that is a source change: stop, update
it, commit, and **restart WP5 from T5.1**. Do this review early (during WP4)
to avoid a restart.

Second pass (P-8): the current SBOM covers only `Cargo.lock`, with every
licence `NOASSERTION`. Before D9, build the complete inventory the owner will
review:
- Rust crates, with their licences from `cargo metadata`/`cargo deny list`;
- npm packages embedded in the shell (from `package-lock.json`, production
  dependencies only);
- transcribe.cpp/ggml, with the vendored revision and its licence;
- the whisper tiny.en weights. The registry says `Apache-2.0`
  (`crates/narrivy-model-manager/seed/registry.json:48`) and
  `p5_model_cache.rs:1066` pins that, while the notices say MIT. Check the
  upstream model card and fix the registry, the test and the notices
  together;
- the system libraries the package depends on.

Rewrite `THIRD_PARTY_NOTICES.md` from that inventory. It currently says the
model is "not bundled" and lists models that aren't shipped. Make
`build_candidate_supply_chain.py` emit real licence expressions (T4.5 item
11).

Record the owner's D9 approval in `agent/DECISIONS.md` together with the
SHA-256 of the exact `THIRD_PARTY_NOTICES.md` and SBOM reviewed. If
`release_gate.py` gets a legal check (T6.0), it compares those hashes (Sol
S05).

## T6.5 — P11 report and the two gates

```bash
python3 scripts/build_manifest.py
python3 scripts/phase_gate.py --phase P11
python3 scripts/generate_phase_report.py --phase P11 --write-headline
python3 scripts/build_manifest.py
python3 scripts/release_gate.py --target linux-1.0
```

- The P11 JSON report must say `status: passed`, `gate_exit_code: 0` and
  `git_end_commit == FREEZE_COMMIT`. Check `generate_phase_report.py --help`
  for the flags that write the JSON as well as the headline.
- Both gates must exit 0. If the release gate fails, read the grouped errors.
  An evidence-only gap means rerunning the recorder or generator (still
  frozen). A source change needs a new freeze (T5.3).

## T6.6 — Preserve the result

The gate passes only while `HEAD == FREEZE_COMMIT` with the evidence in the
working tree. After you commit the evidence, `HEAD` moves, and a fresh gate
run fails the commit check by design. So:

1. Save the passing gate output to
   `artifacts/release-evidence/linux-1.0/release-gate-output.txt`.
2. **Ask the owner** whether to create a local tag on the freeze commit, for
   example `git tag -a v1.0.0 <FREEZE_COMMIT>`. It is local; do not push.
3. Commit all evidence, the CSV, `BUILD-PACK-MANIFEST.json`, the phase reports,
   `artifacts/release-evidence/**` and `artifacts/sbom/**` as one commit:
   "Record Linux 1.0 release evidence for <FREEZE_COMMIT>".
4. Add a re-verification recipe to `artifacts/release-evidence/linux-1.0/README.md`:
   ```bash
   git worktree add /tmp/narrivy-verify <FREEZE_COMMIT>
   cd /tmp/narrivy-verify
   git checkout <EVIDENCE_COMMIT> -- agent/evidence spec/requirements-traceability.csv \
     BUILD-PACK-MANIFEST.json agent/phase-reports artifacts/release-evidence artifacts/sbom
   # copy the frozen .deb to artifacts/releases/narrivy-linux-amd64.deb
   python3 scripts/release_gate.py --target linux-1.0
   ```
5. Update `agent/STATE.json` (`release_gate: passed`, `phase_status:
   passed`), `agent/RESTART-HANDOFF.md` and `agent/BLOCKERS.md`.
6. Tell the owner the release is qualified, and give them the package path and
   SHA-256. **Do not push.** Pushing and publishing are the owner's decision.
7. **Before the owner's first push (P-9), prepare a scrub list; don't
   rewrite anything yourself.** Run
   `rg -n "/home/|@pop-os|waqar" --hidden -g '!target' -g '!node_modules'`
   over tracked files and list every hit for the owner, for example:
   - `agent/evidence/P0F/p0f-mic-pipewire.json:67` (raw `wpctl status`
     with user@host, PIDs and running app names);
   - `packaging/deb/BUILD_LOG.md`;
   - the supply-chain `result.json`.

   Current files can be fixed in a normal commit. History needs the owner's
   squash-or-full decision at push time.

---

## WP6 exit check (= definition of done)

- `release_gate.py` exit 0 at `FREEZE_COMMIT`, with its output saved;
- the evidence commit and re-verification README exist;
- `STATE.json` is truthful;
- nothing has been pushed.
