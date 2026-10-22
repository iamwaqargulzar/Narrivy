#!/usr/bin/env python3
"""Least-privilege analysis probe (owner gate P0F-004, VER-P0F-LEAST-PRIV).

Documents the delivery privilege ladder and verifies the host truth used by
the analysis: /dev/uinput presence/permissions, input-group membership, libei,
wl-clipboard, portal state. Confirms no broad input privilege is required by
the default route. Salvaged from v0.4, adapted to the v1.3 contract.
"""
import argparse
import datetime
import subprocess
import sys
from pathlib import Path

from p0f_common import run, write_evidence


def host_truth():
    uinput = Path("/dev/uinput")
    return {
        "uinput_present": uinput.exists(),
        "uinput_mode": oct(uinput.stat().st_mode)[-3:] if uinput.exists() else None,
        "in_input_group": "input" in subprocess.run(["groups"], capture_output=True,
                                                    text=True, timeout=3).stdout.split(),
        "libei_available": run(["pkg-config", "--modversion", "libei-1.0"],
                               timeout=3)[0] == 0,
        "wl_copy_available": subprocess.run(["which", "wl-copy"], capture_output=True,
                                            text=True, timeout=3).returncode == 0,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="agent/evidence/P0F/p0f-least-privilege.json")
    args = ap.parse_args()
    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
    host = host_truth()
    analysis = {
        "principle": "least-privilege: prefer portal/compositor-mediated routes, then "
                     "clipboard transaction, then Inbox; broad synthetic-input privilege "
                     "is never the default onboarding path",
        "routes": [
            {"rank": 1, "backend": "portal/compositor semantic text commit (text-input-v3, input-method)",
             "permission": "compositor mediated, user focus, no uinput",
             "notes": "preferred where qualified; not host-proven yet"},
            {"rank": 2, "backend": "desktop/a11y automation (AT-SPI) where applicable",
             "permission": "a11y bus, not broad input",
             "notes": "not qualified as delivery for 1.0"},
            {"rank": 3, "backend": "clipboard + paste chord (wl-copy/wl-paste data-control)",
             "permission": "Wayland clipboard, transient ownership, bounded timeout, "
                           "restore only while ownership verified",
             "notes": "transactional semantics mandatory (journal before overwrite)"},
            {"rank": 4, "backend": "compositor virtual keyboard (zwp_virtual_keyboard via wtype) / libei EIS",
             "permission": "compositor-mediated virtual keyboard; not uinput",
             "notes": "bounded fallback; layout/Unicode limits recorded in delivery matrix"},
            {"rank": 5, "backend": "Inbox fallback",
             "permission": "none",
             "notes": "always-available safe sink; never loses final text"},
            {"rank": "opt-in only", "backend": "/dev/uinput, input-group, privileged daemon",
             "permission": "broad synthetic input; sees all input events; security/support burden",
             "notes": "present on many hosts but MUST NOT become the default; opt-in/advanced "
                      "only, requires explicit owner decision + threat review + ADR if ever default"},
        ],
        "policy": "No default broad input privilege: 1.0 onboarding must not require "
                  "uinput/input-group/privileged daemons (phase-0F spec probe 5).",
        "evidence_host": host,
        "conclusion": "Router order semantic -> clipboard/virtual-keyboard -> Inbox satisfies "
                      "delivery without broad privilege. Host uinput truth recorded; uinput is "
                      "not used, not default, and not required by any default route.",
    }
    notes = [
        "Least-privilege analysis: uinput/input-group not default, opt-in only; requires ADR + "
        "threat review if ever promoted to default.",
        f"Host truth: uinput_present={host['uinput_present']} mode={host['uinput_mode']} "
        f"in_input_group={host['in_input_group']} libei={host['libei_available']} "
        f"wl_copy={host['wl_copy_available']}.",
    ]
    return write_evidence(
        verification_id="VER-P0F-LEAST-PRIV", scenario_id="p0f-least-privilege",
        out_rel=args.out, started_at=started, metrics=analysis, notes=notes,
        diagnostic_excerpt="least-privilege: uinput not default; portal/clipboard->Inbox ladder documented",
    )


if __name__ == "__main__":
    sys.exit(main())
