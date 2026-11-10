#!/usr/bin/env python3
"""Assemble voice mode's voice packs: Kokoro (English) and Piper (Urdu).

The pack is what the Store installs for voice mode (Models → Text to
speech): Kokoro v1.0 as sherpa-onnx converts it (`kokoro-multi-lang-v1_0`),
cut to what English needs. The worker turns text into Kokoro's phonemes from
`lexicon-us-en.txt` itself (engines/voice-sherpa/src/phonemes.rs), so the
eSpeak NG data, the Chinese files and the dictionaries are left out. Every
file is copied (or hard-linked) unchanged, and `runtime-manifest.json` lists
each with its size and SHA-256.

    scripts/build_voice_pack.py \\
        --kokoro ~/Downloads/kokoro-multi-lang-v1_0 \\
        --out ~/.cache/narrivy/hf-staging

The pack is written to `<out>/store/voices/kokoro-v1.0-en`, its folder in the
Store repository (`narrivy/store`). The printed manifest digest and sizes go
into `packaging/store/catalog-additions.json` (`runtimes`, with
`"runtime": "voice"`); publishing is the owner's step.

The Urdu voice (D51) is Piper's `ur_PK-fasih-medium` as sherpa-onnx packs
it, with only the eSpeak NG data Urdu needs, flat beside the model (the
Urdu speaker points eSpeak NG at the pack itself):

    scripts/build_voice_pack.py \\
        --piper-urdu ~/Downloads/vits-piper-ur_PK-fasih-medium \\
        --out ~/.cache/narrivy/hf-staging
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
from pathlib import Path

PACK_ID = "voice-kokoro-v1.0-en"
STORE_PATH = "store/voices/kokoro-v1.0-en"
FILES = ["model.onnx", "voices.bin", "tokens.txt", "lexicon-us-en.txt", "LICENSE"]
ORIGIN = "sherpa-onnx kokoro-multi-lang-v1_0 (Kokoro-82M v1.0, Apache-2.0)"

URDU_ID = "voice-piper-ur-fasih-medium"
URDU_STORE_PATH = "store/voices/piper-ur-fasih-medium"
URDU_ORIGIN = "sherpa-onnx vits-piper-ur_PK-fasih-medium (Piper, MIT)"
ESPEAK_ORIGIN = "eSpeak NG data from sherpa-onnx's Piper pack (GPL-3.0-or-later)"
# Pack path -> path in the sherpa-onnx folder.
URDU_FILES = {
    "model.onnx": "ur_PK-fasih-medium.onnx",
    "tokens.txt": "tokens.txt",
    "MODEL_CARD": "MODEL_CARD",
}
URDU_ESPEAK = {
    "phontab": "espeak-ng-data/phontab",
    "phondata": "espeak-ng-data/phondata",
    "phondata-manifest": "espeak-ng-data/phondata-manifest",
    "phonindex": "espeak-ng-data/phonindex",
    "intonations": "espeak-ng-data/intonations",
    "ur_dict": "espeak-ng-data/ur_dict",
    "lang/ur": "espeak-ng-data/lang/inc/ur",
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def place(source: Path, target: Path, copy: bool) -> None:
    if source.is_symlink() or not source.is_file():
        raise SystemExit(f"not a plain file: {source}")
    target.parent.mkdir(parents=True, exist_ok=True)
    if copy:
        shutil.copyfile(source, target)
    else:
        os.link(source, target)


def write_manifest(pack: Path, pack_id: str, licence: str, origins: dict[str, str]) -> None:
    files = []
    for path in sorted(p for p in pack.rglob("*") if p.is_file() and p.name != "runtime-manifest.json"):
        name = path.relative_to(pack).as_posix()
        files.append({
            "path": name,
            "sha256": sha256(path),
            "size_bytes": path.stat().st_size,
            "origin": origins.get(name, "unknown"),
        })
    manifest = {
        "schema_version": 1,
        "id": pack_id,
        "kind": "runtime",
        "runtime": "voice",
        "version": "1.0",
        "platform": "any",
        "license": licence,
        "files": files,
    }
    manifest_path = pack / "runtime-manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    total = sum(entry["size_bytes"] for entry in files)
    print(f"{pack}: {len(files)} files, {total} bytes")
    print(f"id {pack_id}")
    print(f"manifest sha256 {sha256(manifest_path)} size {manifest_path.stat().st_size}")


def urdu(source: Path, out: Path, copy: bool) -> int:
    pack = out / URDU_STORE_PATH
    if pack.exists():
        shutil.rmtree(pack)
    pack.mkdir(parents=True)
    origins: dict[str, str] = {}
    for name, relative in {**URDU_FILES, **URDU_ESPEAK}.items():
        place(source / relative, pack / name, copy)
        origins[name] = ESPEAK_ORIGIN if name in URDU_ESPEAK else URDU_ORIGIN
    (pack / "README.md").write_text(
        "# Piper Urdu voice for Narrivy's voice mode\n\n"
        "Piper's `ur_PK-fasih-medium` voice (MIT, see `MODEL_CARD`) as packed\n"
        "by sherpa-onnx, with the eSpeak NG data it needs for Urdu\n"
        "(`phontab`, `phondata`, `phondata-manifest`, `phonindex`,\n"
        "`intonations`, `ur_dict`, `lang/ur`). That data is part of eSpeak NG\n"
        "and is GPL-3.0-or-later; its source is\n"
        "https://github.com/espeak-ng/espeak-ng. Narrivy's separate Urdu\n"
        "speaker (`narrivy-urdu-speaker`, GPL-3.0-or-later) reads this pack.\n",
        encoding="utf-8",
    )
    origins["README.md"] = "written by this script"
    write_manifest(pack, URDU_ID, "MIT AND GPL-3.0-or-later", origins)
    print(f"source_path {URDU_STORE_PATH.removeprefix('store/')}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--kokoro", type=Path,
                        help="the kokoro-multi-lang-v1_0 folder from sherpa-onnx's tts-models release")
    source.add_argument("--piper-urdu", type=Path,
                        help="the vits-piper-ur_PK-fasih-medium folder from sherpa-onnx's tts-models release")
    parser.add_argument("--out", type=Path, required=True, help="staging directory")
    parser.add_argument("--copy", action="store_true", help="copy instead of hard-linking")
    args = parser.parse_args()
    if args.piper_urdu:
        return urdu(args.piper_urdu, args.out, args.copy)

    missing = [name for name in FILES if not (args.kokoro / name).is_file()]
    if missing:
        raise SystemExit(f"not in {args.kokoro}: {', '.join(missing)}")
    pack = args.out / STORE_PATH
    if pack.exists():
        shutil.rmtree(pack)
    pack.mkdir(parents=True)
    origins: dict[str, str] = {}
    for name in FILES:
        source, target = args.kokoro / name, pack / name
        if source.is_symlink():
            raise SystemExit(f"links are not packed: {name}")
        if args.copy:
            shutil.copyfile(source, target)
        else:
            os.link(source, target)
        origins[name] = ORIGIN

    (pack / "README.md").write_text(
        "# Kokoro English voice for Narrivy's voice mode\n\n"
        "Kokoro-82M v1.0 (54 voices) as converted by sherpa-onnx, with the\n"
        "American English word list. Narrivy's voice worker reads the word\n"
        "list itself, so eSpeak NG is not needed. Installed from Narrivy's\n"
        "Store; every file is checked against `runtime-manifest.json`.\n\n"
        "Kokoro is Apache-2.0 (see `LICENSE`).\n",
        encoding="utf-8",
    )
    origins["README.md"] = "written by this script"

    files = []
    for path in sorted(p for p in pack.iterdir() if p.is_file()):
        files.append({
            "path": path.name,
            "sha256": sha256(path),
            "size_bytes": path.stat().st_size,
            "origin": origins.get(path.name, "unknown"),
        })
    manifest = {
        "schema_version": 1,
        "id": PACK_ID,
        "kind": "runtime",
        "runtime": "voice",
        "version": "1.0",
        "platform": "any",
        "license": "Apache-2.0",
        "files": files,
    }
    manifest_path = pack / "runtime-manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    total = sum(entry["size_bytes"] for entry in files)
    print(f"{pack}: {len(files)} files, {total} bytes")
    print(f"id {PACK_ID}")
    print(f"source_path {STORE_PATH.removeprefix('store/')}")
    print(f"manifest sha256 {sha256(manifest_path)} size {manifest_path.stat().st_size}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
