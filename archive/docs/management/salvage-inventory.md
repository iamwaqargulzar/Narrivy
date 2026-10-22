# Salvage Inventory — v0.4 Archive → v1.3 Build

**Status:** owner-approved 2026-08-22. The v1.3 tree is the product; the v0.4
archive (`code/narrivy-v0.4/`, commit `5168619`) is a salvage source, not a submodule of truth.

## Rules

1. **Own code, freely reusable.** `code/narrivy-v0.4/` is Narrivy's own MIT-licensed code
   written under this project's rules. The clean-room policy restricts external
   products only; it does not restrict the archive.
2. **Adapt, never transplant.** Salvaged code lands only behind v1.3 contracts
   (Pipeline IDs, Module/CapabilityRegistry, Engine-vs-Runtime taxonomy,
   resource-registry identities). Copying old structure verbatim reintroduces
   the singleton assumptions the reset removed.
3. **Evidence does not transfer.** Old acceptance/evidence JSON is bound to old
   commits and satisfies nothing in the new fail-closed gates. Old *results*
   are expectations to re-prove, not proof.
4. **Record each landing.** Every material salvage lands with a
   `agent/DECISIONS.md` entry referencing this inventory row.

## What was salvaged already (2026-08-22)

| Item | From | To | Adaptation |
|---|---|---|---|
| P0F probes (activation, delivery, least-privilege, target-identity, mic, tauri) | `code/narrivy-v0.4/tools/p0f-probes/*.py` | `tools/p0f-probes/` + `p0f_common.py` | scenario hash from `spec/scenarios/*.json`; detected (not hardcoded) environment facts; GNOME lane fails truthfully on COSMIC host instead of simulating; wtype check added; shared helpers factored |
| Verified volatile versions (OpenVINO 2025.4.1 archive+sha) | old daemon/model-manager practice | `agent/RESOLVED_VERSIONS.json` notes | marked archive-verified; re-verify at P5 before use |

## Phase-by-phase salvage map (port when the phase starts)

| Target phase | Item (old location) | Notes |
|---|---|---|
| P1 | IPC framing/client/handshake, SO_PEERCRED peer validation, stale-socket cleanup, flock single-instance (`code/narrivy-v0.4/crates/narrivy-ipc`, `code/narrivy-v0.4/services/narrivyd/src/daemon.rs`) | DTOs gain `pipeline_id`; bounds/handshake semantics already match ADR-0011 |
| P1 | Recovery journal (file-per-tx JSON + fsync + rename) and SQLite persistence/migrations (`code/narrivy-v0.4/crates/narrivy-core`) | implements ADR-0013 nearly verbatim |
| P1 | Daemon skeleton: accept loop, bounded EventBus (drop-oldest), session-worker cancel/deliver atomics, signal handling | `DaemonState` reworked to multi-pipeline |
| P2 | Delivery router: bounded `wtype --` invocation, wl-copy/xclip copy-only honesty, uinput opt-in, Inbox store, typed outcomes (`code/narrivy-v0.4/adapters/platform-linux`) | maps to `delivery.*` module slots; hardest-won platform truth |
| P2/P5 | engine wiring patterns, audio prep, abort-callback cancellation, pinned catalog (`code/narrivy-v0.4/engines/asr-whisper`) | adapt behind the transcribe.cpp GGUF lane (`engines/transcribe-cpp`, feature-gated) per owner decision 2026-08-22 |
| P4 | Pre-roll ring buffer, non-blocking capture pipeline, mic ranking/hotplug, duck/pause restoration journal (`code/narrivy-v0.4/adapters/platform-linux/src/audio.rs`) | consolidate with (and retire) the daemon's pw-record+WavTail path |
| P5 | HF downloader (streaming sha256, `.part`→rename, LFS-hash fallback), safe ustar reader, OpenVINO runtime lifecycle, `DevicePlanner::discover_real()` (`code/narrivy-v0.4/crates/narrivy-model-manager`) | OpenVINO manager is the runtime-pack pattern (ADR-0015/0017); device planner de-duplicated into one compute module |
| P3 | Frontend plumbing: Tauri command proxy, event bridge, DaemonSnapshot reducer, theme/tokens, capability-driven UI patterns (`code/narrivy-v0.4/apps/narrivy-desktop`) | IA reworked to v1.3 pipelines; patterns reusable |
| P5/P11 | `narrivy-qualify` scenario runner + hardware matrix (`code/narrivy-v0.4/tools/narrivy-qualify`) | re-target new scenario IDs |
| P2+ | p2–p11 integration test suites (~98 tests, `code/narrivy-v0.4/crates/narrivy-core/tests/`) | blueprints; rewrite against v1.3 contracts |
| P10 | Tauri deb bundling (AppImage packaging salvaged but 1.0 scope dropped it — owner 2026-08-22), hardened systemd unit, maintainer scripts, `build_deb.sh`, SBOM/provenance pipeline (`code/narrivy-v0.4/apps/narrivy-desktop/src-tauri`, `code/narrivy-v0.4/packaging`, `code/narrivy-v0.4/scripts`) | Ubuntu 22.04 AppImage baseline per reproducible-builder.md |
| P0/P1 | Vetted dependency set (ureq, rusqlite bundled, nix, fs2, sha2, hound) + ggml/Vulkan bindgen env pattern (`code/narrivy-v0.4/.cargo/config.toml`) | reuse for transcribe.cpp native builds if needed |

## Knowledge salvage (expectations to re-prove, not evidence)

- Toggle activation works via portal where GlobalShortcuts exists; COSMIC portal 0.1 (xdg-desktop-portal 1.18.4) lacks the interface and lacks reliable key-release → Hold/PTT stays capability-dependent; toggle is default (locked decision).
- Semantic text insertion unavailable on the probed host class; wtype (zwp_virtual_keyboard) is the proven insertion mechanism; clipboard copy-only must never claim `Delivered`; GNOME insertion is security-model-limited.
- Lunar Lake NPU at `/dev/accel/accelN` (driver `intel_vpu`), Vulkan classification via `vulkaninfo` — device discovery approach proven.
- OpenVINO 2025.4.1 (build `2025.4.1.20426.82bbf0292c5`) archive + sha256 sidecar verified; GGUF model repos pin at immutable revisions at lock time; the ggml `vulkan` feature builds with local clang/bindgen setup.
- v0.4 "streaming" partials were full-inference snapshots (~1/s), not incremental streaming — a real streaming engine decision remains open for P5.

## Explicitly not carried over

- Singleton `NarrivyCore` (one engine/session), `StartDictation{mode, activation}` without Pipeline ID, in-memory Mode presets.
- Engine/Runtime taxonomy violations (`engines/asr-openvino` as engine; ONNX labels on GGML artifacts; static hardcoded model catalog).
- Duplicated `DevicePlanner`; parallel capture implementations; pw-record+WavTail as the primary capture path.
- Any old acceptance/evidence JSON as gate evidence for new requirements.

## Owner-authored NPU work (code/handy-npu/)

Extracted 2026-08-22 from the owner's public Handy-Pop-OS fork (commit range
016b175..c366fbb). Per owner decision, only owner-authored material is
retained; the fork tree and its git history were removed (the public fork
remains the authorship record). Verified NPU facts, worker protocol pattern,
and model-conversion recipes are in `agent/RESEARCH_LOG.md` (2026-08-22) and
the upstream version audit.

**Pending owner decision — fork code salvage:** MIT license would permit reuse,
and the NPU additions are the owner's own work; but unlike `code/narrivy-v0.4/` (pure Narrivy
code), this tree mixes owner work with upstream Handy code. If the owner amends
the clean-room policy for specifically the owner-authored NPU additions
(isolated worker, Eddy integration, Qwen3-ASR conversion scripts, pinned model
catalogue), they become code-salvage candidates for P5. Until then:
knowledge/oracle only, no code copying.
