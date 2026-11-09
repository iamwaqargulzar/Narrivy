#!/usr/bin/env python3
"""Print a sanitized installed-model/resident-worker status summary.

Read-only: this probe never provisions, starts, or stops a model or worker. It
is used by ``scripts/qualify_candidate.py`` inside its disposable container and
may be run manually against an explicitly selected absolute socket path.

Output is a narrow, content-free fact object: the installed-model lifecycle
token, the live worker lifecycle token, the artifact-binding token, and the
boolean readiness decision. Raw status payloads, paths, and capabilities
outside this closed set never leave the container.
The configured composition's fixed mode identities are queried independently
over ListPipelines; only the fixed configured default identity and its typed
enabled boolean enter the summary, never any advertised name.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from qualify_installed_model import Client, QualificationError, _status, status_facts, pipeline_facts


def summarize(status: Mapping[str, Any]) -> dict[str, object]:
    """Reduce one typed status payload to stable, content-free facts."""

    facts = status_facts(status)
    summary = {
        "installed_model": facts.get("installed_model"),
        "worker_ready": facts.get("worker_ready", False),
        "worker": facts.get("worker"),
        "worker_artifact": facts.get("worker_artifact"),
        "model_artifact": facts.get("model_artifact"),
    }
    return {key: value for key, value in summary.items() if value is not None}


def probe(socket_path: Path) -> dict[str, object]:
    if not socket_path.is_absolute():
        raise QualificationError("relative_socket_path", "status socket path must be absolute")
    client = Client(socket_path)
    try:
        summary = summarize(_status(client))
        summary.update(pipeline_facts(client))
        return summary
    finally:
        client.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True, type=Path)
    values = parser.parse_args(argv)
    try:
        summary = probe(values.socket)
    except (QualificationError, OSError, ValueError) as error:
        print(f"probe_installed_status: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
