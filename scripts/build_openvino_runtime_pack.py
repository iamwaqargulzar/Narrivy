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

`--platform windows-x86_64` builds the Windows pack from the Windows
archive (`openvino_genai_windows_2026.4.0.0_x86_64.zip`, unpacked): the
DLLs from `runtime/bin/intel64/Release` and oneTBB's from
`runtime/3rdparty/tbb/bin`, without a version suffix, into
`<out>/store/runtimes/openvino/2026.4-windows-x86_64`. It has no NPU
driver (`--npu-driver` is not used): Intel's Windows NPU driver installs
it system-wide. It does carry the Microsoft C++ runtime DLLs the OpenVINO
DLLs link, so a PC without the Visual C++ Redistributable runs it:
`--msvc-crt` names the `x64/Microsoft.VC143.CRT` folder of Visual Studio's
redistributable files (`VC/Redist/MSVC/<version>`), whose DLLs Microsoft
lists as redistributable. Narrivy's own programs link the runtime
statically and need none of it.

    scripts/build_openvino_runtime_pack.py --platform windows-x86_64 \\
        --sdk ~/.cache/narrivy/openvino-windows/openvino_genai_windows_2026.4.0.0_x86_64 \\
        --msvc-crt ~/.cache/narrivy/msvc-crt-14.44.35112 \\
        --out ~/.cache/narrivy/hf-staging

`--version` builds a pack of another OpenVINO archive for a developer to
try (for example a nightly, `2026.5.0.dev20261023`): the libraries are the
ones with that version's suffix (`.2650`), and the pack is written to
`<out>/dev/openvino-runtime-<version>-linux-x86_64`. `scripts/dev-daemon.sh`
uses it when `NARRIVY_OPENVINO_RUNTIME_DIR` names it; it is never published.
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
VERSION_SHORT = "2026.4"

# Versioned libraries of the closure, by stem: each gets the version's
# soname suffix (`.2640` for 2026.4.0).
SDK_VERSIONED = [
    "libopenvino.so",
    "libopenvino_c.so",
    "libopenvino_genai.so",
    "libopenvino_genai_c.so",
    "libopenvino_ir_frontend.so",
]
# Versioned libraries GenAI links from a given release on: (stem, (year,
# minor)).
SDK_VERSIONED_SINCE = [("libopenvino_gguf_frontend.so", (2026, 5))]
# The rest of the library closure the worker needs, by the soname other
# libraries ask for.
SDK_LIBRARIES = [
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

# Windows: the same closure as DLLs, which carry no version suffix. The
# optional ones are copied when the archive has them (the GGUF frontend
# from 2026.5 on; the NPU compiler pieces vary by release; ICU when the
# tokenizers DLL links it).
WINDOWS_DLLS = [
    "openvino.dll",
    "openvino_c.dll",
    "openvino_genai.dll",
    "openvino_genai_c.dll",
    "openvino_ir_frontend.dll",
    "openvino_tokenizers.dll",
    "openvino_intel_cpu_plugin.dll",
    "openvino_intel_gpu_plugin.dll",
    "openvino_intel_npu_plugin.dll",
]
WINDOWS_OPTIONAL_DLLS = [
    "openvino_gguf_frontend.dll",
    "openvino_intel_npu_compiler.dll",
    "openvino_intel_npu_compiler_loader.dll",
    "openvino_intel_npu_vm_runtime.dll",
]
WINDOWS_OPTIONAL_PATTERNS = ["icu*.dll"]
WINDOWS_TBB_DLLS = ["tbb12.dll", "tbbmalloc.dll"]
# The Microsoft C++ runtime DLLs the DLLs above import (the rest of the C
# runtime, `api-ms-win-crt-*`, is part of Windows 10 and later).
WINDOWS_MSVC_CRT_DLLS = [
    "msvcp140.dll",
    "msvcp140_codecvt_ids.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
]
MSVC_CRT_NOTE = """Microsoft Visual C++ runtime

`lib/msvcp140.dll`, `lib/msvcp140_codecvt_ids.dll`, `lib/vcruntime140.dll`
and `lib/vcruntime140_1.dll` are Microsoft's, copied unchanged and signed by
Microsoft, from the redistributable files of Visual Studio 2022 (VC143).
Microsoft lists them as Distributable Code that may be shipped with an
application: https://aka.ms/vs/17/redist.txt. They are here so the OpenVINO
DLLs run on a PC without the Visual C++ Redistributable, and are under
Microsoft's licence terms, not Apache-2.0.
"""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def soname_suffix(version: str) -> str:
    """`2640` for 2026.4.0 (and 2026.4.0.dev…): the year's last two digits,
    the minor and the patch number, as OpenVINO names its libraries."""
    parts = version.split(".")
    if len(parts) < 3 or not all(p.isdigit() for p in parts[:3]):
        raise SystemExit(f"not an OpenVINO version: {version}")
    return f"{int(parts[0]) % 100}{parts[1]}{parts[2]}"


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
    parser.add_argument("--npu-driver", type=Path,
                        help="directory with libze_loader.so.1 and libze_intel_npu.so.1 (Linux)")
    parser.add_argument("--msvc-crt", type=Path,
                        help="Visual Studio's x64 Microsoft.VC143.CRT folder (Windows)")
    parser.add_argument("--platform", choices=["linux-x86_64", "windows-x86_64"],
                        default="linux-x86_64")
    parser.add_argument("--out", type=Path, required=True, help="staging directory")
    parser.add_argument("--version", default=VERSION,
                        help=f"the archive's OpenVINO version (default {VERSION}); another one "
                        "builds a developer pack, not a Store pack")
    args = parser.parse_args()
    if args.platform == "windows-x86_64":
        return build_windows(args)
    if args.npu_driver is None:
        raise SystemExit("--npu-driver is needed for the Linux pack")
    store_path = STORE_PATH
    pack_id = PACK_ID
    version = args.version
    if version != VERSION:
        pack_id = f"openvino-runtime-{version}-linux-x86_64"
        store_path = f"dev/{pack_id}"
    release = tuple(int(part) for part in version.split(".")[:2])
    stems = SDK_VERSIONED + [stem for stem, since in SDK_VERSIONED_SINCE if release >= since]
    libraries = [f"{stem}.{soname_suffix(version)}" for stem in stems] + SDK_LIBRARIES

    sdk_lib = args.sdk / "runtime" / "lib" / "intel64"
    tbb_lib = args.sdk / "runtime" / "3rdparty" / "tbb" / "lib"
    licences = args.sdk / "docs" / "licensing"
    for needed in (sdk_lib, tbb_lib, licences, args.npu_driver):
        if not needed.is_dir():
            raise SystemExit(f"missing directory: {needed}")

    pack = args.out / store_path
    if pack.exists():
        shutil.rmtree(pack)
    (pack / "lib").mkdir(parents=True)
    origins: dict[str, str] = {}
    for name in libraries:
        copy_real(sdk_lib / name, pack / "lib" / name)
        origins[f"lib/{name}"] = f"openvino_genai {version} archive, runtime/lib/intel64"
    for name in TBB_LIBRARIES:
        copy_real(tbb_lib / name, pack / "lib" / name)
        origins[f"lib/{name}"] = f"openvino_genai {version} archive, runtime/3rdparty/tbb"
    for name in NPU_DRIVER_LIBRARIES:
        copy_real(args.npu_driver / name, pack / "lib" / name)
        origins[f"lib/{name}"] = "Intel linux-npu-driver / level-zero loader (MIT)"
    for licence in sorted(licences.iterdir()):
        if licence.is_file():
            target = pack / "licenses" / licence.name
            target.parent.mkdir(exist_ok=True)
            shutil.copyfile(licence, target)
            origins[f"licenses/{licence.name}"] = f"openvino_genai {version} archive, docs/licensing"

    readme = pack / "README.md"
    readme.write_text(
        f"# OpenVINO runtime {version} for Narrivy (Linux x86_64)\n\n"
        "The OpenVINO add-on for Narrivy: the libraries its speech worker loads to run\n"
        "models on an Intel NPU, Intel GPU or the CPU, and Intel's NPU user-space\n"
        "driver. Installed from Narrivy's Store; every file is checked against\n"
        "`runtime-manifest.json` before use.\n\n"
        "OpenVINO and OpenVINO GenAI are Apache-2.0; oneTBB is Apache-2.0; the\n"
        "Level Zero loader and NPU driver are MIT. Licences are in `licenses/`.\n",
        encoding="utf-8",
    )
    origins["README.md"] = "written by this script"
    return write_manifest(pack, pack_id, version, "linux-x86_64", "Apache-2.0 AND MIT", origins)


def build_windows(args: argparse.Namespace) -> int:
    version = args.version
    if version != VERSION:
        raise SystemExit("the Windows pack is built for the pinned version only")
    platform = "windows-x86_64"
    pack_id = f"openvino-runtime-{VERSION_SHORT}-{platform}"
    sdk_bin = args.sdk / "runtime" / "bin" / "intel64" / "Release"
    tbb_bin = args.sdk / "runtime" / "3rdparty" / "tbb" / "bin"
    licences = args.sdk / "docs" / "licensing"
    if args.msvc_crt is None:
        raise SystemExit("--msvc-crt is needed for the Windows pack")
    for needed in (sdk_bin, tbb_bin, licences, args.msvc_crt):
        if not needed.is_dir():
            raise SystemExit(f"missing directory: {needed}")
    pack = args.out / f"store/runtimes/openvino/{VERSION_SHORT}-{platform}"
    if pack.exists():
        shutil.rmtree(pack)
    (pack / "lib").mkdir(parents=True)
    origins: dict[str, str] = {}
    names = list(WINDOWS_DLLS)
    names += [name for name in WINDOWS_OPTIONAL_DLLS if (sdk_bin / name).is_file()]
    for pattern in WINDOWS_OPTIONAL_PATTERNS:
        names += sorted(path.name for path in sdk_bin.glob(pattern))
    for name in names:
        copy_real(sdk_bin / name, pack / "lib" / name)
        origins[f"lib/{name}"] = f"openvino_genai {version} Windows archive, runtime/bin/intel64/Release"
    for name in WINDOWS_TBB_DLLS:
        copy_real(tbb_bin / name, pack / "lib" / name)
        origins[f"lib/{name}"] = f"openvino_genai {version} Windows archive, runtime/3rdparty/tbb/bin"
    for name in WINDOWS_MSVC_CRT_DLLS:
        copy_real(args.msvc_crt / name, pack / "lib" / name)
        origins[f"lib/{name}"] = "Visual Studio 2022 redistributable files, Microsoft.VC143.CRT"
    for licence in sorted(licences.iterdir()):
        if licence.is_file():
            target = pack / "licenses" / licence.name
            target.parent.mkdir(exist_ok=True)
            shutil.copyfile(licence, target)
            origins[f"licenses/{licence.name}"] = f"openvino_genai {version} Windows archive, docs/licensing"
    (pack / "licenses" / "microsoft-vc-runtime.md").write_text(
        MSVC_CRT_NOTE, encoding="utf-8")
    origins["licenses/microsoft-vc-runtime.md"] = "written by this script"
    (pack / "README.md").write_text(
        f"# OpenVINO runtime {version} for Narrivy (Windows x86_64)\n\n"
        "The OpenVINO add-on for Narrivy on Windows: the libraries its language-model\n"
        "worker loads to run models on an Intel GPU, Intel NPU or the CPU. The NPU\n"
        "needs Intel's NPU driver for Windows, which is not in this pack. Installed\n"
        "from Narrivy's Models page; every file is checked against\n"
        "`runtime-manifest.json` before use.\n\n"
        "OpenVINO and OpenVINO GenAI are Apache-2.0; oneTBB is Apache-2.0. The\n"
        "Microsoft C++ runtime DLLs are Microsoft's redistributable files, under\n"
        "Microsoft's terms. Licences are in `licenses/`.\n",
        encoding="utf-8",
    )
    origins["README.md"] = "written by this script"
    return write_manifest(pack, pack_id, version, platform,
                          "Apache-2.0 AND LicenseRef-Microsoft-VC-Redistributable", origins)


def write_manifest(pack: Path, pack_id: str, version: str, platform: str, licence: str,
                   origins: dict[str, str]) -> int:
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
        "id": f"narrivy/{pack_id}",
        "kind": "runtime",
        "runtime": "openvino",
        "version": version,
        "platform": platform,
        "license": licence,
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
