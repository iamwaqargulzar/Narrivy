#!/usr/bin/env python3
"""Tauri/WebKitGTK/package feasibility probe — builder dependencies, toolchain,
workspace compile viability, webkit link flags, packaging tooling.

Salvaged from v0.4 (old/tools/p0f-probes/probe-tauri.py), adapted to the v1.3
tree (desktop client shell itself arrives in P3; this probe proves the build
boundary only). GUI launch is intentionally not performed here.
"""
import argparse
import datetime
import shutil
import subprocess
import sys

from p0f_common import ROOT, run, scenario_spec, write_evidence

SCENARIO = "p0f-tauri-build"


def check_deps():
    res = {}
    for pkg in ["webkit2gtk-4.1", "javascriptcoregtk-4.1", "libsoup-3.0", "gtk-3.0"]:
        rc, out, err = run(["pkg-config", "--modversion", pkg], timeout=3)
        res[pkg] = out.strip() if rc == 0 else f"missing ({err.strip()[:150]})"
    rc, out, err = run(["pkg-config", "--modversion", "webkitgtk-6.0"], timeout=3)
    res["webkitgtk-6.0"] = out.strip() if rc == 0 else "missing (expected when 4.1 is the lane)"
    for tool in ["cargo", "rustc", "node", "npm"]:
        rc, out, _ = run([tool, "--version"], timeout=3)
        res[tool] = (out or "").strip()[:120] or "missing"
    rc, out, _ = run(["cargo", "tauri", "--version"], timeout=5)
    res["cargo_tauri_available"] = rc == 0
    res["cargo_tauri_version"] = (out or "").strip()[:200]
    rc, out, _ = run(["dpkg", "-l", "libwebkit2gtk-4.1-dev"], timeout=3)
    res["dpkg_webkit_dev"] = ("installed" if "ii" in out and "libwebkit2gtk-4.1-dev" in out
                              else "missing")
    for exe in ["pkg-config", "cmake", "gcc"]:
        res[exe + "_available"] = shutil.which(exe) is not None
    return res


def test_build_boundary():
    res = {}
    # Workspace compile viability (bounded; the v1.3 workspace is small).
    try:
        r = subprocess.run(["cargo", "check", "--workspace", "--locked"], cwd=ROOT,
                           capture_output=True, text=True, timeout=600)
        res["workspace_cargo_check"] = {"exit_code": r.returncode,
                                        "stderr_excerpt": r.stderr[-800:],
                                        "pass": r.returncode == 0}
    except Exception as exc:
        res["workspace_cargo_check"] = {"error": str(exc), "pass": False}
    rc, out, _ = run(["pkg-config", "--libs", "webkit2gtk-4.1"], timeout=3)
    res["webkit_libs_ok"] = rc == 0
    res["webkit_libs"] = out.strip()[:400] if rc == 0 else "pkg-config libs failed"
    res["launch_test"] = {
        "performed": False,
        "reason": "GUI launch requires the P3 client shell and an interactive session; "
                  "this probe proves the builder/link boundary only",
    }
    return res


def check_package_deps():
    res = {}
    for exe in ["dpkg-deb", "dpkg-buildpackage", "appimagetool", "linuxdeploy"]:
        res[exe] = shutil.which(exe) or "missing"
    rc, out, _ = run(["dpkg", "-l", "libssl-dev"], timeout=3)
    res["libssl_dev"] = "installed" if "libssl-dev" in out else "missing"
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=None)
    args = ap.parse_args()
    out = args.out or f"agent/evidence/P0F/{SCENARIO}.json"
    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
    spec, spec_hash = scenario_spec(SCENARIO)

    deps = check_deps()
    build = test_build_boundary()
    pkg = check_package_deps()
    webkit_ok = deps.get("webkit2gtk-4.1", "").startswith("2.")
    workspace_ok = build.get("workspace_cargo_check", {}).get("pass", False)
    metrics = {
        "deps": deps, "build": build, "package": pkg,
        "summary": {
            "webkit2gtk_4_1_version": deps.get("webkit2gtk-4.1"),
            "webkit_ok": webkit_ok,
            "cargo_available": "cargo" in deps.get("cargo", ""),
            "cargo_check_workspace_pass": workspace_ok,
            "cargo_tauri_available": deps.get("cargo_tauri_available", False),
            "clean_launch_deps_proven": webkit_ok and workspace_ok,
        },
    }
    notes = [
        f"Tauri/WebKitGTK probe: webkit2gtk-4.1={deps.get('webkit2gtk-4.1')}, "
        f"node={deps.get('node')}, cargo-tauri CLI={deps.get('cargo_tauri_available')} "
        "(installable via cargo install tauri-cli; not a P0F blocker).",
        f"Workspace cargo check pass={workspace_ok}; webkit link flags resolvable="
        f"{build.get('webkit_libs_ok')}.",
        "Client shell is deliberately absent until P3; this probe proves the builder "
        "boundary (deps + toolchain + link viability) per phase-0F spec item 8.",
        f"Packaging tooling: dpkg-deb={pkg.get('dpkg-deb')} appimagetool={pkg.get('appimagetool')} "
        "(full packaging proof belongs to P10).",
    ]
    return write_evidence(
        verification_id="VER-P0F-TAURI", scenario_id=SCENARIO, out_rel=out,
        started_at=started, metrics=metrics, notes=notes,
        diagnostic_excerpt=(f"webkit={deps.get('webkit2gtk-4.1')} "
                            f"cargo_check={workspace_ok} "
                            f"tauri_cli={deps.get('cargo_tauri_available')}"),
        scenario_hash=spec_hash,
    )


if __name__ == "__main__":
    sys.exit(main())
