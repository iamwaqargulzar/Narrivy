#!/usr/bin/env python3
"""Text delivery probe — backend capability truth, Unicode/multiline fixtures,
target-class inventory, per-backend matrix per docs/platforms/
linux-text-delivery-matrix.md.

Salvaged from v0.4 (old/tools/p0f-probes/probe-delivery.py), adapted to the
v1.3 evidence contract and extended with a wtype/zwp_virtual-keyboard check
(the delivery mechanism the v0.4 daemon actually proved on this host class).
"""
import argparse
import datetime
import os
import shutil
import sys
from pathlib import Path

from p0f_common import run, scenario_spec, write_evidence

SCENARIO = "p0f-delivery-matrix"


def check_backends():
    b = {}
    rc, _, _ = run(["pkg-config", "--modversion", "libei-1.0"], timeout=3)
    b["libei_eis_available"] = rc == 0
    wl_copy = shutil.which("wl-copy") is not None
    wl_paste = shutil.which("wl-paste") is not None
    b["wl_clipboard"] = {"wl_copy": wl_copy, "wl_paste": wl_paste,
                         "available": wl_copy and wl_paste}
    b["wtype_available"] = shutil.which("wtype") is not None
    b["xclip_available"] = shutil.which("xclip") is not None
    b["xsel_available"] = shutil.which("xsel") is not None
    b["uinput_present"] = Path("/dev/uinput").exists()
    _, groups, _ = run(["groups"], timeout=3)
    b["uinput_in_input_group"] = "input" in groups.split()
    b["atspi_available"] = (shutil.which("at-spi-bus-launcher") is not None
                            or "AT_SPI_BUS_ADDRESS" in os.environ)
    return b


def fixture_tests():
    fixtures = {
        "ascii": "hello world 123",
        "accented": "Café naïve résumé façade",
        "emoji": "Hello 👋 🌍 emoji test 🚀",
        "urdu_arabic": "اردو زبان، سلام دنیا مرحبا",
        "cjk": "你好世界 こんにちは 안녕하세요",
        "multiline": "line1\nline2\nline3\n\nline5 with spaces   and\ttabs",
        "punctuation": "Hello, world! \"Quote\" — dash… spacing?  Don't glue words.",
        "large_5k": "A" * 5120,
    }
    results = {}
    for name, text in fixtures.items():
        try:
            encoded = text.encode("utf-8")
            results[name] = {"len_chars": len(text), "len_bytes": len(encoded),
                             "roundtrip_ok": encoded.decode("utf-8") == text}
        except Exception as exc:
            results[name] = {"error": str(exc), "roundtrip_ok": False}
    return results


def check_targets():
    targets = {}
    for app, exe in [("COSMIC Edit", "cosmic-edit"), ("GNOME Text Editor", "gnome-text-editor"),
                     ("COSMIC Terminal", "cosmic-term"), ("GNOME Terminal", "gnome-terminal"),
                     ("Firefox", "firefox"), ("Chromium", "chromium"), ("Brave", "brave-browser"),
                     ("Chrome", "google-chrome"), ("VS Code", "code"), ("Electron generic", "electron")]:
        targets[app] = shutil.which(exe) is not None
    rc, out, _ = run(["ps", "-ef"], timeout=3)
    targets["XWayland_running"] = "Xwayland" in out
    return targets


def per_backend_matrix(backends):
    m = {}
    m["input-method/native_text_commit"] = {
        "permission_model": "portal/compositor mediated; no broad input privilege",
        "unicode": "utf-8 native if supported", "multiline": "capability-dependent per app",
        "layout_dependence": "none (commits text, not keys)",
        "focus_requirements": "target must have text input focus",
        "verification_possible": False, "clipboard_impact": "none",
        "failure_detection": "per-protocol ack or timeout", "fallback": "clipboard -> inbox",
        "current_host_available": False,
        "notes": "requires compositor text-input support and client integration; "
                 "not generic injection — must be qualified per lane",
    }
    m["accessibility/automation"] = {
        "permission_model": "AT-SPI a11y bus; not broad input",
        "unicode": "where app exposes editable text", "multiline": "per-app",
        "layout_dependence": "none", "focus_requirements": "a11y focus",
        "verification_possible": False, "clipboard_impact": "none",
        "failure_detection": "a11y error", "fallback": "clipboard -> inbox",
        "current_host_available": backends.get("atspi_available", False),
        "notes": "AT-SPI present but not qualified as reliable delivery for 1.0",
    }
    m["virtual-keyboard/keyboard-emulation"] = {
        "permission_model": "compositor virtual keyboard (zwp_virtual_keyboard via wtype) "
                            "or libei/EIS; not uinput by default",
        "unicode": "limited — key symbols are layout dependent; emoji/composite glyphs unreliable",
        "multiline": "per-key emission; large text problematic",
        "layout_dependence": "yes", "focus_requirements": "focus required",
        "verification_possible": False, "clipboard_impact": "none",
        "failure_detection": "no insertion ack; bounded timeout", "fallback": "clipboard -> inbox",
        "current_host_available": backends.get("wtype_available", False)
                                  or backends.get("libei_eis_available", False),
        "notes": f"wtype present={backends.get('wtype_available')} (v0.4 daemon proved this "
                 f"mechanism on this host class); libei={backends.get('libei_eis_available')}",
    }
    m["clipboard+paste"] = {
        "permission_model": "Wayland data-control/clipboard; transient ownership, bounded timeout",
        "unicode": "full (UTF-8 MIME)", "multiline": "full",
        "layout_dependence": "paste-chord variance only",
        "focus_requirements": "focus + correct paste chord",
        "verification_possible": "limited — clipboard ownership check, no insertion ack",
        "clipboard_impact": "overwrites clipboard unless preserved with verified ownership",
        "failure_detection": "set-ok but paste unverified; timeout to Inbox", "fallback": "Inbox",
        "current_host_available": backends["wl_clipboard"]["available"],
        "notes": "transactional semantics required: journal before overwrite, restore only "
                 "while ownership is still ours",
    }
    m["Inbox"] = {
        "permission_model": "none — no target injection",
        "unicode": "full", "multiline": "full", "layout_dependence": "none",
        "focus_requirements": "none", "verification_possible": True,
        "clipboard_impact": "none", "failure_detection": "journal durability",
        "fallback": "itself (final)", "current_host_available": True,
        "notes": "always-available safe sink; never loses final text",
    }
    return m


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=None)
    args = ap.parse_args()
    out = args.out or f"agent/evidence/P0F/{SCENARIO}.json"
    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
    spec, spec_hash = scenario_spec(SCENARIO)

    backends = check_backends()
    fixtures = fixture_tests()
    targets = check_targets()
    matrix = per_backend_matrix(backends)
    metrics = {
        "backends": backends, "targets_installed": targets, "fixtures": fixtures,
        "per_backend_matrix": matrix,
        "summary": {
            "semantic_available": False,
            "wtype_available": backends["wtype_available"],
            "libei_available": backends["libei_eis_available"],
            "clipboard_available": backends["wl_clipboard"]["available"],
            "uinput_present": backends["uinput_present"],
            "uinput_is_default": False,
            "unicode_all_fixtures_roundtrip": all(v["roundtrip_ok"] for v in fixtures.values()),
            "inbox_fallback_always_available": True,
        },
    }
    notes = [
        "Delivery matrix probe: backend/tool capability truth + Unicode fixtures + per-backend matrix.",
        f"Backends: wtype={backends['wtype_available']} wl-clipboard={backends['wl_clipboard']['available']} "
        f"libei={backends['libei_eis_available']} atspi={backends['atspi_available']} "
        f"uinput_present={backends['uinput_present']} in_input_group={backends['uinput_in_input_group']}.",
        f"Unicode fixtures (ascii/accented/emoji/urdu-arabic/cjk/multiline/punctuation/large_5k): "
        f"all UTF-8 roundtrips pass={metrics['summary']['unicode_all_fixtures_roundtrip']}.",
        "Router order stays semantic -> virtual-keyboard/clipboard transaction -> Inbox; "
        "uinput is opt-in only and never the default onboarding path.",
        "Real per-target insertion evidence (native Wayland editor, browser, Electron, terminal, "
        "XWayland) is collected during P0F execution with a focused target; this probe records "
        "backend/tool truth only.",
    ]
    return write_evidence(
        verification_id="VER-P0F-DELIVERY", scenario_id=SCENARIO, out_rel=out,
        started_at=started, metrics=metrics, notes=notes,
        diagnostic_excerpt=(f"wtype={backends['wtype_available']} "
                            f"wl_clip={backends['wl_clipboard']['available']} "
                            f"unicode={metrics['summary']['unicode_all_fixtures_roundtrip']}"),
        scenario_hash=spec_hash,
    )


if __name__ == "__main__":
    sys.exit(main())
