#!/usr/bin/env python3
"""Target identity probe — app/window identity, selection/surrounding-text
discovery, permission checks, fallback semantics.

Salvaged from v0.4 (old/tools/p0f-probes/probe-target-identity.py), adapted to
the v1.3 evidence contract (no hardcoded hardware; shared helpers).
"""
import argparse
import datetime
import os
import shutil
import sys

from p0f_common import run, scenario_spec, write_evidence

SCENARIO = "p0f-target-identity"


def check_window_identity():
    res = {}
    rc, out, _ = run(["busctl", "--user", "introspect",
                      "com.system76.CosmicComp", "/"], timeout=3)
    res["cosmic_comp_introspect_success"] = rc == 0
    res["cosmic_comp_has_window_api"] = ("Window" in out or "window" in out.lower())
    for exe in ["wmctrl", "xdotool", "xprop"]:
        res[exe + "_available"] = shutil.which(exe) is not None
    res["atspi_bus_address_present"] = "AT_SPI_BUS_ADDRESS" in os.environ
    rc, out, _ = run(["ps", "-eo", "comm"], timeout=3)
    res["running_apps_sample"] = [c for c in ["cosmic-comp", "firefox", "brave", "code",
                                              "gnome-text-editor", "cosmic-edit", "cosmic-term"]
                                  if c in out][:5]
    # No standard Wayland portal exposes generic window identity; compositor
    # cooperation or a11y is required. Truthful default until proven otherwise.
    res["generic_wayland_window_identity_available"] = False
    res["xwayland_window_identity_available"] = bool(
        res.get("wmctrl_available") or res.get("xdotool_available")
        or res.get("xprop_available"))
    return res


def check_selection_surrounding():
    res = {}
    res["wl_copy_available"] = shutil.which("wl-copy") is not None
    res["wl_paste_available"] = shutil.which("wl-paste") is not None
    res["xclip_available"] = shutil.which("xclip") is not None
    res["libei_available"] = run(["pkg-config", "--modversion", "libei-1.0"],
                                 timeout=3)[0] == 0
    rc, out, _ = run(["busctl", "--user", "introspect",
                      "org.freedesktop.portal.Desktop",
                      "/org/freedesktop/portal/desktop"], timeout=5)
    res["portal_has_text_input"] = ("InputMethod" in out or "TextInput" in out)
    # Selection/surrounding text need app cooperation (a11y or input-method
    # surrounding-text); not generically available on Wayland. Truthful until
    # a specific mechanism is qualified (deferred P8 surface).
    res["selection_discovery_available"] = False
    res["surrounding_text_available"] = False
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=None)
    args = ap.parse_args()
    out = args.out or f"agent/evidence/P0F/{SCENARIO}.json"
    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
    spec, spec_hash = scenario_spec(SCENARIO)

    win = check_window_identity()
    sel = check_selection_surrounding()
    metrics = {
        "window_identity": win, "selection_surrounding": sel,
        "summary": {
            "app_window_id_available": win["generic_wayland_window_identity_available"],
            "xwayland_window_available": win["xwayland_window_identity_available"],
            "selection_available": sel["selection_discovery_available"],
            "surrounding_available": sel["surrounding_text_available"],
            "fallback_required": True,
            "permission_required": "compositor-specific or a11y; no generic portal "
                                   "window-identity interface on this host",
        },
    }
    notes = [
        "Target identity probe: app/window identity and selection/surrounding truth.",
        f"Window/app identity: generic Wayland={metrics['summary']['app_window_id_available']} "
        f"(truthfully unavailable without compositor/a11y cooperation); "
        f"XWayland tools present={[k for k in ('wmctrl_available', 'xdotool_available', 'xprop_available') if win.get(k)]}; "
        f"cosmic-comp window API introspectable={win['cosmic_comp_introspect_success']}; "
        f"atspi_bus={win['atspi_bus_address_present']}.",
        f"Selection/surrounding: wl_copy={sel['wl_copy_available']} libei={sel['libei_available']} "
        f"portal_text_input={sel['portal_has_text_input']} "
        f"selection={sel['selection_discovery_available']} surrounding={sel['surrounding_text_available']}.",
        "Fallback semantics: when identity is missing/ambiguous the delivery router must still "
        "deliver to the focused target or Inbox — dictation is never blocked on identity; "
        "'unknown target' is an explicit state, never a silent failure.",
    ]
    return write_evidence(
        verification_id="VER-P0F-TARGET", scenario_id=SCENARIO, out_rel=out,
        started_at=started, metrics=metrics, notes=notes,
        diagnostic_excerpt=(f"window={metrics['summary']['app_window_id_available']} "
                            f"selection={metrics['summary']['selection_available']} "
                            "fallback required"),
        scenario_hash=spec_hash,
    )


if __name__ == "__main__":
    sys.exit(main())
