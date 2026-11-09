# ADR-0082: The Urdu voice runs in a separate GPL program

Status: Accepted (2026-11-07). Amends ADR-0081.

## Context

- ADR-0081 left voice mode without Urdu: Piper's Urdu voice needs eSpeak NG
  (GPL-3.0) to turn Urdu text into phonemes, and `deny.toml` allows no GPL.
- The owner accepted GPL for the Urdu voice (D51).
- sherpa-onnx 2 (used by the voice worker) has no eSpeak NG; 1.13.8 still
  links it statically. Both crates declare `links = "sherpa-onnx"`, so they
  cannot share one Cargo workspace.

## Decision

- `engines/voice-urdu` is its own workspace (excluded from the main one)
  and builds `narrivy-urdu-speaker`, GPL-3.0-or-later, on sherpa-onnx
  1.13.8. It reads one JSON line of text and answers with the audio
  (base64 f32), and ends when its input closes. It logs nothing.
- The voice worker stays MIT. It starts the Urdu speaker with an empty
  environment when the daemon passes `--urdu-speaker` and `--urdu-voice`,
  sends it sentences written in Urdu letters, and plays what comes back.
  If it fails, English reads the rest of the session
  (`urdu_speaker_failed`).
- The daemon names the Urdu speaker only after checking it: the packaged
  one through its own fixed-root manifest (`urdu-speaker-manifest.json`,
  runtime `urdu-speaker`), a developer one by `NARRIVY_URDU_SPEAKER` with
  its digest and size. Unlike the other workers it is started by path, by
  the worker; the path is root- (or administrator-) owned in both packages.
- The Urdu voice is a Store pack, `voice-piper-ur-fasih-medium`
  (Piper `ur_PK-fasih-medium`, MIT, plus the seven eSpeak NG data files
  Urdu needs, GPL-3.0-or-later). The files sit flat beside the model and
  eSpeak NG is pointed at the pack itself, because the Store allows paths
  at most two folders deep.
- An English voice is still what voice mode needs; Urdu only adds Urdu.
- The `.deb` and the MSI ship the GPL text (`COPYING`) and a source note
  (`SOURCE`) beside the program; its source is this repository.

## Consequences

- No GPL code is linked into the daemon, the app or the voice worker;
  `deny.toml` still applies unchanged to the main workspace. The Urdu
  workspace has its own `Cargo.lock` and is not checked by `cargo deny`.
- Release builds compile one more workspace and its prebuilt sherpa-onnx
  archive (about 23 MB on Linux, 123 MB on Windows, downloaded at build).
- Whisper large-v3-turbo heard the Urdu voice's test sentence back nearly
  word for word; the Windows build has not run yet.
