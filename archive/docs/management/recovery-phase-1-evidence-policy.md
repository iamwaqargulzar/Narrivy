# Recovery Phase 1 — Evidence Policy

Recovery Phase 1 resets release governance to a fail-closed baseline. Existing
`agent/evidence/**/*.json` records use schema version 1 and are retained
unchanged as historical material. They are quarantined and are never release,
phase, or requirement proof.

New evidence must use schema version 2. `spec/evidence-verification-profiles.json`
maps every traceability verification layer to a reviewed profile. The profile
defines the permitted evidence types, controlled/native/package execution
environment, artifact-binding policy, native-platform need, and whether a
behavioural runner is mandatory. Unknown layers and unapproved procedures fail
closed. This intentionally does not equate `release_gate: required` with a
native-product claim: the required `unit/contract` rows use controlled unit
evidence and do not claim to exercise a release package.

Every evidence type must have `result: "pass"`, `pass: true`, and exit code
`0`; a clean matching candidate commit; available declared environment; complete
child-failure/substitution declarations; authoritative index registration; and
an approved procedure for its verification ID and profile. Behavioural native
and package profiles must bind the actual artifact exercised through
`artifact_under_test`: an executable invoked by the reviewed runner or a
release `.deb` passed to it. The exact path and SHA-256 must also appear in
`artifact_hashes`. The procedure fixes command argv, runner hash, and required
behavioural assertions. `test <package>`, `true <package>`, comments, prose,
and unrelated commands do not prove package execution. Unit/contract profiles
forbid an artifact-under-test binding and remain truthful controlled evidence.

Each profile and each reviewed procedure also declares
`permitted_substitutions`. A declared substitution is valid only when it is in
both allowlists; controlled unit/integration proof receives no blanket
substitution allowance. Native-behaviour and package-artifact procedures reject
every supported substitute (`mock`, `fixture`, `proxy`, `stub`, and
`synthetic-audio`) regardless of declarations. This rule is applied centrally
by `scripts/evidence_policy.py` to the requirement marker, evidence verifier,
and phase/release validators.

`agent/evidence/INDEX.json` is authoritative for schema-v2 evidence. Each v2
record must be explicitly listed with its exact evidence-file SHA-256 before it
can support a claim. An unregistered or hash-mismatched v2 record is rejected.

`scripts/verify_evidence.py` and the phase/release gates are read-only. The
P9/P11 runners and both P9/P11 evidence-refresh scripts are quarantined; direct
and imported `main()` calls exit `2` without executing legacy commands or
changing files. Generating new evidence requires a reviewed future procedure
that emits schema-v2 records and an explicit index registration. The only
permitted manifest mutation is an explicit packaging action
through `scripts/build_manifest.py`; validation reports mismatches and exits
nonzero.
