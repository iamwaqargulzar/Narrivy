# Recovery Phase 4L — Capture-first streaming admission

**Recorded:** 2026-08-29  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled start-of-speech/lifecycle repair implemented; native
clipping, timing, acoustic, package, and release qualification blocked; all 98
Linux requirements remain blocked

## Objective and scope

Remove the strict streaming lane's unbounded start-of-speech gap. The previous
order admitted the resident ASR stream before opening PipeWire capture; a slow
native begin could therefore miss speech that followed `StartSession`.

Scope is portable streaming orchestration, the Linux retained-chunk terminal
ordering, controlled tests, architecture records, read-only validation, and
generated-cache cleanup. No microphone, audio, transcript, native timing,
evidence/index, requirement status, manifest reseal, packaging, commit, push,
or release action occurred.

## Implemented boundary

- After journal creation, raw-local audio policy approval, and model streaming
  capability validation, the transaction opens capture and waits for the first
  real complete chunk before publishing `Listening` or opening ASR.
- Linux `wait_ready` retains sequence zero outside the channel. Core later feeds
  that chunk and every queued successor through one sequence validator and ASR
  path exactly once.
- The channel remains fixed at eight 80-ms chunks (640 ms), plus the separately
  retained first 80-ms chunk. This is bounded transaction admission, not
  continuous or always-on pre-roll. The effective native margin is unmeasured.
- A producer terminal error preempts the retained chunk. If ASR admission stalls
  until the producer reports backpressure or another terminal loss, no audio or
  volatile partial from the invalid transaction escapes first.
- Capture readiness failure or rejected `Listening` opens no ASR owner.
  Observer-triggered cancellation, ASR admission failure, and ASR admission
  cancellation abort and boundedly reap capture. Cleanup failure remains the
  reported terminal truth and takes precedence over the initiating failure.
- After ASR is admitted, the existing shared absolute cleanup deadline and
  capture/ASR ownership rules are unchanged.

## Independent review and correction

A read-only adversarial review found that the first draft could release its
retained chunk after the Linux producer had already reported queue overflow.
That could publish a partial before the next call exposed
`capture_backpressure`. The Linux adapter now checks producer terminal state
before releasing pending audio. A two-barrier controlled test retains sequence
zero, resumes the producer into overflow, and proves that backpressure preempts
audio while the recorder and parser are reaped.

The review also found that the first controlled core fixture asserted two ASR
feeds without actually caching its readiness chunk. The fixture now consumes
and retains behavior-faithful sequence zero, advances the source cursor, and
proves sequence zero and one are each fed once. Capture-only cleanup-failure
precedence and cancellation from the `Listening` observer were added. The
independent re-review returned PASS and modified no files.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| core library tests | 0 | 96 passed |
| Linux platform library | 0 | 208 passed; 1 attended portal observation ignored |
| focused adversarial overflow test | 0 | retained sequence zero; forced backpressure; no audio escaped; owners reaped |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | pass |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | complete pass; one attended portal observation remains ignored |
| workspace all-target/all-feature Clippy with `-D warnings` | 0 | no diagnostics |
| `cargo fmt --all -- --check` / `git diff --check` | 0 / 0 | pass |
| desktop Vitest / TypeScript | 0 / 0 | 17 files, 25 tests; pre-existing React `act(...)` warnings |
| doctor | 0 | current Linux/COSMIC/PipeWire toolchain observed; no permission granted |
| governance suite | 0 | 29 tests passed |
| dependency/source boundaries | 0 / 0 | 9 protected packages |
| action pins / traceability | 0 / 0 | 5 pins; 136 mapped; all 98 Linux requirements blocked |
| production evidence | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| pack / release | 1 / 1 | dirty/unsealed; all requirements blocked; read-only |
| P11 gate | 2 | quarantined and non-writing |

`cargo deny` was not rerun because Recovery 4L changes no dependency. The last
accepted result remains the relevant blocker: 20 pre-existing desktop/Tauri
transitive advisory or unmaintained findings, with no exception added.

The governance window was non-mutating. Exact matching before/after hashes:

- Git status:
  `d5db2e424c14f6d1a4d088a609937aa875ec737ee8a480f0de0df01e2697e09f`;
- binary diff:
  `f40d7e97d9dcaac50a3036a831620d83e233e6420cfd88656b528f4221d45b66`;
- tracked/unignored inventory/content, with explicit missing-file markers:
  `6fdbc47273b2ac61a58e1eec4250c2e541037ccb59b4e8bf2062cc0821366494`.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Qualification boundary and remaining work

This repair closes the prior period in which no capture owner existed during
ASR admission. It does not prove that the bounded retained-plus-queued window
covers native model admission under load, that the first or last word is never
clipped, or that partial/final latency meets a target. Those require renewed
microphone permission and content-free native stage timing; controlled tests
must not be converted into release evidence.

Without microphone access, suitable next work includes model-worker warm
admission/resource policy, continuous-segmentation design, dependency/security
remediation, packaging hygiene, and truthful desktop failure UX. Queue growth
or continuous listening requires measurement, privacy analysis, and a separate
decision.

## Cleanup

Exact generated `__pycache__` directories from governance were removed after
validation. No package-manager debug log was present. No Narrivy daemon,
recorder, transcribe worker, or sidecar process remained. Intentional Rust,
Node, and model caches were preserved; they are build/cache state, not release
artifacts or abandoned runtime payloads.
