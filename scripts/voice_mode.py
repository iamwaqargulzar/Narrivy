#!/usr/bin/env python3
"""Voice mode for Narrivy's AI assistant: a spoken conversation, locally.

Run by narrivyd (one process per voice-mode session). Everything stays on
this computer.

  * Listens on the microphone (the echo-cancelled one when Narrivy's echo
    canceller runs, so the assistant does not hear itself).
  * Silero VAD finds speech; about a second of silence ends the turn.
  * Whisper (OpenVINO) transcribes while you speak, every ~2 s, and once
    more as soon as you pause, so the words are ready when the turn ends.
  * The reply arrives in pieces on stdin; each sentence is spoken as soon as
    it is complete (Kokoro, or Piper for Urdu, through sherpa-onnx on the
    CPU), the next one prepared while the current one plays.
  * Speaking while it talks interrupts it (after ~0.25 s of speech), or is
    kept for after the reply, as the user chose.
  * "Say that again" replays the last reply without asking the model.

stdin: one JSON object per line from the daemon:
  {"op":"say","text":"..."}   a piece of the reply
  {"op":"end"}                the reply is complete
  {"op":"stop"}               stop speaking now (the reply was cancelled)
  {"op":"settings", ...}      speed, push_to_talk, interrupt, sounds, muted
  {"op":"ptt","down":bool}    push-to-talk key
  {"op":"quit"}
stdout: one JSON object per line for the daemon:
  {"ev":"ready"} {"ev":"state","state":...} {"ev":"partial","text":...}
  {"ev":"heard","text":...,"language":...} {"ev":"interrupt"}
  {"ev":"repeat"} {"ev":"timing",...} {"ev":"error","code":...}
stderr carries messages for people (the daemon discards it); no line
anywhere is written to disk.
"""

import argparse
import json
import os
import queue
import re
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

RATE = 16000
HOME = Path.home()
VAD_MODEL = HOME / ".cache/narrivy/diarization/silero_vad.onnx"
TTS_DIR = HOME / ".cache/narrivy/tts"
KOKORO = TTS_DIR / "kokoro-multi-lang-v1_0"
URDU = TTS_DIR / "vits-piper-ur_PK-fasih-medium"
# Silence that ends a turn, and speech that interrupts the reply.
TURN_SILENCE = 1.0
BARGE_IN = 0.1
# Transcribe while speaking this often, and once at this much silence.
ROLLING = 2.0
EARLY = 0.0
URDU_LETTERS = re.compile(r"[؀-ۿ]")
REPEAT = re.compile(
    r"^\W*(please\s+)?(say (that|it) again|repeat( that| it)?|come again|pardon( me)?|"
    r"what did you say|دوبارہ\s*(کہیں|کہو|بولیں|بولو)|پھر سے\s*(کہیں|کہو|بولیں|بولو))\W*$",
    re.IGNORECASE,
)
INVENTED = {"thank you", "thanks", "thank you for watching", "you", "bye", "thanks for watching"}

_out_lock = threading.Lock()


def emit(**event) -> None:
    with _out_lock:
        sys.stdout.write(json.dumps(event, ensure_ascii=False) + "\n")
        sys.stdout.flush()


def note(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


# ------------------------------------------------------------------ speech


class Transcriber:
    def __init__(self, model: Path, device: str, languages: list[str]):
        import openvino_genai as og

        self.pipe = og.WhisperPipeline(str(model), device)
        self.languages = languages
        self.lock = threading.Lock()

    def run(self, audio, language=None):
        config = self.pipe.get_generation_config()
        config.task = "transcribe"
        config.language = f"<|{language}|>" if language else None
        config.return_timestamps = False
        config.max_new_tokens = 220
        result = self.pipe.generate(audio.tolist(), config)
        detected = (getattr(result, "language", None) or language or "").strip("<|>")
        return result.texts[0].strip(), detected

    def __call__(self, audio):
        with self.lock:
            text, detected = self.run(audio)
            if detected == "hi" and "ur" in self.languages:
                text, detected = self.run(audio, "ur")
            elif self.languages and detected and detected not in self.languages:
                text, detected = self.run(audio, self.languages[0])
        plain = " ".join("".join(c for c in text.lower() if c.isalnum() or c.isspace()).split())
        if len(audio) < 1.5 * RATE and plain in INVENTED:
            return "", detected
        return text, detected


# ------------------------------------------------------------------ voice


class Voice:
    """Text to speech: Kokoro for English, Piper for Urdu."""

    def __init__(self, speaker: int, threads: int):
        import sherpa_onnx

        self.speaker = speaker
        self.english = None
        self.urdu = None
        if (KOKORO / "model.onnx").is_file():
            config = sherpa_onnx.OfflineTtsConfig(
                model=sherpa_onnx.OfflineTtsModelConfig(
                    kokoro=sherpa_onnx.OfflineTtsKokoroModelConfig(
                        model=str(KOKORO / "model.onnx"),
                        voices=str(KOKORO / "voices.bin"),
                        tokens=str(KOKORO / "tokens.txt"),
                        data_dir=str(KOKORO / "espeak-ng-data"),
                        dict_dir=str(KOKORO / "dict"),
                        lexicon=f"{KOKORO / 'lexicon-us-en.txt'},{KOKORO / 'lexicon-zh.txt'}",
                    ),
                    num_threads=threads,
                ),
                max_num_sentences=1,
            )
            self.english = sherpa_onnx.OfflineTts(config)
        urdu_model = next(iter(sorted(URDU.glob("*.onnx"))), None) if URDU.is_dir() else None
        if urdu_model:
            config = sherpa_onnx.OfflineTtsConfig(
                model=sherpa_onnx.OfflineTtsModelConfig(
                    vits=sherpa_onnx.OfflineTtsVitsModelConfig(
                        model=str(urdu_model),
                        tokens=str(URDU / "tokens.txt"),
                        data_dir=str(URDU / "espeak-ng-data"),
                    ),
                    num_threads=threads,
                ),
                max_num_sentences=1,
            )
            self.urdu = sherpa_onnx.OfflineTts(config)
        if self.english is None and self.urdu is None:
            raise RuntimeError("voice_model_missing")

    def speak(self, text: str, speed: float):
        """(samples, rate) for `text`."""
        import numpy as np

        urdu = bool(URDU_LETTERS.search(text))
        engine = (self.urdu if urdu else self.english) or self.english or self.urdu
        sid = 0 if engine is self.urdu else self.speaker
        audio = engine.generate(text, sid=sid, speed=speed)
        return np.asarray(audio.samples, dtype=np.float32), audio.sample_rate


def tone(rate: int, notes: list[tuple[float, float]], level=0.12):
    """A short soft cue: (frequency, seconds) notes with gentle edges."""
    import numpy as np

    parts = []
    for freq, seconds in notes:
        t = np.arange(int(rate * seconds)) / rate
        wave = np.sin(2 * np.pi * freq * t) * level
        edge = min(len(t) // 4, int(rate * 0.015))
        if edge:
            ramp = np.linspace(0, 1, edge)
            wave[:edge] *= ramp
            wave[-edge:] *= ramp[::-1]
        parts.append(wave.astype(np.float32))
    return np.concatenate(parts)


class Player:
    """Plays audio clips one after another; `stop()` cuts off at once."""

    def __init__(self, output=None):
        self.output = output
        self.clips: queue.Queue = queue.Queue()
        self.process = None
        self.rate = None
        self.lock = threading.Lock()
        self.generation = 0
        self.playing = threading.Event()
        self.started_at = None
        threading.Thread(target=self._run, daemon=True).start()

    def play(self, samples, rate, on_start=None):
        self.clips.put((self.generation, samples, rate, on_start))

    def idle(self) -> bool:
        return self.clips.empty() and not self.playing.is_set()

    def stop(self):
        with self.lock:
            self.generation += 1
            while True:
                try:
                    self.clips.get_nowait()
                except queue.Empty:
                    break
            self._close()

    def _close(self):
        if self.process:
            try:
                self.process.kill()
                self.process.wait(timeout=2)
            except Exception:
                pass
            self.process = None
            self.rate = None

    def _open(self, rate):
        if self.process and self.rate == rate and self.process.poll() is None:
            return
        self._close()
        target = ["--target", self.output] if self.output else []
        self.process = subprocess.Popen(
            ["pw-play", "--raw", "--rate", str(rate), "--channels", "1", "--format", "f32",
             "--latency", "50ms", *target, "-"],
            stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        self.rate = rate

    def _run(self):
        while True:
            generation, samples, rate, on_start = self.clips.get()
            if generation != self.generation:
                continue
            self.playing.set()
            try:
                with self.lock:
                    if generation != self.generation:
                        continue
                    self._open(rate)
                if on_start:
                    on_start()
                # Write in small pieces so stop() takes effect quickly, and
                # pace them to real time so the queue reflects what is heard.
                step = rate // 20
                begin = time.monotonic()
                for i in range(0, len(samples), step):
                    if generation != self.generation:
                        break
                    process = self.process
                    if process is None or process.stdin is None:
                        break
                    try:
                        process.stdin.write(samples[i : i + step].tobytes())
                        process.stdin.flush()
                    except (BrokenPipeError, ValueError, OSError):
                        break
                    ahead = begin + (i + step) / rate - time.monotonic() - 0.15
                    if ahead > 0:
                        time.sleep(ahead)
                if generation == self.generation:
                    # Let the last piece finish sounding.
                    time.sleep(0.15)
            finally:
                if self.clips.empty():
                    self.playing.clear()


# ------------------------------------------------------------------ text


SENTENCE_END = re.compile(r"(?<=[.!?؟۔])[\"')\]]*\s+|\n+")


def speakable(text: str) -> str:
    """Markdown and symbols the voice would read out, removed."""
    text = re.sub(r"```.*?```", " ", text, flags=re.S)
    text = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"^\s*(#+|[-*•]|\d+[.)])\s+", "", text, flags=re.M)
    text = text.replace("**", "").replace("__", "").replace("`", "")
    text = re.sub(r"(?<!\w)[*_](?!\s)|(?<!\s)[*_](?!\w)", "", text)
    return " ".join(text.split())


class Sentences:
    """Collects reply pieces and hands out whole sentences."""

    def __init__(self):
        self.buffer = ""
        self.thinking = False

    def add(self, piece: str) -> list[str]:
        self.buffer += piece
        # A thinking block (if the model writes one) is not spoken.
        while True:
            if self.thinking:
                end = self.buffer.find("</think>")
                if end < 0:
                    return []
                self.buffer = self.buffer[end + len("</think>") :]
                self.thinking = False
            start = self.buffer.find("<think>")
            if start < 0:
                break
            self.buffer = self.buffer[:start] + self.buffer[start + len("<think>") :]
            self.thinking = True
        out = []
        while True:
            match = SENTENCE_END.search(self.buffer)
            if not match:
                break
            sentence, self.buffer = self.buffer[: match.end()], self.buffer[match.end() :]
            if speakable(sentence):
                out.append(speakable(sentence))
        # A very long run with no full stop is spoken at a comma.
        if len(self.buffer) > 220:
            cut = max(self.buffer.rfind(", "), self.buffer.rfind("، "))
            if cut > 60:
                sentence, self.buffer = self.buffer[: cut + 1], self.buffer[cut + 2 :]
                out.append(speakable(sentence))
        return out

    def rest(self) -> list[str]:
        rest, self.buffer = ("" if self.thinking else self.buffer), ""
        self.thinking = False
        return [speakable(rest)] if speakable(rest) else []


# ------------------------------------------------------------------ main


class VoiceMode:
    def __init__(self, args):
        import numpy as np
        import sherpa_onnx

        self.np = np
        self.args = args
        self.speed = args.speed
        self.push_to_talk = args.push_to_talk
        self.interrupt = args.interrupt
        self.sounds = args.sounds
        self.muted = False
        self.ptt_down = False
        self.barge_in = args.echo_cancelled

        config = sherpa_onnx.VadModelConfig()
        config.silero_vad.model = str(VAD_MODEL)
        config.silero_vad.threshold = 0.5
        config.silero_vad.min_silence_duration = TURN_SILENCE
        config.silero_vad.min_speech_duration = 0.25
        config.silero_vad.max_speech_duration = 30
        config.sample_rate = RATE
        self.window = config.silero_vad.window_size
        self.vad = sherpa_onnx.VoiceActivityDetector(config, buffer_size_in_seconds=60)
        # A quick detector for "speaking right now": pauses (for the early
        # transcript) and talking over the reply. The one above keeps
        # reporting speech through the whole pause that ends a turn.
        quick = sherpa_onnx.VadModelConfig()
        quick.silero_vad.model = str(VAD_MODEL)
        quick.silero_vad.threshold = 0.5
        quick.silero_vad.min_silence_duration = 0.2
        quick.silero_vad.min_speech_duration = 0.15
        quick.silero_vad.max_speech_duration = 30
        quick.sample_rate = RATE
        self.quick = sherpa_onnx.VoiceActivityDetector(quick, buffer_size_in_seconds=60)

        languages = [c.strip() for c in args.languages.split(",") if c.strip()]
        self.transcriber = Transcriber(Path(args.model), args.device, languages)
        self.voice = Voice(args.speaker, args.threads)
        self.player = Player(args.output)
        self.cue_rate = 24000
        self.cue_listen = tone(self.cue_rate, [(660, 0.07), (880, 0.09)])
        self.cue_heard = tone(self.cue_rate, [(880, 0.06), (660, 0.08)], level=0.08)

        self.state = None
        self.reply = Sentences()
        self.reply_open = False
        self.to_speak: queue.Queue = queue.Queue()
        # Sentences of the current reply not yet handed to the player.
        self.pending = 0
        self.pending_lock = threading.Lock()
        self.spoken: list = []  # the last reply's audio, for "say that again"
        self.last_reply: list = []
        self.speech_gen = 0

        # The utterance being heard.
        self.utterance = []
        self.recent = []  # the last half second, kept for when speech starts
        self.ended_at = None
        self.speaking_for = 0.0
        self.silent_for = 0.0
        self.since_rolling = 0.0
        self.early_text = None  # (samples heard, text, language)
        self.early_sent = False
        self.asr_jobs: queue.Queue = queue.Queue()
        self.asr_busy = False
        self.timing = {}

        self.audio: queue.Queue = queue.Queue()
        self.commands: queue.Queue = queue.Queue()
        threading.Thread(target=self._synthesize, daemon=True).start()
        threading.Thread(target=self._transcribe_jobs, daemon=True).start()

    # -- state

    def set_state(self, state):
        if state != self.state:
            self.state = state
            emit(ev="state", state=state)

    def listening_state(self):
        if self.muted:
            return "muted"
        return "listening"

    # -- audio in

    def capture(self):
        target = self.args.mic or "@DEFAULT_AUDIO_SOURCE@"
        process = subprocess.Popen(
            ["pw-record", "--raw", "--rate", str(RATE), "--channels", "1", "--format", "f32",
             "--target", target, "-"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        )
        self.recorder = process
        chunk = self.window * 4 * 2  # 64 ms
        while True:
            data = process.stdout.read(chunk)
            if not data:
                self.audio.put(None)
                return
            self.audio.put(self.np.frombuffer(data, dtype=self.np.float32))

    def commands_in(self):
        for line in sys.stdin:
            try:
                self.commands.put(json.loads(line))
            except json.JSONDecodeError:
                continue
        self.commands.put({"op": "quit"})

    # -- the reply

    def _synthesize(self):
        """Turns sentences into audio, at most two ahead of what plays."""
        while True:
            generation, sentence, first = self.to_speak.get()
            while self.player.clips.qsize() >= 2 and generation == self.speech_gen:
                time.sleep(0.05)
            if generation != self.speech_gen:
                continue
            started = time.monotonic()
            try:
                samples, rate = self.voice.speak(sentence, self.speed)
            except Exception as error:  # a sentence the voice cannot say
                note(f"voice failed: {type(error).__name__}")
                self.sentence_done(generation)
                continue
            if generation != self.speech_gen:
                continue
            self.spoken.append((samples, rate))
            on_start = None
            if first:
                synth = time.monotonic() - started

                def on_start(synth=synth):
                    heard = self.timing.get("heard_at")
                    if heard:
                        emit(
                            ev="timing",
                            asr_ms=self.timing.get("asr_ms", 0),
                            reply_ms=int((self.timing.get("first_text_at", heard) - heard) * 1000),
                            voice_ms=int(synth * 1000),
                            total_ms=int((time.monotonic() - self.timing.get("ended_at", heard)) * 1000),
                        )
                    self.set_state("speaking")

            self.player.play(samples, rate, on_start)
            self.sentence_done(generation)

    def sentence_done(self, generation):
        with self.pending_lock:
            if generation == self.speech_gen:
                self.pending = max(0, self.pending - 1)

    def queue_sentence(self, sentence: str):
        first = not self.spoken and self.to_speak.empty() and not self.timing.get("queued")
        self.timing["queued"] = True
        with self.pending_lock:
            self.pending += 1
        self.to_speak.put((self.speech_gen, sentence, first))

    def stop_speaking(self):
        with self.pending_lock:
            self.speech_gen += 1
            self.pending = 0
        while True:
            try:
                self.to_speak.get_nowait()
            except queue.Empty:
                break
        self.player.stop()
        self.reply = Sentences()
        self.reply_open = False

    # -- transcription

    def _transcribe_jobs(self):
        while True:
            kind, samples, length = self.asr_jobs.get()
            started = time.monotonic()
            try:
                text, language = self.transcriber(samples)
            except Exception as error:
                note(f"transcription failed: {type(error).__name__}")
                text, language = "", ""
            self.commands.put({"op": "_asr", "kind": kind, "text": text, "language": language,
                               "length": length, "ms": int((time.monotonic() - started) * 1000)})

    def heard_so_far(self):
        return self.np.concatenate(self.utterance) if self.utterance else self.np.zeros(0, self.np.float32)

    def finish_turn(self, samples):
        """The turn ended: use the early transcript when nothing was said
        after it, else transcribe the whole turn now."""
        # The turn's words ended when the pause that ends it began.
        self.ended_at = time.monotonic() - TURN_SILENCE
        self.utterance = []
        self.speaking_for = self.silent_for = self.since_rolling = 0.0
        early = self.early_text
        self.early_text = None
        self.early_sent = False
        # Any speech after the early transcript cleared it.
        if early:
            self.turn_heard(early[1], early[2], 0)
        else:
            self.set_state("thinking")
            self.asr_jobs.put(("final", samples, len(samples)))

    def turn_heard(self, text, language, asr_ms):
        text = text.strip()
        if not text:
            self.set_state(self.listening_state() if self.player.idle() else "speaking")
            return
        if REPEAT.match(text) and self.last_reply:
            self.stop_speaking()
            emit(ev="repeat")
            self.set_state("speaking")
            for samples, rate in self.last_reply:
                self.player.play(samples, rate)
            self.spoken = list(self.last_reply)
            return
        if self.sounds:
            self.player.play(self.cue_heard, self.cue_rate)
        self.timing = {"heard_at": time.monotonic(), "ended_at": self.ended_at or time.monotonic(), "asr_ms": asr_ms}
        self.spoken = []
        self.reply = Sentences()
        self.reply_open = True
        if self.state != "speaking":
            self.set_state("thinking")
        emit(ev="heard", text=text, language=language or "")

    # -- the loop

    def handle(self, command) -> bool:
        op = command.get("op")
        if op == "quit":
            return False
        if op == "say" and self.reply_open:
            self.timing.setdefault("first_text_at", time.monotonic())
            for sentence in self.reply.add(command.get("text", "")):
                self.queue_sentence(sentence)
        elif op == "end":
            for sentence in self.reply.rest():
                self.queue_sentence(sentence)
            self.reply_open = False
            self.after_reply = True
        elif op == "stop":
            self.stop_speaking()
            self.set_state(self.listening_state())
        elif op == "settings":
            if "speed" in command:
                self.speed = min(1.75, max(1.0, float(command["speed"])))
            if "push_to_talk" in command:
                self.push_to_talk = bool(command["push_to_talk"])
            if "interrupt" in command and command["interrupt"] in ("stop", "queue"):
                self.interrupt = command["interrupt"]
            if "sounds" in command:
                self.sounds = bool(command["sounds"])
            if "voice" in command:
                self.voice.speaker = int(command["voice"])
            if "muted" in command:
                self.muted = bool(command["muted"])
                if self.muted:
                    self.drop_utterance()
                if self.state in ("listening", "muted"):
                    self.set_state(self.listening_state())
        elif op == "ptt":
            down = bool(command.get("down"))
            if self.push_to_talk and self.ptt_down and not down:
                # Releasing the key ends the turn at once.
                self.vad.flush()
                self.collect_segments(force=True)
            self.ptt_down = down
            if down and self.interrupt == "stop" and not self.player.idle():
                self.stop_speaking()
                emit(ev="interrupt")
        elif op == "_asr":
            kind = command["kind"]
            if kind == "final":
                self.turn_heard(command["text"], command["language"], command["ms"])
            elif kind == "early":
                if self.utterance and self.early_sent:
                    self.early_text = (command["length"], command["text"], command["language"])
                    if command["text"]:
                        emit(ev="partial", text=command["text"])
            elif kind == "rolling":
                self.asr_busy = False
                if self.utterance and command["text"]:
                    emit(ev="partial", text=command["text"])
        return True

    def drop_utterance(self):
        self.early_sent = False
        self.vad.reset()
        self.utterance = []
        self.early_text = None
        self.speaking_for = self.silent_for = self.since_rolling = 0.0

    def collect_segments(self, force=False):
        while not self.vad.empty():
            segment = self.np.asarray(self.vad.front.samples, dtype=self.np.float32)
            self.vad.pop()
            if len(segment) >= int(0.3 * RATE):
                self.finish_turn(segment)
            else:
                self.drop_utterance()
        if force and self.utterance:
            samples = self.heard_so_far()
            self.drop_utterance()
            if len(samples) >= int(0.3 * RATE):
                self.finish_turn(samples)

    def feed(self, samples):
        listening = not self.muted and (not self.push_to_talk or self.ptt_down)
        speaking = not self.player.idle()
        # Without echo cancelling, the microphone would hear the reply.
        if speaking and not self.barge_in and not self.push_to_talk:
            listening = False
        if not listening:
            return
        seconds = len(samples) / RATE
        for i in range(0, len(samples) - self.window + 1, self.window):
            piece = samples[i : i + self.window]
            self.vad.accept_waveform(piece)
            self.quick.accept_waveform(piece)
        while not self.quick.empty():
            self.quick.pop()
        if self.quick.is_speech_detected():
            if not self.utterance:
                # Speech is detected a little after it starts.
                self.utterance.extend(self.recent)
            self.utterance.append(samples)
            self.speaking_for += seconds
            self.silent_for = 0.0
            self.since_rolling += seconds
            self.early_text = None
            self.early_sent = False
            if speaking and self.speaking_for >= BARGE_IN and self.interrupt == "stop":
                self.stop_speaking()
                emit(ev="interrupt")
                speaking = False
            if not speaking and self.state != "hearing":
                self.set_state("hearing")
            if self.since_rolling >= ROLLING and not self.asr_busy:
                self.since_rolling = 0.0
                self.asr_busy = True
                heard = self.heard_so_far()
                self.asr_jobs.put(("rolling", heard, len(heard)))
        elif self.utterance:
            self.utterance.append(samples)
            self.silent_for += seconds
            if self.silent_for > EARLY and not self.early_sent:
                # A pause: transcribe now, so the words are ready if the
                # turn ends here.
                self.early_sent = True
                heard = self.heard_so_far()
                self.asr_jobs.put(("early", heard, len(heard)))
        self.recent = (self.recent + [samples])[-8:]
        self.collect_segments()
        if not self.utterance and self.state == "hearing":
            self.set_state(self.listening_state() if self.player.idle() else "speaking")

    def run(self):
        threading.Thread(target=self.capture, daemon=True).start()
        threading.Thread(target=self.commands_in, daemon=True).start()
        emit(ev="ready", english=self.voice.english is not None, urdu=self.voice.urdu is not None)
        self.set_state(self.listening_state())
        if self.sounds:
            self.player.play(self.cue_listen, self.cue_rate)
        self.after_reply = False
        parent = os.getppid()
        last_check = time.monotonic()
        while True:
            try:
                while True:
                    if not self.handle(self.commands.get_nowait()):
                        return
            except queue.Empty:
                pass
            try:
                samples = self.audio.get(timeout=0.05)
            except queue.Empty:
                samples = None
            if samples is None and self.recorder.poll() is not None:
                emit(ev="error", code="voice_microphone_lost")
                return
            if samples is not None and len(samples):
                self.feed(samples)
            # The reply has been spoken: listen again.
            if (
                self.after_reply
                and self.pending == 0
                and self.player.idle()
                and not self.reply_open
            ):
                self.after_reply = False
                self.last_reply = list(self.spoken)
                if self.state in ("speaking", "thinking"):
                    self.set_state(self.listening_state())
                    if self.sounds and self.last_reply:
                        self.player.play(self.cue_listen, self.cue_rate)
            if time.monotonic() - last_check > 2:
                last_check = time.monotonic()
                if os.getppid() != parent:
                    return


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--model", required=True, help="Whisper model directory (OpenVINO)")
    parser.add_argument("--device", default="GPU")
    parser.add_argument("--mic", default=None, help="PipeWire source to listen to")
    parser.add_argument("--output", default=None, help="PipeWire sink to speak through")
    parser.add_argument("--echo-cancelled", action="store_true",
                        help="the source is echo-cancelled: allow interrupting the reply by voice")
    parser.add_argument("--languages", default="en,ur")
    parser.add_argument("--speed", type=float, default=1.0)
    parser.add_argument("--speaker", type=int, default=3, help="Kokoro voice number")
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--push-to-talk", action="store_true")
    parser.add_argument("--interrupt", choices=("stop", "queue"), default="stop")
    parser.add_argument("--sounds", action="store_true")
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, lambda *_: os._exit(0))
    try:
        mode = VoiceMode(args)
    except RuntimeError as error:
        emit(ev="error", code=str(error))
        return 1
    except Exception as error:
        note(f"voice mode failed to start: {type(error).__name__}: {error}")
        emit(ev="error", code="voice_start_failed")
        return 1
    try:
        mode.run()
    finally:
        mode.player.stop()
        recorder = getattr(mode, "recorder", None)
        if recorder:
            recorder.kill()
    return 0


if __name__ == "__main__":
    sys.exit(main())
