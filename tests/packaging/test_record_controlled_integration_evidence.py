from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import record_controlled_integration_evidence as controlled


def test_process_heavy_platform_suite_is_serialized():
    command = controlled.SUITES["platform-linux"]
    assert command[-2:] == ["--", "--test-threads=1"]
