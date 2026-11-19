#!/usr/bin/env python3
"""Stage Narrivy model packs for upload to the narrivy Hugging Face org.

Everything Narrivy publishes lives in one repository, `narrivy/store`, laid
out by kind and runtime:

    models/gguf/<pack>/          transcribe.cpp models (CPU or Vulkan GPU)
    models/openvino/<pack>/      OpenVINO models (NPU, GPU or CPU)
    runtimes/openvino/<pack>/    the OpenVINO add-on's libraries
    modules/                     add-on modules, as they are published

The tree is staged in `<staging>/store`. Reads `packs.json` (each pack's
pinned upstream source, its folder in the Store, and the SHA-256 and size of
every file), finds the files on this computer, verifies every byte against
those pins, and writes one folder per pack holding:

- the model files (hard links, so nothing is copied or changed);
- `model-manifest.json`, valid against spec/schemas/model-manifest.schema.json;
- `README.md`, the model card, stating the quantization up front.

It also writes each manifest to `tools/reference-models/manifests/` so the
repository records exactly what is published, the Store's index README
(listing every model and runtime pack staged, including the runtime pack
from `scripts/build_openvino_runtime_pack.py`), and `upload.sh`. Nothing is
uploaded and no network is used: publishing is the owner's step.

A pack whose local files are missing or do not match its pins is reported
and skipped; it is never staged from unverified bytes.

Usage:
    python3 tools/reference-models/build_model_packs.py [--staging DIR] [--check]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

import jsonschema

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SCHEMA = ROOT / "spec/schemas/model-manifest.schema.json"
DEFAULT_STAGING = Path.home() / ".cache/narrivy/hf-staging"
STORE_REPO = "narrivy/store"
STORE_DIR = "store"
HF_HUB = Path(os.environ.get("HF_HOME", Path.home() / ".cache/huggingface")) / "hub"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def local_dir(local: dict) -> Path:
    if local["kind"] == "hf_cache":
        repo = "models--" + local["repo"].replace("/", "--")
        return HF_HUB / repo / "snapshots" / local["revision"]
    if local["kind"] == "directory":
        return Path(local["path"]).expanduser()
    raise ValueError(f"unknown local kind {local['kind']!r}")


def verify(pack: dict) -> list[str]:
    """Return the problems found; an empty list means every file matches."""
    base = local_dir(pack["local"])
    problems = []
    for entry in pack["files"]:
        path = base / entry["path"]
        if not path.is_file():
            problems.append(f"missing {entry['path']}")
            continue
        real = path.resolve()
        if real.stat().st_size != entry["size_bytes"]:
            problems.append(f"size differs {entry['path']}")
        elif sha256(real) != entry["sha256"]:
            problems.append(f"sha256 differs {entry['path']}")
    return problems


def manifest(pack: dict, revision: str) -> dict:
    devices = ", ".join(pack["devices"])
    notes = (
        f"Quantization: {pack['quantization']}. "
        f"{'Streams text while you speak. ' if pack['streaming'] else 'Returns text after you stop. '}"
        f"May run on {devices}; a device is used only after this exact model, runtime, "
        "driver and device pass qualification on the user's computer."
    )
    return {
        "schema_version": 1,
        "id": pack["id"],
        "revision": revision,
        "display_name": pack["display_name"],
        "source": pack["source"],
        "engine": pack["engine"],
        "format": pack["format"],
        "tasks": pack["tasks"],
        "languages": pack["languages"],
        "files": [
            {
                "path": entry["path"],
                "sha256": entry["sha256"],
                "size_bytes": entry["size_bytes"],
                "optional_signature": None,
            }
            for entry in pack["files"]
        ],
        "devices": pack["devices"],
        "runtime_constraints": {"notes": notes},
        "license": pack["license"],
    }


def model_card(pack: dict, revision: str) -> str:
    total = sum(entry["size_bytes"] for entry in pack["files"])
    source = pack["source"]
    licence = pack["license"]
    tags = ["narrivy", "automatic-speech-recognition", pack["format"]["family"], pack["format"]["precision"].lower()]
    front = [
        "---",
        f"license: {licence['spdx_or_name'].lower()}",
        f"base_model: {pack['base_model']}",
        "pipeline_tag: automatic-speech-recognition",
        "tags:",
        *[f"  - {tag}" for tag in tags],
        "---",
    ]
    rows = "\n".join(
        f"| `{entry['path']}` | {entry['size_bytes']:,} | `{entry['sha256']}` |" for entry in pack["files"]
    )
    body = f"""
# {pack['display_name']}

**Quantization: {pack['quantization']}.**

A speech-recognition model pack for Narrivy, a local dictation app. The
files are redistributed unmodified from the source
below; `model-manifest.json` lists each file's size and SHA-256.

| | |
|---|---|
| Base model | `{pack['base_model']}` |
| Format | {pack['format']['family']}, {pack['format']['precision']} |
| Quantization | {pack['quantization']} |
| Runs with | `{pack['engine']['id']}` {pack['engine']['min_version']} or newer |
| Devices | {', '.join(pack['devices'])} (each used only after it passes qualification on your computer) |
| Languages | {', '.join(pack['languages'])} |
| Tasks | {', '.join(pack['tasks'])} |
| Text appears | {'while you speak (streaming)' if pack['streaming'] else 'after you stop'} |
| Total size | {total / 1e6:,.0f} MB |
| Pack revision | {revision} |

## Source

`{source['locator']}` at revision `{source['immutable_revision']}`
(published by {source['publisher']}). Every file here is byte-identical to
that revision.

## Licence

{licence['spdx_or_name']} ({licence['source']}). {licence['notice']}

## Files

| File | Bytes | SHA-256 |
|---|---:|---|
{rows}
"""
    return "\n".join(front) + "\n" + body.lstrip("\n")


def stage(pack: dict, revision: str, staging: Path) -> Path:
    base = local_dir(pack["local"])
    out = staging / STORE_DIR / pack["store_path"]
    out.mkdir(parents=True, exist_ok=True)
    for entry in pack["files"]:
        target = out / entry["path"]
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() or target.is_symlink():
            target.unlink()
        source = (base / entry["path"]).resolve()
        try:
            os.link(source, target)
        except OSError:
            # A different filesystem: fall back to a link by name.
            target.symlink_to(source)
    (out / "model-manifest.json").write_text(json.dumps(manifest(pack, revision), indent=2) + "\n")
    (out / "README.md").write_text(model_card(pack, revision))
    return out


UPLOAD_SCRIPT = f"""#!/bin/bash
# Publish the staged Narrivy Store ({STORE_REPO}): every model, runtime and
# module folder under ./{STORE_DIR}. This uploads to the public narrivy
# Hugging Face organisation: run it yourself, after `hf auth login`.
set -euo pipefail
cd "$(dirname "$0")"
hf repo create "{STORE_REPO}" --repo-type model --exist-ok
hf upload "{STORE_REPO}" "{STORE_DIR}" . --repo-type model --commit-message "Narrivy Store"
echo "Uploaded. Record the new revision in tools/reference-models/published.json."
"""

STORE_CARD_HEAD = f"""---
license: other
license_name: per-pack
pipeline_tag: automatic-speech-recognition
tags:
  - narrivy
---
# Narrivy Store

Everything the Narrivy dictation app can add from its Store, in one place.
Narrivy installs a pack only at a pinned revision of this repository, and
checks every file against the pack's manifest before using it. Each pack's
folder has its own README and licence.

| Folder | What is in it |
|---|---|
| `models/gguf/` | Speech models for the built-in engine (transcribe.cpp), on the CPU or a Vulkan GPU |
| `models/openvino/` | Speech models for the OpenVINO add-on, on an Intel NPU, GPU or the CPU |
| `runtimes/openvino/` | The OpenVINO add-on itself: the libraries and Intel's NPU driver |
| `modules/` | Add-on modules such as noise suppression and the writing assistant, as they are published |
"""

MODULES_CARD = """# Modules

Add-on modules for Narrivy (noise suppression, the writing assistant's
language models and more) will be published here, one folder per module,
grouped by the runtime they need (for example `modules/openvino/<module>/`).
"""


def store_index(store: Path) -> str:
    """The Store's README: its layout, then one row per staged pack."""
    rows = []
    for manifest_path in sorted(store.glob("*/*/*/*-manifest.json")):
        folder = manifest_path.parent.relative_to(store).as_posix()
        document = json.loads(manifest_path.read_text())
        runtime = {"openvino": "OpenVINO"}.get(document.get("runtime", ""), document.get("runtime", ""))
        name = document.get("display_name") or f"{runtime} runtime {document.get('version', '')} ({document.get('platform', '')})"
        licence = document["license"]
        licence = licence["spdx_or_name"] if isinstance(licence, dict) else licence
        total = sum(entry["size_bytes"] for entry in document["files"])
        rows.append(f"| [`{folder}/`]({folder}) | {name} | {licence} | {total / 1e6:,.0f} MB |")
    table = "\n".join(["| Pack | Name | Licence | Size |", "|---|---|---|---:|", *rows])
    return STORE_CARD_HEAD + "\n## Packs\n\n" + table + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--staging", type=Path, default=DEFAULT_STAGING)
    parser.add_argument("--check", action="store_true", help="verify and validate only; stage nothing")
    args = parser.parse_args()

    config = json.loads((HERE / "packs.json").read_text())
    revision = config["pack_revision"]
    schema = json.loads(SCHEMA.read_text())
    staged, failed = [], 0
    for pack in config["packs"]:
        document = manifest(pack, revision)
        jsonschema.validate(document, schema)
        problems = verify(pack)
        if problems:
            failed += 1
            print(f"SKIP  {pack['id']}: {'; '.join(problems[:3])}")
            continue
        size = sum(entry["size_bytes"] for entry in pack["files"]) / 1e6
        if args.check:
            print(f"OK    {pack['id']}: {len(pack['files'])} files, {size:,.0f} MB verified")
            continue
        (HERE / "manifests").mkdir(exist_ok=True)
        name = pack["id"].split("/", 1)[1]
        (HERE / "manifests" / f"{name}.json").write_text(json.dumps(document, indent=2) + "\n")
        out = stage(pack, revision, args.staging)
        staged.append(out)
        print(f"STAGED {pack['id']}: {len(pack['files'])} files, {size:,.0f} MB -> {out}")
    if staged and not args.check:
        store = args.staging / STORE_DIR
        (store / "modules").mkdir(parents=True, exist_ok=True)
        (store / "modules" / "README.md").write_text(MODULES_CARD)
        (store / "README.md").write_text(store_index(store))
        script = args.staging / "upload.sh"
        script.write_text(UPLOAD_SCRIPT)
        script.chmod(0o755)
        print(f"store index: {store / 'README.md'}")
        print(f"upload script: {script}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
