#!/usr/bin/env python3
"""Assemble the OpenVINO runtime pack from an OpenVINO 2026.4 archive.

The pack is what the Store installs for the OpenVINO add-on (D25): the
minimal library closure the speech worker loads, Intel's NPU user-space
driver, their licences, and `runtime-manifest.json` listing every file with
its size and SHA-256. Nothing here is built from source; every file is
copied from the given inputs, and the manifest records where each came from.

    scripts/build_openvino_runtime_pack.py \\
        --sdk ~/.cache/handy-openvino-build/sdk-2026.4-installed \\
        --npu-driver /usr/lib/Handy/openvino \\
        --out ~/.cache/narrivy/hf-staging

The pack is written to `<out>/store/runtimes/openvino/2026.4-linux-x86_64`,
its folder in the Store repository (`narrivy/store`). Then run
`tools/reference-models/build_model_packs.py`, which refreshes the Store's
index and `upload.sh`; publishing is the owner's step.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path

PACK_ID = "openvino-runtime-2026.4-linux-x86_64"
STORE_PATH = "store/runtimes/openvino/2026.4-linux-x86_64"
VERSION = "2026.4.0"

# Library closure the worker needs, by the soname other libraries ask for.
SDK_LIBRARIES = [
    "libopenvino.so.2640",
    "libopenvino_c.so.2640",
    "libopenvino_genai.so.2640",
    "libopenvino_genai_c.so.2640",
    "libopenvino_ir_frontend.so.2640",
    "libopenvino_tokenizers.so",
    "libopenvino_intel_cpu_plugin.so",
    "libopenvino_intel_gpu_plugin.so",
    "libopenvino_intel_npu_plugin.so",
    "libopenvino_intel_npu_compiler.so",
    "libopenvino_intel_npu_compiler_loader.so",
    "libopenvino_intel_npu_vm_runtime.so",
]
TBB_LIBRARIES = ["libtbb.so.12", "libtbbmalloc.so.2"]
NPU_DRIVER_LIBRARIES = ["libze_loader.so.1", "libze_intel_npu.so.1"]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def copy_real(source: Path, target: Path) -> None:
    """Copy the file a soname link points at, under the soname."""
    real = source.resolve(strict=True)
    if not real.is_file():
        raise SystemExit(f"not a file: {source}")
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(real, target)
    target.chmod(0o644)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--sdk", type=Path, required=True, help="OpenVINO 2026.4 archive root")
    parser.add_argument("--npu-driver", type=Path, required=True,
                        help="directory with libze_loader.so.1 and libze_intel_npu.so.1")
    parser.add_argument("--out", type=Path, required=True, help="staging directory")
    args = parser.parse_args()

    sdk_lib = args.sdk / "runtime" / "lib" / "intel64"
    tbb_lib = args.sdk / "runtime" / "3rdparty" / "tbb" / "lib"
    licences = args.sdk / "docs" / "licensing"
    for needed in (sdk_lib, tbb_lib, licences, args.npu_driver):
        if not needed.is_dir():
            raise SystemExit(f"missing directory: {needed}")

    pack = args.out / STORE_PATH
    if pack.exists():
        shutil.rmtree(pack)
    (pack / "lib").mkdir(parents=True)
    origins: dict[str, str] = {}
    for name in SDK_LIBRARIES:
        copy_real(sdk_lib / name, pack / "lib" / name)
        origins[f"lib/{name}"] = f"openvino_genai {VERSION} archive, runtime/lib/intel64"
    for name in TBB_LIBRARIES:
        copy_real(tbb_lib / name, pack / "lib" / name)
        origins[f"lib/{name}"] = f"openvino_genai {VERSION} archive, runtime/3rdparty/tbb"
    for name in NPU_DRIVER_LIBRARIES:
        copy_real(args.npu_driver / name, pack / "lib" / name)
        origins[f"lib/{name}"] = "Intel linux-npu-driver / level-zero loader (MIT)"
    for licence in sorted(licences.iterdir()):
        if licence.is_file():
            target = pack / "licenses" / licence.name
            target.parent.mkdir(exist_ok=True)
            shutil.copyfile(licence, target)
            origins[f"licenses/{licence.name}"] = f"openvino_genai {VERSION} archive, docs/licensing"

    readme = pack / "README.md"
    readme.write_text(
        f"# OpenVINO runtime {VERSION} for Narrivy (Linux x86_64)\n\n"
        "The OpenVINO add-on for Narrivy: the libraries its speech worker loads to run\n"
        "models on an Intel NPU, Intel GPU or the CPU, and Intel's NPU user-space\n"
        "driver. Installed from Narrivy's Store; every file is checked against\n"
        "`runtime-manifest.json` before use.\n\n"
        "OpenVINO and OpenVINO GenAI are Apache-2.0; oneTBB is Apache-2.0; the\n"
        "Level Zero loader and NPU driver are MIT. Licences are in `licenses/`.\n",
        encoding="utf-8",
    )
    origins["README.md"] = "written by this script"

    files = []
    for path in sorted(p for p in pack.rglob("*") if p.is_file()):
        relative = path.relative_to(pack).as_posix()
        files.append({
            "path": relative,
            "sha256": sha256(path),
            "size_bytes": path.stat().st_size,
            "origin": origins.get(relative, "unknown"),
        })
    manifest = {
        "schema_version": 1,
        "id": f"narrivy/{PACK_ID}",
        "kind": "runtime",
        "runtime": "openvino",
        "version": VERSION,
        "platform": "linux-x86_64",
        "license": "Apache-2.0 AND MIT",
        "files": files,
    }
    (pack / "runtime-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    total = sum(entry["size_bytes"] for entry in files)
    manifest_digest = sha256(pack / "runtime-manifest.json")
    print(f"{pack}: {len(files)} files, {total / 1e6:.0f} MB")
    print(f"manifest sha256 {manifest_digest}")
    print("next: tools/reference-models/build_model_packs.py (Store index and upload.sh)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
