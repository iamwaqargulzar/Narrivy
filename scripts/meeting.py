#!/usr/bin/env python3
"""Record a meeting and write a transcript separated by speaker, locally.

A development tool ahead of Narrivy's meeting feature (F22). Everything runs
on this computer: nothing is uploaded.

  record     Your microphone and the computer's sound go to two files, so
             people on a call (heard through the computer) are kept apart
             from the people in the room (heard by the microphone).
  live       While recording, follow both files and write each turn to
             live.jsonl as soon as it is said: speech is cut by Silero VAD,
             voices are told apart on the fly with CAM++ fingerprints, and
             Whisper writes the words. Stops (finishing what is left) on
             SIGINT/SIGTERM or when its parent ends.
  transcribe Each file is split by voice (pyannote segmentation 3.0 and
             3D-Speaker CAM++ voice fingerprints, through sherpa-onnx), and
             every turn is transcribed by Whisper on OpenVINO, with the
             language found per turn (Urdu taken for Hindi is redone as Urdu).

    scripts/meeting.py record ~/Meetings/2026-10-07
    scripts/meeting.py live ~/Meetings/2026-10-07 [--device GPU]
    scripts/meeting.py transcribe ~/Meetings/2026-10-07 [--room-speakers N]
        [--call-speakers N] [--languages en,ur] [--device GPU]
    scripts/meeting.py rename ~/Meetings/2026-10-07 "Person 1=Waqar" "Person 2=Ali"

Run with the Python that has sherpa-onnx and openvino-genai:
~/.cache/narrivy/convert-venv/bin/python. On calls without headphones, the
microphone also hears the other people: Narrivy records it through its echo
canceller, and words heard twice are kept once (as the call's).
"""

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import time
import wave
from pathlib import Path

RATE = 16000
HOME = Path.home()
MODELS = HOME / ".cache/narrivy/diarization"
WHISPER = HOME / ".cache/narrivy/models/whisper-large-v3-turbo-openvino-int8"
STREAMS = {"room": "mic.wav", "call": "system.wav"}
LABEL = {"room": "Room", "call": "Call"}
# With --progress (as Narrivy's meeting worker), stdout carries only JSON
# progress lines and messages for people go to stderr. No line carries text
# that was said.
PROGRESS = False


def say(*parts, **kwargs) -> None:
    print(*parts, file=sys.stderr if PROGRESS else sys.stdout, **kwargs)


def progress(stage: str, done: int, total: int) -> None:
    if PROGRESS:
        print(json.dumps({"stage": stage, "done": done, "total": total}), flush=True)


# ------------------------------------------------------------------ record


def default_sink() -> str:
    return subprocess.run(
        ["pactl", "get-default-sink"], capture_output=True, text=True, check=True
    ).stdout.strip()


def default_source() -> str:
    return subprocess.run(
        ["pactl", "get-default-source"], capture_output=True, text=True, check=True
    ).stdout.strip()


def record(out: Path, mic: str | None, monitor: str | None) -> None:
    if not shutil.which("pw-record"):
        sys.exit("pw-record (PipeWire) is not installed")
    out.mkdir(parents=True, exist_ok=True)
    mic = mic or default_source()
    # PipeWire has no `<sink>.monitor` node (asking for one records the
    # microphone): capture the output by targeting the sink itself.
    monitor = (monitor or default_sink()).removesuffix(".monitor")
    common = ["--rate", str(RATE), "--channels", "1", "--format", "s16"]
    targets = {"room": mic, "call": monitor}
    capture = {"room": [], "call": ["-P", "{ stream.capture.sink = true }"]}
    procs = []
    for stream, target in targets.items():
        path = out / STREAMS[stream]
        if path.exists():
            sys.exit(f"{path} exists: choose a new folder")
        procs.append(
            subprocess.Popen(
                ["pw-record", "--target", target, *capture[stream], *common, str(path)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
            )
        )
    (out / "recording.json").write_text(
        json.dumps({"started": time.strftime("%Y-%m-%d %H:%M:%S"), "mic": mic, "monitor": monitor}, indent=2)
    )
    print(f"Recording to {out}")
    print(f"  microphone:     {mic}")
    print(f"  computer sound: {monitor}")
    print("Press Ctrl+C to stop.")
    started = time.time()
    try:
        while all(p.poll() is None for p in procs):
            elapsed = int(time.time() - started)
            print(f"\r  {elapsed // 3600:02d}:{elapsed // 60 % 60:02d}:{elapsed % 60:02d}", end="", flush=True)
            time.sleep(1)
    except KeyboardInterrupt:
        pass
    for p in procs:
        if p.poll() is None:
            p.send_signal(signal.SIGINT)
    for p in procs:
        try:
            p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            p.kill()
        if p.returncode not in (0, -2, 130, None) and p.stderr:
            print(p.stderr.read().decode(errors="replace"), file=sys.stderr)
    print(f"\nStopped. Next:\n  {sys.executable} {sys.argv[0]} transcribe {out}")


# -------------------------------------------------------------- transcribe


def read_wav(path: Path):
    import numpy as np

    with wave.open(str(path)) as w:
        if w.getframerate() != RATE or w.getnchannels() != 1 or w.getsampwidth() != 2:
            sys.exit(f"{path}: expected 16 kHz mono 16-bit")
        data = w.readframes(w.getnframes())
    return np.frombuffer(data, dtype=np.int16).astype(np.float32) / 32768.0


def write_full_recording(folder: Path, audio) -> None:
    """One file with everyone (microphone and computer sound together), kept
    as a backup of the whole meeting to listen to."""
    import numpy as np

    path = folder / "recording.wav"
    if path.exists() or not audio:
        return
    length = max(len(samples) for samples in audio.values())
    mixed = np.zeros(length, dtype=np.float32)
    for samples in audio.values():
        mixed[: len(samples)] += samples
    peak = float(np.max(np.abs(mixed))) if length else 0.0
    if peak > 0.99:
        mixed *= 0.99 / peak
    part = folder / "recording.wav.part"
    with wave.open(str(part), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(RATE)
        out.writeframes((mixed * 32767).astype(np.int16).tobytes())
    part.replace(path)


def speech_spans(samples):
    """Where anyone speaks (Silero VAD), as (start, end) seconds."""
    import sherpa_onnx

    config = sherpa_onnx.VadModelConfig()
    config.silero_vad.model = str(MODELS / "silero_vad.onnx")
    config.silero_vad.threshold = 0.4
    config.silero_vad.min_silence_duration = 0.4
    config.silero_vad.min_speech_duration = 0.25
    config.sample_rate = RATE
    vad = sherpa_onnx.VoiceActivityDetector(config, buffer_size_in_seconds=60)
    spans = []
    window = config.silero_vad.window_size
    for i in range(0, len(samples), window):
        vad.accept_waveform(samples[i : i + window])
        while not vad.empty():
            seg = vad.front
            spans.append((seg.start / RATE, (seg.start + len(seg.samples)) / RATE))
            vad.pop()
    vad.flush()
    while not vad.empty():
        seg = vad.front
        spans.append((seg.start / RATE, (seg.start + len(seg.samples)) / RATE))
        vad.pop()
    return spans


def diarize(samples, speakers: int | None, threshold: float):
    """Turns as (start, end, speaker index)."""
    import sherpa_onnx

    config = sherpa_onnx.OfflineSpeakerDiarizationConfig(
        segmentation=sherpa_onnx.OfflineSpeakerSegmentationModelConfig(
            pyannote=sherpa_onnx.OfflineSpeakerSegmentationPyannoteModelConfig(
                model=str(MODELS / "sherpa-onnx-pyannote-segmentation-3-0/model.onnx")
            ),
            num_threads=4,
        ),
        embedding=sherpa_onnx.SpeakerEmbeddingExtractorConfig(
            model=str(MODELS / "campplus.onnx"), num_threads=4
        ),
        clustering=sherpa_onnx.FastClusteringConfig(
            num_clusters=speakers if speakers else -1, threshold=threshold
        ),
        min_duration_on=0.3,
        min_duration_off=0.5,
    )
    if not config.validate():
        sys.exit("speaker models are missing from ~/.cache/narrivy/diarization")
    sd = sherpa_onnx.OfflineSpeakerDiarization(config)
    result = sd.process(samples).sort_by_start_time()
    return [(r.start, r.end, r.speaker) for r in result]


def merge_turns(turns, gap=0.8, longest=28.0):
    """Join a speaker's pieces that follow each other closely."""
    merged = []
    for start, end, who in turns:
        if merged and merged[-1][2] == who and start - merged[-1][1] <= gap and end - merged[-1][0] <= longest:
            merged[-1] = (merged[-1][0], end, who)
        else:
            merged.append((start, end, who))
    return merged


def embedder():
    import sherpa_onnx

    return sherpa_onnx.SpeakerEmbeddingExtractor(
        sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=str(MODELS / "campplus.onnx"), num_threads=2)
    )


def fingerprint(extractor, samples):
    """A voice fingerprint, scaled so that alike voices have a dot product
    near 1."""
    import numpy as np

    s = extractor.create_stream()
    s.accept_waveform(RATE, np.ascontiguousarray(samples, dtype=np.float32))
    s.input_finished()
    found = np.array(extractor.compute(s), dtype=np.float32)
    return found / (np.linalg.norm(found) + 1e-9)


# A "voice" heard for less than this in total is checked against the others.
SMALL_VOICE = 4.0
# Pieces of one person score about 0.5-0.9 against their main voice; two
# different people usually below 0.3.
SAME_VOICE = 0.4


def absorb_small_voices(turns, samples, extractor):
    """Short pieces (a one-second "okay") give unreliable fingerprints and
    are often taken for a new person. A voice heard only briefly that sounds
    like a main voice is that voice."""
    import numpy as np

    total = {}
    for start, end, who in turns:
        total[who] = total.get(who, 0.0) + end - start
    main = [who for who, seconds in total.items() if seconds >= SMALL_VOICE]
    small = [who for who, seconds in total.items() if seconds < SMALL_VOICE]
    if not main or not small:
        return turns

    def centroid(who):
        prints = [
            fingerprint(extractor, samples[int(s * RATE) : int(e * RATE)])
            for s, e, w in turns
            if w == who and e - s >= 0.5
        ]
        if not prints:
            return None
        c = np.mean(prints, axis=0)
        return c / (np.linalg.norm(c) + 1e-9)

    centres = {who: centroid(who) for who in main}
    moved = {}
    for who in small:
        mine = centroid(who)
        if mine is None:
            continue
        best = max(centres, key=lambda m: -2 if centres[m] is None else float(np.dot(mine, centres[m])))
        if centres[best] is not None and float(np.dot(mine, centres[best])) >= SAME_VOICE:
            moved[who] = best
    return merge_turns([(s, e, moved.get(w, w)) for s, e, w in turns])


# Two voices whose average fingerprints are this alike are one person. One
# speaker split apart by the clustering scores about 0.7-0.9 between the
# parts (a talk changes tone and pace); two different people stay below
# about 0.5.
ONE_PERSON = 0.65


def join_alike_voices(turns, samples, extractor):
    """The clustering can split one long speaker into several voices that
    sound almost the same. Join the closest pair while it is alike enough,
    so one person stays one person."""
    import numpy as np

    def centroid(who, labels):
        prints = [
            fingerprint(extractor, samples[int(s * RATE) : int(e * RATE)])
            for (s, e, _), w in zip(turns, labels)
            if w == who and e - s >= 0.5
        ]
        if not prints:
            return None
        c = np.mean(prints, axis=0)
        return c / (np.linalg.norm(c) + 1e-9)

    labels = [w for _, _, w in turns]
    while True:
        voices = sorted(set(labels), key=labels.index)
        centres = {who: centroid(who, labels) for who in voices}
        best = None
        for i, a in enumerate(voices):
            for b in voices[i + 1 :]:
                if centres[a] is None or centres[b] is None:
                    continue
                score = float(np.dot(centres[a], centres[b]))
                if score >= ONE_PERSON and (best is None or score > best[0]):
                    best = (score, a, b)
        if best is None:
            break
        _, keep, gone = best
        labels = [keep if w == gone else w for w in labels]
    return merge_turns([(s, e, w) for (s, e, _), w in zip(turns, labels)])


# Pieces shorter than this give unreliable fingerprints: they go to the
# voice next to them instead of deciding a voice of their own.
SHORTEST_OWN = 0.8


def short_follow_neighbours(turns):
    """A piece under SHORTEST_OWN seconds takes the voice of the piece
    before it (or after it, at the start)."""
    labels = [w for _, _, w in turns]
    for i, (start, end, _) in enumerate(turns):
        if end - start >= SHORTEST_OWN:
            continue
        before = next((labels[j] for j in range(i - 1, -1, -1) if turns[j][1] - turns[j][0] >= SHORTEST_OWN), None)
        after = next((labels[j] for j in range(i + 1, len(turns)) if turns[j][1] - turns[j][0] >= SHORTEST_OWN), None)
        labels[i] = before if before is not None else after if after is not None else labels[i]
    return merge_turns([(s, e, w) for (s, e, _), w in zip(turns, labels)])


# ------------------------------------------------------------ known voices
#
# The user's own voice (their trained profile) is labelled "You"; people
# the user named in earlier meetings are recognised by their remembered
# voice. Fingerprints only: no audio is kept.

YOU = "You"
# The user's speech scores about 0.5-0.9 against their trained profile;
# other people below about 0.3.
IS_YOU = 0.5
# A remembered person: at least this alike is them; a little less is shown
# as a guess ("Name?") for the user to confirm by naming.
IS_KNOWN = 0.70
MAYBE_KNOWN = 0.55


def unit(vector):
    import numpy as np

    v = np.asarray(vector, dtype=np.float32)
    return v / (np.linalg.norm(v) + 1e-9)


class Voices:
    """The user's profile and the remembered people."""

    def __init__(self, profile: Path | None, known: Path | None):
        self.you = None
        self.known = {}
        try:
            if profile and profile.exists():
                self.you = unit(json.loads(profile.read_text())["fingerprint"])
        except (OSError, ValueError, KeyError):
            self.you = None
        try:
            if known and known.exists():
                self.known = {name: unit(v["fingerprint"]) for name, v in json.loads(known.read_text()).items()}
        except (OSError, ValueError, KeyError, TypeError):
            self.known = {}

    def names(self, centres: dict) -> dict:
        """Names for voices (label -> centroid): "You" for the one most
        like the user, then remembered people, each name used once."""
        import numpy as np

        found = {}
        if self.you is not None:
            scored = [(float(np.dot(c, self.you)), who) for who, c in centres.items()]
            if scored:
                score, who = max(scored)
                if score >= IS_YOU:
                    found[who] = YOU
        pairs = sorted(
            ((float(np.dot(c, v)), who, name) for who, c in centres.items() for name, v in self.known.items()),
            reverse=True,
        )
        used = set()
        for score, who, name in pairs:
            if score < MAYBE_KNOWN:
                break
            if who in found or name in used:
                continue
            found[who] = name if score >= IS_KNOWN else f"{name}?"
            used.add(name)
        return found


def centres_of(turns, audio, extractor):
    """Each speaker's average fingerprint, from pieces long enough."""
    import numpy as np

    prints = {}
    for turn in turns:
        if turn["end"] - turn["start"] < 0.5:
            continue
        samples = audio[turn["stream"]][int(turn["start"] * RATE) : int(turn["end"] * RATE)]
        prints.setdefault(turn["speaker"], []).append(fingerprint(extractor, samples))
    return {who: unit(np.mean(p, axis=0)) for who, p in prints.items()}


def set_auto_names(folder: Path, found: dict) -> None:
    """Write names found by voice, leaving names the user gave alone. Names
    this script gave earlier (a live guess) are replaced."""
    names_path, auto_path = folder / "names.json", folder / "auto-names.json"
    try:
        names = json.loads(names_path.read_text()) if names_path.exists() else {}
        auto = json.loads(auto_path.read_text()) if auto_path.exists() else {}
    except (OSError, ValueError):
        return
    for who, name in auto.items():
        if names.get(who) == name:
            del names[who]
    given = {}
    for who, name in found.items():
        if who not in names:
            names[who] = name
            given[who] = name
    for path, data in ((names_path, names), (auto_path, given)):
        part = path.with_suffix(".part")
        part.write_text(json.dumps(data, ensure_ascii=False, indent=1))
        part.replace(path)


def remember(folder: Path, speaker: str, name: str, known: Path) -> None:
    """Remember the voice of `speaker` in this meeting as `name`, so later
    meetings recognise them. The stored fingerprint is a running average."""
    import numpy as np

    name = name.strip().rstrip("?").strip()
    if not name or name == YOU:
        return
    turns = [t for t in json.loads((folder / "transcript.json").read_text()) if t["speaker"] == speaker]
    audio = {}
    for stream in {t["stream"] for t in turns}:
        path = folder / STREAMS[stream]
        if path.exists():
            audio[stream] = read_wav(path)
    turns = [t for t in turns if t["stream"] in audio]
    centre = centres_of(turns, audio, embedder()).get(speaker)
    if centre is None:
        return
    try:
        store = json.loads(known.read_text()) if known.exists() else {}
    except (OSError, ValueError):
        store = {}
    old = store.get(name)
    if old:
        count = int(old.get("count", 1))
        centre = unit(unit(old["fingerprint"]) * count + centre)
        count += 1
    else:
        count = 1
    store[name] = {"fingerprint": [round(float(x), 6) for x in centre], "count": count}
    part = known.with_suffix(".part")
    part.write_text(json.dumps(store, ensure_ascii=False))
    os.chmod(part, 0o600)
    part.replace(known)


class Transcriber:
    def __init__(self, model: Path, device: str, languages: list[str]):
        import openvino_genai as og

        say(f"Loading Whisper ({model.name}) on {device}…", flush=True)
        self.pipe = og.WhisperPipeline(str(model), device)
        self.languages = languages

    def run(self, audio, language=None, no_repeat=0):
        config = self.pipe.get_generation_config()
        config.task = "transcribe"
        config.language = f"<|{language}|>" if language else None
        config.return_timestamps = False
        config.max_new_tokens = 440
        if no_repeat:
            config.no_repeat_ngram_size = no_repeat
        result = self.pipe.generate(audio.tolist(), config)
        detected = (getattr(result, "language", None) or language or "").strip("<|>")
        return result.texts[0].strip(), detected

    def __call__(self, audio):
        text, detected = self.run(audio)
        if detected == "hi" and "ur" in self.languages:
            # Spoken Urdu is often taken for Hindi and written in Devanagari.
            text, detected = self.run(audio, "ur")
        if looping(text):
            text, detected = self.run(audio, detected or None, no_repeat=4)
            if looping(text):
                text = cut_loop(text)
        return text, detected


def looping(text: str) -> bool:
    """Whisper sometimes repeats a phrase until it runs out of room."""
    import zlib

    raw = text.encode()
    return len(raw) > 120 and len(raw) / len(zlib.compress(raw)) > 2.6


def cut_loop(text: str) -> str:
    words = text.split()
    for size in range(1, 9):
        for i in range(len(words) - 3 * size):
            gram = words[i : i + size]
            if words[i + size : i + 2 * size] == gram and words[i + 2 * size : i + 3 * size] == gram:
                return " ".join(words[: i + size]) + " […]"
    return text


def clock(seconds: float) -> str:
    seconds = int(seconds)
    return f"{seconds // 3600:02d}:{seconds // 60 % 60:02d}:{seconds % 60:02d}"


def transcribe(folder: Path, args) -> None:
    import numpy as np

    progress("load", 0, 1)
    transcriber = Transcriber(
        Path(args.model).expanduser(), args.device, [c.strip() for c in args.languages.split(",") if c.strip()]
    )
    progress("load", 1, 1)
    audio = {}
    for stream, name in STREAMS.items():
        path = folder / name
        if path.exists() and path.stat().st_size > 44 + RATE:
            audio[stream] = read_wav(path)
    if not audio:
        sys.exit(f"no recordings in {folder}")

    write_full_recording(folder, audio)
    extractor = embedder()
    progress("speakers", 0, len(audio))
    speech = {stream: speech_spans(samples) for stream, samples in audio.items()}
    turns = []
    for done, (stream, samples) in enumerate(audio.items(), 1):
        heard = sum(e - s for s, e in speech[stream])
        if heard < 1.0:
            say(f"{LABEL[stream]}: no speech")
            progress("speakers", done, len(audio))
            continue
        speakers = args.room_speakers if stream == "room" else args.call_speakers
        t0 = time.time()
        found = merge_turns(diarize(samples, speakers, args.threshold))
        found = short_follow_neighbours(found)
        if not speakers:
            found = absorb_small_voices(found, samples, extractor)
            found = join_alike_voices(found, samples, extractor)
        # Number speakers 1, 2, 3 in the order they first speak.
        order = {}
        for _, _, who in found:
            order.setdefault(who, len(order))
        say(f"{LABEL[stream]}: {len(order)} speaker(s), {len(found)} turns ({time.time() - t0:.0f} s)")
        progress("speakers", done, len(audio))
        for start, end, who in found:
            who = order[who]
            turns.append({"stream": stream, "start": start, "end": end, "speaker": f"{LABEL[stream]} {who + 1}"})

    turns.sort(key=lambda t: t["start"])
    t0 = time.time()
    for i, turn in enumerate(turns, 1):
        samples = audio[turn["stream"]]
        piece = samples[int(turn["start"] * RATE) : int(turn["end"] * RATE)]
        if len(piece) < int(0.4 * RATE):
            turn["text"], turn["language"] = "", ""
            progress("words", i, len(turns))
            continue
        # Whisper expects at most 30 s per call; longer turns go in parts.
        texts, langs = [], []
        for j in range(0, len(piece), 29 * RATE):
            part = piece[j : j + 29 * RATE]
            if len(part) < int(0.4 * RATE):
                continue
            text, lang = transcriber(np.ascontiguousarray(part))
            texts.append(text)
            langs.append(lang)
        turn["text"] = " ".join(t for t in texts if t)
        turn["language"] = max(set(langs), key=langs.count) if langs else ""
        progress("words", i, len(turns))
        if not PROGRESS:
            print(f"\r  transcribed {i}/{len(turns)} turns", end="", flush=True)
    say(f"\nWhisper: {time.time() - t0:.0f} s")
    turns = [t for t in turns if t["text"] and not invented(t["text"], t["end"] - t["start"])]
    # The microphone hearing the call through the speakers says the same
    # words at the same time: keep the call's copy.
    calls = [t for t in turns if t["stream"] == "call"]
    turns = [
        t
        for t in turns
        if t["stream"] != "room"
        or not any(c["start"] < t["end"] + 1 and c["end"] > t["start"] - 1 and same_words(t["text"], c["text"]) for c in calls)
    ]
    # People are numbered in the order they first speak, after dropping
    # turns with no words so no number is skipped. Whether someone was heard
    # by the microphone or through the computer stays in "stream".
    numbers: dict[str, str] = {}
    for turn in turns:
        if turn["speaker"] not in numbers:
            numbers[turn["speaker"]] = f"Person {len(numbers) + 1}"
        turn["speaker"] = numbers[turn["speaker"]]
    voices = Voices(args.profile, args.known)
    set_auto_names(folder, voices.names(centres_of(turns, audio, extractor)))
    temporary = folder / "transcript.json.part"
    temporary.write_text(json.dumps(turns, ensure_ascii=False, indent=1))
    temporary.replace(folder / "transcript.json")
    if not PROGRESS:
        write_markdown(folder, turns, {})


def write_markdown(folder: Path, turns, names: dict[str, str]) -> None:
    lines = [f"# Meeting transcript\n", f"_{folder.name}. People are told apart by voice._\n"]
    previous = None
    for turn in turns:
        who = names.get(turn["speaker"], turn["speaker"])
        lang = f" ({turn['language']})" if turn.get("language") not in ("", "en") else ""
        if who == previous:
            lines.append(f"{turn['text']}\n")
        else:
            lines.append(f"\n**{who}** · {clock(turn['start'])}{lang}\n\n{turn['text']}\n")
        previous = who
    path = folder / "transcript.md"
    path.write_text("".join(lines))
    print(f"Transcript: {path}")


def rename(folder: Path, pairs: list[str]) -> None:
    turns = json.loads((folder / "transcript.json").read_text())
    names_path = folder / "names.json"
    names = json.loads(names_path.read_text()) if names_path.exists() else {}
    for pair in pairs:
        old, _, new = pair.partition("=")
        if not new:
            sys.exit(f'use "Person 1=Name", not "{pair}"')
        names[old.strip()] = new.strip()
    names_path.write_text(json.dumps(names, ensure_ascii=False, indent=1))
    write_markdown(folder, turns, names)


# -------------------------------------------------------------------- live


def data_offset(head: bytes) -> int | None:
    """Where the samples start in a WAV file still being written."""
    if len(head) < 12 or head[:4] != b"RIFF" or head[8:12] != b"WAVE":
        return None
    pos = 12
    while pos + 8 <= len(head):
        size = int.from_bytes(head[pos + 4 : pos + 8], "little")
        if head[pos : pos + 4] == b"data":
            return pos + 8
        pos += 8 + size + (size & 1)
    return None


class Tail:
    """New samples appended to a recording that is still growing."""

    def __init__(self, path: Path):
        self.path = path
        self.file = None
        self.offset = None
        self.carry = b""

    def read(self):
        import numpy as np

        if self.file is None:
            if not self.path.exists():
                return None
            self.file = open(self.path, "rb")
        if self.offset is None:
            self.file.seek(0)
            self.offset = data_offset(self.file.read(4096))
            if self.offset is None:
                return None
            self.file.seek(self.offset)
        data = self.carry + self.file.read()
        whole = len(data) // 2 * 2
        self.carry = data[whole:]
        if not whole:
            return None
        return np.frombuffer(data[:whole], dtype="<i2").astype(np.float32) / 32768.0


# The live view: speech is cut into pieces of at most this many seconds, and
# a piece still going on is transcribed again this often, so words show up
# while they are said. The detector's own longest piece is not a hard limit
# (a video's talk with music behind ran into 28 s pieces, each shown only
# once it ended): a piece going on for LIVE_PIECE_SECONDS is cut here, at
# the quietest moment of its last CUT_LOOK_SECONDS.
LIVE_PIECE_SECONDS = 6
CUT_LOOK_SECONDS = 1.5
PARTIAL_EVERY = 1.5
# Audio kept to transcribe the piece going on.
HISTORY_SECONDS = 12


class LiveStream:
    """One source: speech segments as they end, each with a voice number,
    and the speech going on now."""

    def __init__(self, stream: str, extractor, threshold: float):
        import sherpa_onnx

        config = sherpa_onnx.VadModelConfig()
        config.silero_vad.model = str(MODELS / "silero_vad.onnx")
        config.silero_vad.threshold = 0.4
        config.silero_vad.min_silence_duration = 0.4
        config.silero_vad.min_speech_duration = 0.25
        config.silero_vad.max_speech_duration = LIVE_PIECE_SECONDS
        config.sample_rate = RATE
        self.window = config.silero_vad.window_size
        self.vad = sherpa_onnx.VoiceActivityDetector(config, buffer_size_in_seconds=120)
        self.stream = stream
        self.extractor = extractor
        self.threshold = threshold
        self.pending = None
        self.voices = []  # [centroid, count]
        self.last = None
        # The last seconds heard, from sample `kept_from` on; and where the
        # speech going on now began (`None`: none is going on).
        self.kept = None
        self.kept_from = 0
        self.fed = 0
        self.speech_from = None
        # Speech before this sample was already given out as a piece cut
        # short (see `cut`).
        self.cut_at = 0

    def feed(self, samples) -> None:
        import numpy as np

        self.kept = samples if self.kept is None else np.concatenate([self.kept, samples])
        self.fed += len(samples)
        excess = len(self.kept) - HISTORY_SECONDS * RATE
        if excess > 0:
            self.kept = self.kept[excess:]
        self.kept_from = self.fed - len(self.kept)
        if self.pending is not None and len(self.pending):
            samples = np.concatenate([self.pending, samples])
        whole = len(samples) // self.window * self.window
        for i in range(0, whole, self.window):
            self.vad.accept_waveform(samples[i : i + self.window])
        self.pending = samples[whole:]
        if self.vad.is_speech_detected():
            if self.speech_from is None:
                # The detector is a little late: start just before.
                self.speech_from = max(self.kept_from, self.fed - len(samples) - int(0.3 * RATE))
        else:
            self.speech_from = None

    def speaking(self):
        """The speech going on now: (start, end, samples), or None."""
        if self.speech_from is None or self.kept is None:
            return None
        begin = max(self.speech_from, self.kept_from, self.cut_at)
        samples = self.kept[begin - self.kept_from :]
        # Shorter, Whisper guesses (often in capitals).
        if len(samples) < int(1.5 * RATE):
            return None
        return begin / RATE, self.fed / RATE, samples

    def cut(self):
        """The speech going on, when it has gone on for LIVE_PIECE_SECONDS:
        (stream, start, end, samples) up to its quietest moment lately,
        given out as a finished piece. The rest goes on as a new piece."""
        import numpy as np

        if self.speech_from is None or self.kept is None:
            return None
        begin = max(self.speech_from, self.kept_from, self.cut_at)
        if self.fed - begin < LIVE_PIECE_SECONDS * RATE:
            return None
        samples = self.kept[begin - self.kept_from :]
        # The quietest 20 ms of the last moments: not in a word.
        frame = RATE // 50
        look = int(CUT_LOOK_SECONDS * RATE) // frame * frame
        tail = samples[len(samples) - look :]
        energy = np.square(tail.reshape(-1, frame)).mean(axis=1)
        end = begin + len(samples) - look + int(np.argmin(energy)) * frame + frame // 2
        self.cut_at = self.speech_from = end
        return self.stream, begin / RATE, end / RATE, samples[: end - begin]

    def flush(self) -> None:
        if self.pending is not None and len(self.pending):
            self.vad.accept_waveform(self.pending)
            self.pending = None
        self.vad.flush()

    def segments(self):
        found = []
        while not self.vad.empty():
            seg = self.vad.front
            samples = seg.samples
            start, end = seg.start, seg.start + len(samples)
            self.vad.pop()
            # What follows the finished piece starts a new one.
            if self.speech_from is not None:
                self.speech_from = max(self.speech_from, end)
            # Only what was not given out already as a piece cut short.
            if end <= self.cut_at:
                continue
            if start < self.cut_at:
                samples = samples[self.cut_at - start :]
                start = self.cut_at
            found.append((self.stream, start / RATE, end / RATE, samples))
        return found

    def voice(self, samples) -> int:
        """The number of the voice in `samples`, a new one when unlike any
        heard so far. Very short pieces go to the last voice."""
        import numpy as np

        if len(samples) < int(1.5 * RATE):
            # Too short to tell voices apart: the last voice (or the first).
            return self.last if self.last is not None else 0
        found = fingerprint(self.extractor, samples)
        best, score = None, -1.0
        for i, (centroid, _) in enumerate(self.voices):
            similarity = float(np.dot(found, centroid))
            if similarity > score:
                best, score = i, similarity
        if best is not None and score >= self.threshold:
            centroid, count = self.voices[best]
            centroid = centroid * count + found
            self.voices[best] = [centroid / (np.linalg.norm(centroid) + 1e-9), count + 1]
        else:
            self.voices.append([found, 1])
            best = len(self.voices) - 1
        self.last = best
        return best


# Phrases Whisper writes for silence, music or noise. A short turn that is
# only one of these was not said.
INVENTED = {
    "thank you", "thanks", "thank you for watching", "thanks for watching",
    "thank you very much", "please subscribe", "bye", "you",
    "subtitles by the amaraorg community",
}


def invented(text: str, seconds: float) -> bool:
    plain = " ".join("".join(c for c in text.lower() if c.isalnum() or c.isspace()).split())
    return seconds < 1.5 and plain in INVENTED


def same_words(a: str, b: str) -> bool:
    import difflib

    a, b = a.lower().strip(), b.lower().strip()
    return bool(a and b) and difflib.SequenceMatcher(None, a, b).ratio() > 0.6


def live(folder: Path, args) -> None:
    import numpy as np
    import sherpa_onnx

    # SIGINT: transcribe what is left, then end (a meeting that stops).
    # SIGTERM: end now, leaving what is not transcribed yet (a session saved
    # raw: its chat had the live transcript; the sound is kept).
    stopping = []
    signal.signal(signal.SIGINT, lambda *_: stopping.append(True))
    signal.signal(signal.SIGTERM, lambda *_: os._exit(0))
    parent = os.getppid()
    extractor = embedder()
    transcriber = Transcriber(
        Path(args.model).expanduser(), args.device, [c.strip() for c in args.languages.split(",") if c.strip()]
    )
    tails = {stream: Tail(folder / name) for stream, name in STREAMS.items()}
    streams = {stream: LiveStream(stream, extractor, args.threshold) for stream in STREAMS}
    recent = []  # (stream, start, end, text) said lately, to spot echoes
    people: dict[tuple[str, int], str] = {}  # (stream, voice) -> "Person N"
    voices = Voices(args.profile, args.known)
    named: dict[tuple[str, int], int] = {}  # voice -> pieces when last named
    written = 0
    progress("live", 0, 0)
    partial_path = folder / "live-partial.json"
    partial_shown = False
    partial_at = 0.0

    def show_partial(turn) -> None:
        nonlocal partial_shown
        # Replaced whole, so a reader never sees half of it.
        part = partial_path.with_suffix(".part")
        part.write_text(json.dumps(turn or {}, ensure_ascii=False), encoding="utf-8")
        part.replace(partial_path)
        partial_shown = bool(turn)

    with open(folder / "live.jsonl", "a", encoding="utf-8") as out:
        while True:
            final = bool(stopping) or os.getppid() != parent
            ready = []
            for stream, tail in tails.items():
                samples = tail.read()
                if samples is not None:
                    streams[stream].feed(samples)
                if final:
                    streams[stream].flush()
                ready += streams[stream].segments()
                # Speech going on too long is not left to the detector.
                piece = None if final else streams[stream].cut()
                if piece is not None:
                    ready.append(piece)
            ready.sort(key=lambda seg: seg[1])
            for stream, start, end, samples in ready:
                if end - start < 0.4:
                    continue
                who = streams[stream].voice(samples)
                text, language = transcriber(np.ascontiguousarray(samples, dtype=np.float32))
                if not text or invented(text, end - start):
                    continue
                # The microphone hearing the call through the speakers says
                # the same words at the same time: keep the call's copy.
                if stream == "room" and any(
                    other == "call" and s < end + 1 and e > start - 1 and same_words(text, t)
                    for other, s, e, t in recent
                ):
                    continue
                recent = [r for r in recent if r[2] > start - 30] + [(stream, start, end, text)]
                turn = {
                    "stream": stream,
                    "start": start,
                    "end": end,
                    "speaker": people.setdefault((stream, who), f"Person {len(people) + 1}"),
                    "text": text,
                    "language": language,
                }
                out.write(json.dumps(turn, ensure_ascii=False) + "\n")
                out.flush()
                # Name voices once they have a few pieces, and again as they
                # grow (the fingerprint settles).
                heard = streams[stream].voices[who][1] if who < len(streams[stream].voices) else 0
                if heard in (1, 3, 8) and named.get((stream, who)) != heard:
                    named[(stream, who)] = heard
                    centres = {
                        people[(st, w)]: streams[st].voices[w][0]
                        for (st, w) in people
                        if w < len(streams[st].voices)
                    }
                    set_auto_names(folder, voices.names(centres))
                written += 1
                progress("live", written, written)
            if ready and partial_shown:
                show_partial(None)
            if final:
                if partial_shown:
                    show_partial(None)
                break
            # The words being said now, transcribed again as they go on.
            going = [(name, found) for name, found in ((n, s.speaking()) for n, s in streams.items()) if found]
            if not ready and going and time.monotonic() - partial_at >= PARTIAL_EVERY:
                partial_at = time.monotonic()
                # The longest speech going on, when both are.
                stream, (start, end, samples) = max(going, key=lambda item: len(item[1][2]))
                text, language = transcriber(np.ascontiguousarray(samples, dtype=np.float32))
                if text and not invented(text, end - start):
                    show_partial({"stream": stream, "start": start, "end": end, "text": text, "language": language})
            elif not going and partial_shown:
                show_partial(None)
            if not ready:
                time.sleep(0.25)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    r = sub.add_parser("record")
    r.add_argument("folder", type=Path)
    r.add_argument("--mic", help="PipeWire source name (default: the default microphone)")
    r.add_argument("--monitor", help="PipeWire output (sink) whose sound to record (default: the default output)")
    t = sub.add_parser("transcribe")
    t.add_argument("folder", type=Path)
    t.add_argument("--room-speakers", type=int, help="people heard by the microphone, if known")
    t.add_argument("--call-speakers", type=int, help="people heard through the computer, if known")
    t.add_argument("--threshold", type=float, default=0.5, help="voice grouping threshold when the count is unknown")
    t.add_argument("--languages", default="en,ur", help="expected languages; Urdu heard as Hindi is redone as Urdu")
    t.add_argument("--device", default="GPU")
    t.add_argument("--model", default=str(WHISPER), help="OpenVINO Whisper folder")
    t.add_argument("--progress", action="store_true", help="JSON progress lines on stdout (for Narrivy)")
    for p in (t,):
        p.add_argument("--profile", type=Path, help="the user's voice profile: their voice is labelled You")
        p.add_argument("--known", type=Path, help="remembered people's voices (named in earlier meetings)")
    v = sub.add_parser("live")
    v.add_argument("folder", type=Path)
    v.add_argument("--threshold", type=float, default=SAME_VOICE, help="how alike two pieces must sound to be one voice")
    v.add_argument("--languages", default="en,ur", help="expected languages; Urdu heard as Hindi is redone as Urdu")
    v.add_argument("--device", default="GPU")
    v.add_argument("--model", default=str(WHISPER), help="OpenVINO Whisper folder")
    v.add_argument("--profile", type=Path, help="the user's voice profile: their voice is labelled You")
    v.add_argument("--known", type=Path, help="remembered people's voices (named in earlier meetings)")
    m = sub.add_parser("remember")
    m.add_argument("folder", type=Path)
    m.add_argument("--speaker", required=True)
    m.add_argument("--name", required=True)
    m.add_argument("--known", type=Path, required=True)
    n = sub.add_parser("rename")
    n.add_argument("folder", type=Path)
    n.add_argument("pairs", nargs="+")
    args = parser.parse_args()
    folder = args.folder.expanduser()
    if args.command == "record":
        record(folder, args.mic, args.monitor)
    elif args.command == "transcribe":
        global PROGRESS
        PROGRESS = args.progress
        transcribe(folder, args)
    elif args.command == "live":
        PROGRESS = True
        live(folder, args)
    elif args.command == "remember":
        remember(folder, args.speaker, args.name, args.known.expanduser())
    else:
        rename(folder, args.pairs)


if __name__ == "__main__":
    os.environ.setdefault("OMP_NUM_THREADS", "4")
    main()
