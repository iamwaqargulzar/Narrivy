# Release task 020: complete and test the known-phrase runner

Complete the partial output of task019. Work only in:

- `scripts/qualify_candidate_known_phrase.py`
- `tests/packaging/test_qualify_candidate_known_phrase.py` (new)
- `scripts/README.md`

Do not commit, push, use the web/subagents, play audio, access real audio or
user state, build packages, write evidence, or run broad checks. Read the
task019 specification and current runner first. Use Ponytail lite: delete or
simplify code where possible and do not add abstraction without a test need.

Task019 ended at its turn cap after creating only the 1,463-line runner. No
tests or docs exist, so it is not accepted. Codex review found these concrete
issues to repair:

1. Shut down and reap the daemon within a fixed bound **before** the final log
   scan, then scan complete logs. Process-group termination must return a
   truthful outcome; a process that survives TERM/KILL or cannot be reaped must
   fail the run and must never yield `child_failures_ignored=false`.
2. The published result must not expose the absolute repository path/user name.
   Record the source commit and safe repository identity only. Include
   content-free native OS/kernel/desktop/session and physical microphone class
   facts needed to identify the run without recording the source device name.
3. Require the candidate suffix `.deb`. Preserve fail-closed archive and
   build-info validation.
4. Review cleanup ordering. Transcript-bearing extraction/runtime/state must be
   gone before result publication on every success/failure path; cleanup or
   daemon termination uncertainty must force failure. Output remains atomic,
   new-only and mode 0600.
5. Add the focused hermetic test module required by task019. It must never use
   actual audio, D-Bus, network, rc5 or the user's state. Use temporary files
   and injected operations. Cover the task019 boundary list with meaningful
   parameterized cases; do not mirror implementation line-by-line. Include a
   full fake success flow and failures for startup/playback/stop/idle, fragment,
   log leak, child cleanup and transcript-path cleanup.
6. Add the exact CLI invocation and honesty boundary to `scripts/README.md`.

Run only:

```bash
python3 -m unittest tests.packaging.test_qualify_candidate_known_phrase -v
python3 -m py_compile scripts/qualify_candidate_known_phrase.py tests/packaging/test_qualify_candidate_known_phrase.py
git diff --check -- scripts/qualify_candidate_known_phrase.py tests/packaging/test_qualify_candidate_known_phrase.py scripts/README.md
```

Finish with files changed, exact checks/outcomes, and unresolved assumptions.
