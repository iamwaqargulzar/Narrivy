# ADR-0081: Voice mode listens and speaks in its own Rust worker

Status: Accepted (2026-11-07).

## Context

- Voice mode (D34) ran `scripts/voice_mode.py` in a developer Python with
  sherpa-onnx and OpenVINO GenAI. The installed app had neither, so on Linux
  and Windows the voices showed "Not set up" and nothing could install them.
- The owner: the voices must be downloadable and work in the installed app,
  with parity between Linux and Windows.
- sherpa-onnx 1.x contains eSpeak NG (GPL-3.0). `deny.toml` allows no GPL
  licence. sherpa-onnx 2 (Apache-2.0) drops eSpeak and reads words from the
  voice's word list, which skips words it lacks (plurals, names, numbers).

## Decision

- `engines/voice-sherpa` builds `narrivy-voice-worker`: Silero finds turns,
  Kokoro speaks, through sherpa-onnx 2.0.1 linked statically. Audio goes
  through PipeWire's `pw-record`/`pw-play` on Linux and WASAPI (`cpal`) on
  Windows. It keeps the Python worker's line protocol.
- The worker does not recognise speech. It sends each turn's audio to the
  daemon, which recognises it with the speech model already loaded for
  dictation. Voice mode needs no second speech model.
- English text becomes Kokoro phonemes in the worker (`phonemes.rs`): word
  list first, then a known word plus an ending, then the parts of a
  hyphenated word, spelled acronyms, and simple spelling rules for names.
  Numbers are written as words first.
- The package installs the worker with its own manifest
  (`voice-worker-manifest.json`) and the Silero detector (MIT), which the
  daemon checks against a compiled digest. The MSI does the same beside the
  daemon.
- The voice is a Store pack (`runtime: "voice"` in the runtime catalog),
  installed into `<data>/voices/<id>` and checked against its manifest on
  every start. The app lists it under Models → Text to speech.
- The worker starts with an empty environment. The daemon passes only
  PipeWire's routing variables (`--env KEY=VALUE`), which the worker hands
  only to `pw-record` and `pw-play`. It is started from the thread that
  reads it, because the launcher kills a worker when its starting thread
  ends.

## Consequences

- Voice mode works from the installed package on Linux (verified end to end
  with a virtual microphone). The Windows path builds but has not run yet.
- No Urdu voice in this worker: Piper Urdu needs eSpeak (GPL). The owner
  accepted a separate GPL program for it (D51, ADR-0082).
- Unknown names are said by spelling rules, which can be wrong ("Waqar" was
  heard as "Wackar").
- The build script of sherpa-onnx-sys downloads its pinned prebuilt archive.
  Its build-time crates are allowed by name in `deny.toml`.
