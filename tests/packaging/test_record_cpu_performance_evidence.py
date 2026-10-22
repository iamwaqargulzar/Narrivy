from pathlib import Path
import struct
import sys

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import record_cpu_performance_evidence as performance


def test_protocol_v5_default_decode_has_explicit_empty_options_frame():
    samples = b"\x00\x00\x00\x00"
    request = performance.default_decode_request(7, 1, samples)

    assert struct.unpack("<H", request[4:6])[0] == 5
    assert request[6] == performance.OP_DECODE
    assert int.from_bytes(request[8:24], "little") == 7
    assert struct.unpack("<I", request[24:28])[0] == 1
    assert struct.unpack("<I", request[28:32])[0] == 0
    assert request[32:] == samples


def test_capability_payload_is_bounded_and_typed():
    capabilities = performance.parse_capabilities(bytes([0b1000, 0, 1, 0, 2]) + b"en")
    assert capabilities == {
        "streaming": False,
        "language_detection": False,
        "translation": False,
        "context_bias": True,
        "languages": ["en"],
        "translation_targets": [],
    }

    with pytest.raises(performance.BenchmarkError, match="trailing"):
        performance.parse_capabilities(bytes([0, 0, 0, 0, 1]))
