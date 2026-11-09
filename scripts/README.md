# Scripts

Helper scripts for developing, checking and packaging Narrivy. Run them from
the repository root.

## Set up a development machine

```bash
python3 scripts/doctor.py                  # what this machine offers (read-only)
scripts/bootstrap-linux-dev.sh             # list the Debian/Ubuntu packages needed
scripts/bootstrap-linux-dev.sh --install   # install them (review the list first)
```

## Run the app in development

Linux:

```bash
scripts/dev-daemon.sh            # real dictation: microphone, resident speech model, typing
scripts/dev-desktop-entry.sh     # launcher icon for the development build (--remove to undo)
(cd apps/narrivy-desktop && npm ci && npm run build) && cargo run -p narrivy-desktop
```

Windows (an ordinary, unelevated PowerShell in the desktop session):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\dev-daemon.ps1
```

`dev-daemon` binds the speech worker and model by exact size and SHA-256.
Without it, `narrivyd` runs a mock engine: sessions finish at once and nothing
is typed.

`scripts/install-dev-service.sh` installs a user service that runs this
checkout's daemon, and `scripts/install-browser-extension.sh` registers the
browser connector for Chrome and Brave.

## Checks

The smallest check that covers a change is enough while editing; CI runs all
of them.

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check

# Desktop frontend
npm test --prefix apps/narrivy-desktop

# Python suites (packaging, hardening, repository rules)
python3 -m pytest tests -q

# Architecture and repository rules
python3 scripts/verify-boundaries.py            # portable crates use no OS APIs
python3 scripts/verify_dependency_boundaries.py # and depend on nothing OS-specific
python3 scripts/verify_frontend_boundaries.py   # the UI names no OS, engine or model
python3 scripts/verify_architecture_map.py      # catalogs in spec/catalogs agree
python3 scripts/verify_pipeline_contracts.py    # pipeline examples are valid
python3 scripts/check_log_redaction.py          # logs never carry content
python3 scripts/verify_secrets.py               # no committed credentials
python3 scripts/check_action_pins.py            # GitHub Actions pinned by SHA
python3 scripts/check_docs.py                   # relative Markdown links resolve
```

Compile check of the portable crates for Windows from Linux (needs
`rustup target add x86_64-pc-windows-gnu`):

```bash
python3 scripts/check_windows_portable.py
```

Longer property-test runs over the untrusted-input boundaries (IPC frames,
archives, manifests); the same tests run with small case counts in
`cargo test`:

```bash
scripts/fuzz-untrusted-input.sh [cases]
```

## Build the packages

Linux `.deb`, from a clean commit:

```bash
scripts/build_release_candidate.sh 1.0.0~rc33
```

It builds the frontend and the release binaries (the speech worker with
Vulkan), checks the speech model against its registry pin, and writes the
package to `artifacts/releases/`. `build_deb_installable.py`,
`stage_installed_runtime.py` and `build_candidate_supply_chain.py` (SPDX SBOM
and provenance) are the steps it runs.

Windows `.msi`, on Windows:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build_windows_msi.ps1
```

The script's header lists what it needs (MSVC build tools, Rust, tauri-cli 2,
the Vulkan SDK and the built frontend).

## Package tests

- `qualify_candidate.py`: installs one exact `.deb` in a disposable Ubuntu
  24.04 container and checks install, model provisioning, restart, removal
  and purge.
- `qualify_candidate_upgrade.py`: the same for an upgrade from the previous
  package and back.
- `qualify_candidate_known_phrase.py`: plays a public test clip through the
  speakers and checks that the packaged daemon hears and transcribes it
  (audible; asks for `--confirm-audible-playback`).

## Add-on packs and the Store

`build_openvino_runtime_pack.py`, `build_llm_pack.py`, `build_voice_pack.py`
and `build_browser_pack.py` assemble the optional packs the Store offers;
`build_store_catalog.py` writes the model list the Store publishes.

## Workers

`voice_filter.py` ("Only my voice"), `meeting.py` (meeting transcripts with
speakers), `screen_capture.py` (meeting screen pictures) and `voice_mode.py`
are the Python workers the daemon starts for those features.

## README screenshots

```bash
npm --prefix apps/narrivy-desktop run dev -- --port 1420   # in another terminal
node scripts/readme_screenshots.mjs --chrome /path/to/chrome-headless-shell
```

The pictures come from the UI's showcase mode, which holds invented content
only.
