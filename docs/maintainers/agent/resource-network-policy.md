# Autonomous Agent Resource & Network Policy

> Authority: legacy planning Part M. Bounds so implementation cannot consume unbounded resources.

## Scope

Applies to the autonomous build agent (and to CI) when operating during P0–P11. Human-initiated builds outside the agent may override with explicit owner approval.

## Automatic (no owner prompt) allowed

- Installing safe reversible dev dependencies via `apt-get`/`cargo install` within the container/VM (e.g., `build-essential`, `pkg-config`, `libwebkit2gtk-4.1-dev`, `PipeWire` dev packages listed in `scripts/bootstrap-linux-dev.sh`).
- Running `cargo check/test/clippy`, `cargo deny`, `npm install` for the desktop.
- Downloading model fixtures for qualification **up to the per-model size limit** (see below).

## Requires owner action (blocker)

- Broad privilege changes (`sudo` outside container, adding user to `input`/`audio` groups, enabling `/dev/uinput`, persistent security-policy changes).
- Account/authentication actions (Hugging Face push token, GitHub release signing, apt repo push).
- Legal decisions, secrets/signing key handling, irreversible external actions (publishing a release, deleting data).

## Defaults

| Resource | Default limit | Behavior on exceed |
|---|---|---|
| **Model download (automatic)** | 5 GB per model, 20 GB total cache without owner action | Stop, record `B-MODEL-SIZE` blocker, ask owner to approve larger cache or select smaller quant (e.g., Whisper INT4 vs FP16). |
| **Cache / model storage budget** | 20 GB under `$XDG_CACHE_HOME/narrivy` | `doctor` warns, Model Manager shows “Cache full — clear compiled cache or remove models”. Agent may `cargo clean` or clear compiled blobs with `narrivy-cli cache --clear` before requesting more space. |
| **VM/container disk** | 40 GB for disposable builders/VMs | Fail the job, request owner to enlarge or to run on a larger runner. |
| **Retry count** | 3 retries for failing builds/tests/downloads with backoff (5 s, 30 s, 120 s) | After 3, record failure in `agent/METRICS.csv` and surface as blocker if persistent; do not loop forever. |
| **Timeout** | 10 min per cargo/test command, 30 min for full `cargo test --workspace`, 60 min for packaging | Timeout is a failure; `doctor` diagnostics suggest bumping if host is slow, but do not silently extend. |
| **Network source preference** | Prefer official Rust/Node registries (`crates.io`, `npmjs.org`), official `apt`/`rustup` releases, and `huggingface.co` with pinned revisions; verify checksum/signature where available. | Deny arbitrary `curl | sh` installers per `SECURITY.md`; if an upstream is unavailable, record blocker rather than switching to an untrusted mirror. |
| **Package install permissions** | Automatic inside the ephemeral builder/container; on the host, only via `scripts/bootstrap-linux-dev.sh` with explicit `apt` list | Never `sudo pip install`, never modify host `sources.list` without owner instruction. |
| **System-setting changes** | None automatically (no `gsettings set`, no PipeWire config write) except where the product’s own settings require it and the change is reversible via `Settings → Reset` | Record as `B-SYS-SETTING` if needed. |
| **Privileged operations** | Deny by default (`/dev/uinput`, raw `evdev`, `input` group) — only as opt-in advanced backend per Part C §5 | If a probe shows PTT needs `uinput`, do not enable it; instead limit 1.0 to toggle and document. |

## Cleanup

- Disposable spikes (`tools/p0f-probes/`) and build artifacts (`target/`, `dist/`, `artifacts/tmp/`) are cleaned by `cargo clean` or explicit `rm -rf` of the ephemeral dir; they are never committed (see `.gitignore`).
- `doctor` checks disk/cache budget and reports actionable remediation.

## Enforcement

- `scripts/doctor.py --json` reports `resource_policy: {model_budget_gb, cache_used_gb, disk_free_gb}`.
- `validate_pack.py` does not enforce runtime resource use, but `phase_gate.py` checks that `B-MODEL-SIZE` or similar blockers are resolved before advancing a phase that needs large models.
