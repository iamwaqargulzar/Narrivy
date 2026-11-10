#!/usr/bin/env python3
"""Narrivy's voice filter: what reaches the speech model during dictation.

A resident worker run by the daemon. For each dictation it:

- finds where someone speaks (Silero VAD). With no speech at all it returns
  nothing, so silence never becomes "Thank you." (a phrase Whisper invents
  for silence);
- cuts the silence before, after and between the speech, where Whisper also
  invents words;
- when the user has trained their voice, keeps only the speech that sounds
  like them (3D-Speaker CAM++ voice fingerprints), so other people talking
  nearby are left out.

Protocol on stdin/stdout, one request at a time: a JSON line, then the audio
as little-endian float32 at 16 kHz mono; the reply is a JSON line, then the
audio kept. Nothing said is ever written anywhere else; the voice profile
holds a fingerprint, not audio.

  {"op": "filter", "samples": N, "profile": PATH|null, "threshold": T?}
                                                         -> {"kept": M, "speech_ms": .., "other_ms": ..}
  {"op": "enroll", "samples": N, "profile": PATH}        -> {"ok": true, "speech_ms": .., "consistency": ..}
                                                            or {"ok": false, "error": CODE}

Run with the Python that has sherpa-onnx (~/.cache/narrivy/convert-venv).
"""

import json
import os
import sys
import time
from pathlib import Path

import numpy as np
import sherpa_onnx

RATE = 16000
MODELS = Path(os.environ.get("NARRIVY_DIARIZATION_MODELS", Path.home() / ".cache/narrivy/diarization"))
# Speech is kept with this much around it, so first and last sounds survive.
# Soft first sounds ("check", "what") start before Silero is sure.
PAD = 0.3
# A piece at least this long is checked against the user's voice; shorter
# pieces follow the nearest checked piece.
CHECKED = 1.0
# The user's own speech scores about 0.45-0.9 against their profile; other
# people usually below 0.25. Leaning towards keeping: dropping the user's own
# words is worse than letting a stray voice through.
SAME_VOICE = 0.3
# The user's own pieces in one dictation score alike; another person scores
# far below the user's best piece. A piece is left out only when it is under
# the threshold AND this far below the best piece of the same dictation, so
# one weak-sounding clause of the user's own is never cut.
OTHER_VOICE_GAP = 0.15
# Training needs this much speech.
ENROLL_SPEECH = 6.0


def vad():
    config = sherpa_onnx.VadModelConfig()
    config.silero_vad.model = str(MODELS / "silero_vad.onnx")
    config.silero_vad.threshold = 0.45
    config.silero_vad.min_silence_duration = 0.35
    config.silero_vad.min_speech_duration = 0.2
    config.silero_vad.max_speech_duration = 20
    config.sample_rate = RATE
    return config


VAD = vad()
EXTRACTOR = sherpa_onnx.SpeakerEmbeddingExtractor(
    sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=str(MODELS / "campplus.onnx"), num_threads=2)
)
_profiles: dict[str, tuple[float, np.ndarray]] = {}


def speech(samples: np.ndarray) -> list[tuple[int, int]]:
    """Speech as (start, end) sample offsets, padded and merged."""
    detector = sherpa_onnx.VoiceActivityDetector(VAD, buffer_size_in_seconds=max(30, len(samples) // RATE + 5))
    window = VAD.silero_vad.window_size
    found = []
    for i in range(0, len(samples) - window + 1, window):
        detector.accept_waveform(samples[i : i + window])
        while not detector.empty():
            found.append((detector.front.start, detector.front.start + len(detector.front.samples)))
            detector.pop()
    detector.flush()
    while not detector.empty():
        found.append((detector.front.start, detector.front.start + len(detector.front.samples)))
        detector.pop()
    pad = int(PAD * RATE)
    merged: list[tuple[int, int]] = []
    for start, end in found:
        start, end = max(0, start - pad), min(len(samples), end + pad)
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
        else:
            merged.append((start, end))
    return merged


def fingerprint(samples: np.ndarray) -> np.ndarray:
    s = EXTRACTOR.create_stream()
    s.accept_waveform(RATE, np.ascontiguousarray(samples, dtype=np.float32))
    s.input_finished()
    found = np.array(EXTRACTOR.compute(s), dtype=np.float32)
    return found / (np.linalg.norm(found) + 1e-9)


def profile(path: str | None) -> np.ndarray | None:
    if not path:
        return None
    try:
        mtime = os.stat(path).st_mtime
    except OSError:
        return None
    cached = _profiles.get(path)
    if cached and cached[0] == mtime:
        return cached[1]
    try:
        vector = np.array(json.loads(Path(path).read_text())["fingerprint"], dtype=np.float32)
    except (OSError, ValueError, KeyError):
        return None
    vector /= np.linalg.norm(vector) + 1e-9
    _profiles[path] = (mtime, vector)
    return vector


def filter_audio(samples: np.ndarray, voice: np.ndarray | None, threshold: float = SAME_VOICE):
    spans = speech(samples)
    if not spans:
        return np.zeros(0, dtype=np.float32), 0, 0
    keep = [True] * len(spans)
    if voice is not None:
        scores = {
            i: float(np.dot(fingerprint(samples[start:end]), voice))
            for i, (start, end) in enumerate(spans)
            if end - start >= CHECKED * RATE
        }
        keep = decide(len(spans), scores, threshold)
    gap = np.zeros(int(0.15 * RATE), dtype=np.float32)
    parts, speech_n, other_n = [], 0, 0
    for (start, end), kept in zip(spans, keep):
        if kept:
            if parts:
                parts.append(gap)
            parts.append(samples[start:end])
            speech_n += end - start
        else:
            other_n += end - start
    out = np.concatenate(parts) if parts else np.zeros(0, dtype=np.float32)
    return out.astype(np.float32), speech_n * 1000 // RATE, other_n * 1000 // RATE


def decide(count: int, scores: dict[int, float], threshold: float) -> list[bool]:
    """Which of `count` pieces to keep, from the voice scores of the pieces
    long enough to check (by index). Short pieces follow the nearest checked
    piece. When no piece sounds like the user (another microphone, a cold),
    everything is kept: dropping the user's own words is the worse mistake."""
    if not scores or max(scores.values()) < threshold:
        return [True] * count
    best = max(scores.values())
    checked = {i: s >= threshold or s >= best - OTHER_VOICE_GAP for i, s in scores.items()}
    return [checked[i] if i in checked else checked[min(checked, key=lambda j: abs(j - i))] for i in range(count)]


def enroll(samples: np.ndarray, path: str) -> dict:
    spans = speech(samples)
    voiced = np.concatenate([samples[s:e] for s, e in spans]) if spans else np.zeros(0, dtype=np.float32)
    if len(voiced) < ENROLL_SPEECH * RATE:
        return {"ok": False, "error": "voice_training_too_short", "speech_ms": len(voiced) * 1000 // RATE}
    step = 3 * RATE
    prints = [fingerprint(voiced[i : i + step]) for i in range(0, len(voiced) - RATE, step)]
    centre = np.mean(prints, axis=0)
    centre /= np.linalg.norm(centre) + 1e-9
    consistency = float(np.mean([np.dot(p, centre) for p in prints]))
    part = Path(path + ".part")
    part.write_text(json.dumps({"fingerprint": centre.tolist(), "speech_ms": len(voiced) * 1000 // RATE, "created": int(time.time())}))
    os.chmod(part, 0o600)
    part.replace(path)
    return {"ok": True, "speech_ms": len(voiced) * 1000 // RATE, "consistency": round(consistency, 3)}


def read_exact(stream, n: int) -> bytes:
    chunks, got = [], 0
    while got < n:
        chunk = stream.read(n - got)
        if not chunk:
            raise EOFError
        chunks.append(chunk)
        got += len(chunk)
    return b"".join(chunks)


def main() -> None:
    stdin, stdout = sys.stdin.buffer, sys.stdout.buffer
    stdout.write(b'{"ready": true}\n')
    stdout.flush()
    while True:
        line = stdin.readline()
        if not line:
            return
        request = json.loads(line)
        count = int(request.get("samples", 0))
        samples = np.frombuffer(read_exact(stdin, count * 4), dtype="<f4").astype(np.float32) if count else np.zeros(0, dtype=np.float32)
        if request.get("op") == "enroll":
            reply = enroll(samples, request["profile"])
            stdout.write((json.dumps(reply) + "\n").encode())
        else:
            threshold = float(request.get("threshold") or SAME_VOICE)
            threshold = min(max(threshold, 0.1), 0.7)
            kept, speech_ms, other_ms = filter_audio(samples, profile(request.get("profile")), threshold)
            stdout.write((json.dumps({"kept": len(kept), "speech_ms": speech_ms, "other_ms": other_ms}) + "\n").encode())
            stdout.write(kept.astype("<f4").tobytes())
        stdout.flush()


if __name__ == "__main__":
    os.environ.setdefault("OMP_NUM_THREADS", "2")
    try:
        main()
    except (EOFError, BrokenPipeError, KeyboardInterrupt):
        pass
