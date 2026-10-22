#!/usr/bin/env python3
"""Non-destructive Narrivy development/runtime capability doctor — per the legacy planning amendment Part N.

Emits human-readable or structured JSON (--json). As phases advance it inspects
desktop/session, portal backend, PipeWire, microphone, libei/EIS, clipboard,
uinput (as capability diagnostic only), Secret Service, WebKitGTK/Tauri, ASR runtime,
disk/model cache budget, package prerequisites, and versions used in qualification.
Diagnostics must redact sensitive content (no transcript/audio/clipboard).
"""
from __future__ import annotations
import argparse, json, os, platform, shutil, subprocess, sys
from pathlib import Path

def version(cmd):
    exe=shutil.which(cmd[0])
    if not exe:
        return "missing"
    try:
        p=subprocess.run([exe,*cmd[1:]], capture_output=True, text=True, timeout=5)
        line=(p.stdout or p.stderr).strip().splitlines()
        return line[0] if line else f"present ({p.returncode})"
    except Exception as e:
        return f"present (version check failed: {type(e).__name__})"

def check_portal():
    # Check xdg-desktop-portal and GlobalShortcuts availability via busctl
    if not shutil.which("busctl"):
        return "unknown (busctl missing)"
    try:
        p=subprocess.run(["busctl","--user","status","org.freedesktop.portal.Desktop"], capture_output=True, text=True, timeout=5)
        if p.returncode==0:
            return "portal available"
        return "portal not running"
    except Exception as e:
        return f"portal check failed: {e}"

def check_pipewire():
    for cmd in (["pw-cli","--version"], ["wpctl","status"]):
        v=version(cmd)
        if "missing" not in v:
            return v
    return "PipeWire not detected"

def check_uinput():
    # Only as capability diagnostic, not instruction
    try:
        return "available" if Path("/dev/uinput").exists() else "not present (expected for 1.0 default)"
    except Exception:
        return "unknown"

def check_secret_service():
    try:
        p=subprocess.run(["busctl","--user","status","org.freedesktop.secrets"], capture_output=True, text=True, timeout=5)
        return "Secret Service available" if p.returncode==0 else "Secret Service not running (expected for P0, required from P8)"
    except Exception:
        return "unknown"

def check_webkit():
    v=version(["pkg-config","--modversion","webkit2gtk-4.1"])
    if "missing" in v:
        v=version(["pkg-config","--modversion","webkitgtk-6.0"])
    return v

def check_asr_runtime():
    # Check vulkaninfo, rocminfo, openvino if available
    results={}
    for name, cmd in [("vulkaninfo",["vulkaninfo","--summary"]), ("rocminfo",["rocminfo"]), ("openvino",["python3","-c","import openvino; print(openvino.__version__)"])]:
        results[name]=version(cmd)
    return results

def disk_budget():
    try:
        p=Path.home()
        # Check cache
        cache=Path(os.getenv("XDG_CACHE_HOME", str(p/".cache")))/"narrivy"
        used=0
        if cache.is_dir():
            for f in cache.rglob("*"):
                try:
                    if f.is_file():
                        used+= f.stat().st_size
                except Exception:
                    pass
        cache_label = "$XDG_CACHE_HOME/narrivy" if os.getenv("XDG_CACHE_HOME") else "~/.cache/narrivy"
        return {"cache_path": cache_label, "cache_used_bytes": used, "cache_used_gb": round(used/1e9,2)}
    except Exception as e:
        return {"error": f"cache inspection failed: {type(e).__name__}"}

def remediation_for(report):
    actions=[]
    if report["portal"] != "portal available":
        actions.append({"capability":"desktop portal","action":"Install and start xdg-desktop-portal plus the backend for your desktop session."})
    if "not detected" in report["pipewire"] or "missing" in report["pipewire"]:
        actions.append({"capability":"audio service","action":"Install and start PipeWire and WirePlumber, then sign in again."})
    if "missing" in report["microphone"].lower():
        actions.append({"capability":"microphone discovery","action":"Install pactl (usually provided by pipewire-pulse or pulseaudio-utils)."})
    if report["clipboard"]["wl_clipboard"] == "missing" and report["clipboard"]["xclip"] == "missing":
        actions.append({"capability":"clipboard fallback","action":"Install wl-clipboard for Wayland or xclip for X11 compatibility applications."})
    if "not running" in report["secret_service"]:
        actions.append({"capability":"secret storage","action":"Start a Secret Service provider such as GNOME Keyring or KWallet before configuring remote providers."})
    if report["webkit_tauri"] == "missing":
        actions.append({"capability":"desktop runtime","action":"Install WebKitGTK 6.0 (or the supported 4.1 compatibility development/runtime package)."})
    return actions

def build_report():
    import platform as pf
    report={
        "desktop": {
            "os": pf.platform(),
            "session_type": os.getenv("XDG_SESSION_TYPE","<unset>"),
            "current_desktop": os.getenv("XDG_CURRENT_DESKTOP","<unset>"),
            "wayland_display": os.getenv("WAYLAND_DISPLAY","<unset>"),
            "display": os.getenv("DISPLAY","<unset>"),
            "xdg_runtime_dir": os.getenv("XDG_RUNTIME_DIR","<unset>"),
        },
        "portal": check_portal(),
        "pipewire": check_pipewire(),
        "microphone": version(["pactl","list","sources"]) if shutil.which("pactl") else "pactl missing (PipeWire/Pulse check limited)",
        "libei_eis": version(["pkg-config","--modversion","libei-1.0"]) if shutil.which("pkg-config") else "pkg-config missing",
        "clipboard": {
            "wl_clipboard": version(["wl-copy","--version"]),
            "xclip": version(["xclip","-version"]),
        },
        "uinput": check_uinput(),
        "secret_service": check_secret_service(),
        "webkit_tauri": check_webkit(),
        "asr_runtime": check_asr_runtime(),
        "disk": disk_budget(),
        "toolchain": {
            "python": sys.version.split()[0],
            "cargo": version(["cargo","--version"]),
            "rustc": version(["rustc","--version"]),
            "node": version(["node","--version"]),
            "git": version(["git","--version"]),
            "cargo_deny": version(["cargo-deny","--version"]),
        },
        "notes": "Doctor is non-destructive, no transcript/audio/clipboard content inspected. uinput shown only as capability diagnostic."
    }
    report["remediation"] = remediation_for(report)
    return report

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true", help="emit JSON")
    ap.add_argument("--output", help="path to write JSON")
    args=ap.parse_args()
    report=build_report()
    # Also include human-readable facts
    if args.json:
        out=json.dumps(report, indent=2)
        if args.output:
            Path(args.output).write_text(out+"\n")
            print(f"Wrote {args.output}")
        else:
            print(out)
    else:
        # Human readable (legacy)
        print("Narrivy doctor — non-destructive; no user content is inspected")
        print(f"OS: {platform.platform()}")
        print(f"Python: {sys.version.split()[0]}")
        for name,cmd in [
            ("cargo",["cargo","--version"]),("rustc",["rustc","--version"]),
            ("rustup",["rustup","--version"]),("node",["node","--version"]),("npm",["npm","--version"]),
            ("git",["git","--version"]),("cmake",["cmake","--version"]),("pkg-config",["pkg-config","--version"]),
            ("pw-cli",["pw-cli","--version"]),("wpctl",["wpctl","--version"]),("busctl",["busctl","--version"]),
            ("cargo-deny",["cargo-deny","--version"]),
        ]:
            print(f"{name}: {version(cmd)}")
        print(f"XDG_SESSION_TYPE: {report['desktop']['session_type']}")
        print(f"XDG_CURRENT_DESKTOP: {report['desktop']['current_desktop']}")
        print(f"WAYLAND_DISPLAY: {report['desktop']['wayland_display']}")
        print(f"DISPLAY: {report['desktop']['display']}")
        print(f"XDG_RUNTIME_DIR: {report['desktop']['xdg_runtime_dir']}")
        print(f"portal: {report['portal']}")
        print(f"pipewire: {report['pipewire']}")
        print(f"uinput: {report['uinput']} (capability only, not default)")
        print(f"secret_service: {report['secret_service']}")
        print(f"webkit: {report['webkit_tauri']}")
        print(f"disk: {report['disk']}")
        print("\nDoctor does not grant permissions or install packages. See scripts/bootstrap-linux-dev.sh for explicit dev dependencies.")
        print("For JSON: python3 scripts/doctor.py --json")

if __name__=="__main__":
    main()
