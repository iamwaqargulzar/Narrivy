#!/usr/bin/env python3
"""Exercise installed-mode offline model provisioning over Narrivy IPC.

On failure this helper prints exactly one bounded, sanitized, typed diagnosis
line to stderr::

    qualification_diagnosis={"schema_version":1,"reason":"<code>","facts":{...}}

Every fact is a closed-vocabulary lifecycle token, boolean, or integer copied
from daemon-owned typed status fields. Raw status payloads, paths, transcript
content, environment, and free-form messages never enter the summary.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import socket
import struct
import sys
import time
from collections.abc import Mapping
from pathlib import Path
from typing import Any

MAX_FRAME = 1_048_576
DIAGNOSIS_SCHEMA_VERSION = 1
DIAGNOSIS_PREFIX = "qualification_diagnosis="
MAX_TOKEN_CHARS = 64

WORKER_CAPABILITY = "asr.transcribe-cpp.worker="
WORKER_ARTIFACT_CAPABILITY = "asr.transcribe-cpp.worker-artifact="
MODEL_ARTIFACT_CAPABILITY = "asr.transcribe-cpp.model-artifact="
# The strict Linux composition advertises its two canonical built-in modes.
# The configured default mode is the one an id-less StartSession resolves to.
EXPECTED_PIPELINE_ID = "clean"
EXPECTED_PIPELINE_IDS = ("clean", "raw")
_TOKEN_RE = re.compile(r"[A-Za-z0-9_.:+-]+")

TERMINAL_PROVISION_STATES = {"ready", "cancelled", "timed_out", "failed"}


def resident_worker_ready(status: Mapping[str, Any]) -> bool:
    """Require live lifecycle readiness, never just binary integrity."""
    return (
        status.get("installed_model") == "ready"
        and capability_token(status, WORKER_CAPABILITY) == "ready"
    )


def capability_token(status: Mapping[str, Any], prefix: str) -> str | None:
    """Extract one closed-vocabulary capability value, or ``None``."""
    capabilities = status.get("capabilities")
    if not isinstance(capabilities, list):
        return None
    for capability in capabilities:
        if isinstance(capability, str) and capability.startswith(prefix):
            return _token(capability[len(prefix) :])
    return None


def _token(value: object) -> str | None:
    if not isinstance(value, str) or not value:
        return None
    if len(value) > MAX_TOKEN_CHARS or _TOKEN_RE.fullmatch(value) is None:
        return None
    return value


def status_facts(status: Mapping[str, Any]) -> dict[str, object]:
    """Reduce one daemon status payload to narrow, content-free facts."""
    facts: dict[str, object] = {
        "installed_model": _token(status.get("installed_model")),
        "worker": capability_token(status, WORKER_CAPABILITY),
        "worker_ready": resident_worker_ready(status),
        "worker_artifact": capability_token(status, WORKER_ARTIFACT_CAPABILITY),
        "model_artifact": capability_token(status, MODEL_ARTIFACT_CAPABILITY),
    }
    provisioning = status.get("model_provisioning")
    if isinstance(provisioning, dict):
        facts["provisioning_state"] = _token(provisioning.get("state"))
        facts["provisioning_publication"] = _token(provisioning.get("publication"))
        facts["provisioning_cleanup"] = _token(provisioning.get("cleanup"))
        facts["provisioning_failure"] = _token(provisioning.get("failure"))
    return {key: value for key, value in facts.items() if value is not None}


def diagnosis_line(reason: str, facts: Mapping[str, object]) -> str:
    payload = {
        "schema_version": DIAGNOSIS_SCHEMA_VERSION,
        "reason": _token(reason) or "unknown",
        "facts": {key: value for key, value in sorted(facts.items()) if value is not None},
    }
    return DIAGNOSIS_PREFIX + json.dumps(payload, sort_keys=True, separators=(",", ":"))


class QualificationError(RuntimeError):
    """One typed qualification failure with narrow content-free facts."""

    def __init__(
        self,
        reason: str,
        detail: str,
        facts: Mapping[str, object] | None = None,
    ) -> None:
        super().__init__(detail)
        self.reason = reason
        self.facts: dict[str, object] = dict(facts or {})


def _write_frame(connection: socket.socket, value: object) -> None:
    payload = json.dumps(value, separators=(",", ":")).encode()
    if len(payload) > MAX_FRAME:
        raise QualificationError("request_frame_too_large", "qualification request exceeded frame bound")
    connection.sendall(struct.pack("<I", len(payload)) + payload)


def _read_exact(connection: socket.socket, length: int) -> bytes:
    output = bytearray()
    while len(output) < length:
        chunk = connection.recv(length - len(output))
        if not chunk:
            raise QualificationError("ipc_closed_early", "qualification IPC ended early")
        output.extend(chunk)
    return bytes(output)


def _read_frame(connection: socket.socket) -> dict[str, Any]:
    length = struct.unpack("<I", _read_exact(connection, 4))[0]
    if length > MAX_FRAME:
        raise QualificationError("response_frame_too_large", "qualification response exceeded frame bound")
    value = json.loads(_read_exact(connection, length))
    if not isinstance(value, dict):
        raise QualificationError("response_not_object", "qualification response was not an object")
    return value


class Client:
    def __init__(self, socket_path: Path) -> None:
        self.connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.connection.settimeout(5)
        try:
            self.connection.connect(str(socket_path))
        except OSError as error:
            self.connection.close()
            raise QualificationError(
                "ipc_unavailable",
                f"qualification IPC connection failed ({error.__class__.__name__})",
            ) from None
        _write_frame(
            self.connection,
            {
                "protocol_major": 1,
                "protocol_minor": 5,
                "build_identity": "installed-qualification",
                "client_role": "qualification",
            },
        )
        handshake = _read_frame(self.connection)
        accepted = handshake.get("Accepted")
        if not isinstance(accepted, dict) or accepted.get("protocol_minor", 0) < 5:
            raise QualificationError("ipc_handshake_rejected", "daemon rejected qualification handshake")
        self.request_id = 0

    def close(self) -> None:
        self.connection.close()

    def request(self, kind: str, command: dict[str, Any]) -> dict[str, Any]:
        self.request_id += 1
        request_id = self.request_id
        _write_frame(
            self.connection,
            {
                "version_major": 1,
                "version_minor": 5,
                "kind": kind,
                "request_id": request_id,
                "session_id": None,
                "transaction_id": None,
                "event_seq": None,
                "payload": command,
            },
        )
        response = _read_frame(self.connection)
        if response.get("kind") != "response" or response.get("request_id") != request_id:
            raise QualificationError("ipc_correlation_failed", "daemon response correlation failed")
        payload = response.get("payload")
        if not isinstance(payload, dict):
            raise QualificationError("ipc_payload_invalid", "daemon response payload was invalid")
        if payload.get("result") == "rejected":
            raise QualificationError("ipc_request_rejected", "daemon rejected qualification command")
        return payload


def _status(client: Client) -> dict[str, Any]:
    response = client.request("request.get_status", {"command": "get_status"})
    if response.get("result") != "status" or not isinstance(response.get("payload"), dict):
        raise QualificationError("status_unavailable", "daemon did not return status")
    return response["payload"]


def pipeline_facts(client: Client) -> dict[str, object]:
    """Query the configured composition's canonical modes without retaining
    names, descriptions or any other content.

    Both built-in modes must appear exactly once with a typed enabled boolean;
    missing, duplicate or untyped mode entries fail closed, and the reported
    identity stays the fixed configured default rather than daemon text.
    """
    response = client.request("request.list_pipelines", {"command": "list_pipelines"})
    payload = response.get("payload")
    if response.get("result") != "pipelines" or not isinstance(payload, list):
        raise QualificationError("pipelines_unavailable", "daemon did not return pipelines")
    enabled: dict[str, bool] = {}
    for item in payload:
        if not isinstance(item, dict):
            continue
        pipeline_id = item.get("id")
        if pipeline_id not in EXPECTED_PIPELINE_IDS:
            continue
        if not isinstance(item.get("enabled"), bool) or pipeline_id in enabled:
            raise QualificationError("pipeline_invalid", "configured pipeline is absent or ambiguous")
        enabled[pipeline_id] = item["enabled"]
    if len(enabled) != len(EXPECTED_PIPELINE_IDS):
        raise QualificationError("pipeline_invalid", "configured pipeline is absent or ambiguous")
    return {
        "pipeline": EXPECTED_PIPELINE_ID,
        "pipeline_enabled": enabled[EXPECTED_PIPELINE_ID],
    }


def qualify(socket_path: Path, model_source: Path, timeout: float) -> dict[str, object]:
    if not math.isfinite(timeout) or timeout <= 0:
        raise QualificationError("invalid_timeout", "qualification timeout must be positive and finite")
    if not socket_path.is_absolute() or not model_source.is_absolute():
        raise QualificationError(
            "relative_qualification_path", "qualification paths must be absolute"
        )
    facts: dict[str, object] = {}
    client = Client(socket_path)
    try:
        before = _status(client)
        facts.update(status_facts(before))
        if before.get("installed_model") != "awaiting_model":
            raise QualificationError(
                "unexpected_startup_model",
                "installed daemon did not begin awaiting its model",
                facts,
            )
        started = client.request(
            "request.start_offline_model_provision",
            {"command": "start_offline_model_provision", "source_path": str(model_source)},
        )
        if started.get("result") != "offline_model_provision":
            raise QualificationError(
                "provision_not_accepted",
                "model provision did not return typed status",
                facts,
            )

        deadline = time.monotonic() + timeout
        final: dict[str, Any] | None = None
        while time.monotonic() < deadline:
            observed = _status(client)
            facts.update(status_facts(observed))
            candidate = observed.get("model_provisioning")
            if isinstance(candidate, dict) and candidate.get("state") in TERMINAL_PROVISION_STATES:
                final = candidate
                break
            time.sleep(0.05)
        if final is None:
            raise QualificationError(
                "provision_not_terminal",
                "installed model provisioning did not reach a terminal state",
                facts,
            )
        facts["provisioning_state"] = _token(final.get("state"))
        facts["provisioning_publication"] = _token(final.get("publication"))
        facts["provisioning_cleanup"] = _token(final.get("cleanup"))
        facts["provisioning_failure"] = _token(final.get("failure"))
        if final.get("state") != "ready":
            raise QualificationError(
                "provision_failed",
                "installed model provisioning did not reach ready",
                {key: value for key, value in facts.items() if value is not None},
            )
        after = _status(client)
        facts.update(status_facts(after))
        if after.get("installed_model") != "ready":
            raise QualificationError(
                "installed_model_not_ready",
                "installed model lifecycle did not reach ready",
                facts,
            )
        if not resident_worker_ready(after):
            raise QualificationError(
                "resident_worker_not_ready",
                "resident worker did not become ready",
                facts,
            )
        facts.update(pipeline_facts(client))
        if facts["pipeline_enabled"] is not True:
            raise QualificationError(
                "pipeline_not_enabled", "configured pipeline did not become enabled", facts,
            )
        return {
            "cleanup": final.get("cleanup"),
            "installed_model": after.get("installed_model"),
            "publication": final.get("publication"),
            "state": final.get("state"),
            "worker": capability_token(after, WORKER_CAPABILITY),
            "worker_ready": True,
            "pipeline": facts["pipeline"],
            "pipeline_enabled": True,
        }
    except QualificationError as error:
        if not error.facts:
            error.facts = {
                key: value for key, value in facts.items() if value is not None
            }
        raise
    finally:
        client.close()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True, type=Path)
    parser.add_argument("--model-source", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=20.0)
    values = parser.parse_args()
    try:
        result = qualify(values.socket, values.model_source, values.timeout)
    except QualificationError as error:
        print(diagnosis_line(error.reason, error.facts), file=sys.stderr)
        print(f"qualify_installed_model: {error}", file=sys.stderr)
        return 1
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(diagnosis_line("qualification_io_error", {}), file=sys.stderr)
        print(
            f"qualify_installed_model: qualification I/O failed ({error.__class__.__name__})",
            file=sys.stderr,
        )
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
