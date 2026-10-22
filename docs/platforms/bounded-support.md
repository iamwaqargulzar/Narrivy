# Bounded Support Lanes — Linux 1.0

> **Authority:** This file defines the **only** supported environments for Narrivy Linux 1.0 per the legacy planning amendment Part H. Support claims are **generated from P11 evidence**, not from aspirations or from “24.04+” open-ended language.

## First-class lanes (require native qualification per P11)

> **Owner decision 2026-09-28:** the first release qualifies **only** `cosmic-wayland` (Pop!_OS). `gnome-wayland` is the first **post-release** lane, qualified natively after the Pop!_OS release is properly tested; other distributions, then Windows, then macOS follow. Until then GNOME/Ubuntu must not be advertised as supported.

| Lane ID | OS | Version | Kernel baseline | Desktop & Session | Native required for |
|---|---|---|---|---|---|
| `cosmic-wayland` | Pop!_OS | 24.04 LTS (COSMIC Epoch 7.0+) | 6.8+ (HWE where applicable) | COSMIC / Wayland | activation, mic, delivery, lifecycle, latency |
| `gnome-wayland` *(post-release lane)* | Ubuntu | 24.04.2 LTS | 6.8 (HWE 6.8.0-xx) / 6.11 HWE where qualified | GNOME 46 / Wayland | activation, mic, delivery, lifecycle, latency |

Each lane requires:
- at least **one physical/native machine** for platform-sensitive scenarios (Part C P11, Part F harness `native_or_vm: native`);
- qualification证据 (schema-valid `evidence.json` with `environment.native_or_vm: native`, `os_version`, `kernel`, `desktop`, `session`, `package_sha256` bound to release `git_commit` and `artifacts/release-evidence/linux-1.0/release-artifacts.json` hashes);
- `e2e_pass: true` for clean install → onboarding → first dictation → short/long/Unicode/multiline/Raw-Clean → delivery fallback/Inbox → cancel race → daemon crash/restart/recovery → suspend/resume/hotplug → audio restoration.

Exact qualified versions are recorded at **P11 qualification time** in `artifacts/release-evidence/linux-1.0/qualification.json` and in release notes `artifacts/release-evidence/linux-1.0/summary.md`. This file lists the **lanes**; the exact `os_version`/`kernel`/`desktop` recorded as `qualification.json: environments[].os_version` is the source of truth for the release.

## Compatibility signal (not supported)

- XWayland targets on the above lanes (tested per P0F matrix, but reported as “best-effort”).
- Additional Linux desktops/distros (Fedora 40+, Debian 12, Arch, KDE/Wlroots, COSMIC on other bases) may be described as **experimental / community-tested** where VM or anecdotal evidence exists, but **never** as “supported” until native P11 evidence is added.

## Prohibited publish-time claims

- Do not advertise a desktop not qualified (`gnome-wayland` `e2e_pass` false → no GNOME support claim).
- Do not advertise PTT where only `toggle` was proven on that lane (check `P0F-activation-*` capability truth).
- Do not advertise an accelerator merely because the API enumerates it — only qualified tuples `hardware+driver+runtime+model+quant+OS+Narrivy version` with `pass: true` in `qualification.json: accelerator_claims`.
- Do not advertise Unicode correctness on a delivery backend that only handles key events reliably for a subset — per-backend capability matrix `docs/platforms/capability-matrix.md` governs.
- Do not present VM-only platform behavior as native support.

## How release notes are derived

`scripts/release_gate.py` verifies:
- both lanes have `native` `e2e_pass: true`,
- `qualification.json` and `release-artifacts.json` are bound to the release `git_commit`,
- checksums match,
- `summary.md` lists only the lanes that actually passed.

`docs/platforms/capability-matrix.md` is updated from P0F/P11 evidence, not assumptions.

## History

- An earlier pack used open-ended “24.04+ / COSMIC / Wayland and GNOME / Wayland”. The current contract bounds this to exact qualified versions per the two lanes above.
