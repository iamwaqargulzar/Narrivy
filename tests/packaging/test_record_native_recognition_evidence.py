from pathlib import Path
import sys

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import record_native_recognition_evidence as recognition


def test_recognition_operations_checks_capabilities_and_rejections(monkeypatch):
    calls: list[list[str]] = []

    def fake_run(_self, argv, *, env, timeout, cwd=None):
        del env, timeout, cwd
        command = list(argv)
        calls.append(command)
        if command[-1] == "status":
            return recognition.qualifier.CommandResult(
                0,
                b"capabilities=asr.recognition-settings=available,asr.languages=en,"
                b"asr.context-bias=available,asr.language-detection=unavailable,"
                b"asr.translation=unavailable\nno active session\n",
                b"",
            )
        if command[-4:] == ["--language", "en", "--context-hint", "Narrivy"]:
            return recognition.qualifier.CommandResult(0, b"accepted", b"")
        if any(flag in command for flag, _value in recognition.REJECTED_SETTINGS):
            return recognition.qualifier.CommandResult(1, b"", b"request failed: invalid_request")
        return recognition.qualifier.CommandResult(0, b"accepted", b"")

    monkeypatch.setattr(recognition.qualifier.HostOperations, "run", fake_run)
    operations = recognition.RecognitionOperations()
    outcome = operations.run(
        ["/tmp/narrivy-cli", "--socket", "/tmp/socket", "start", "--pipeline", "clean"],
        env={},
        timeout=5,
        cwd=Path("/tmp"),
    )

    assert outcome.returncode == 0
    assert operations.capabilities_verified is True
    assert operations.rejected == len(recognition.REJECTED_SETTINGS)
    assert calls[-1][-4:] == ["--language", "en", "--context-hint", "Narrivy"]


def test_recognition_operations_fails_if_an_unsupported_setting_is_admitted(monkeypatch):
    def fake_run(_self, argv, *, env, timeout, cwd=None):
        del env, timeout, cwd
        command = list(argv)
        if command[-1] == "status":
            return recognition.qualifier.CommandResult(
                0,
                b"asr.recognition-settings=available asr.languages=en "
                b"asr.context-bias=available asr.language-detection=unavailable "
                b"asr.translation=unavailable no active session",
                b"",
            )
        return recognition.qualifier.CommandResult(0, b"accepted", b"")

    monkeypatch.setattr(recognition.qualifier.HostOperations, "run", fake_run)
    with pytest.raises(recognition.qualifier.StepFailure, match="was not rejected"):
        recognition.RecognitionOperations().run(
            ["/tmp/narrivy-cli", "--socket", "/tmp/socket", "start", "--pipeline", "clean"],
            env={},
            timeout=5,
        )
