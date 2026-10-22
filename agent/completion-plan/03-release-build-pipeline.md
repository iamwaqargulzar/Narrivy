# WP3 — One scripted, reproducible release build

**Goal:** one command turns a clean commit into the exact `.deb` that every
recorder uses. It then passes a hands-on native smoke test.

**Why:** the rc57 package (`artifacts/releases/narrivy_0.1.0~rc57+gb1e16ce_amd64.deb`)
was built by hand, and its exact commands are not recorded anywhere in the
repo. Worse, `artifacts/releases/narrivy-linux-amd64.deb` (the fixed path every
recorder reads; see `ARTIFACT =` in `scripts/record_*.py`) still holds the
**2026-09-22** package. Any recorder run today would qualify a stale build.

Depends on: WP2 done (or at least T2.1, T2.4 and T2.5, so the package contains
the fixed unit), and D3 answered (version).

---

## T3.1 — Write `scripts/build_release_candidate.sh`

Build this script from the pieces that already exist. Do not replace
`scripts/build_deb_installable.py`, which is the reviewed, fail-closed
builder (ADR-0067).

The script must:

1. **Refuse a dirty tree.** Require `git status --porcelain` to be empty.
   Record `HEAD` as `SOURCE_HEAD`, and `SOURCE_DATE_EPOCH` from
   `git log -1 --format=%ct`.
2. **Build the front end:** `npm ci && npm run build` in
   `apps/narrivy-desktop`. The GTK shell embeds `apps/narrivy-desktop/dist`
   at compile time (`src-gtk/build.rs`), so this must happen before the Rust
   build.
3. **Build the release binaries into a fresh target directory** (for example
   `CARGO_TARGET_DIR=$SCRATCH/target-$SOURCE_HEAD`):
   ```bash
   cargo build --release --locked -p narrivyd --features asr-transcribe-resident
   cargo build --release --locked -p narrivy-cli -p narrivy-desktop-gtk
   cargo build --release --locked -p narrivy-asr-transcribe --features resident-worker --bin narrivy-transcribe-worker
   ```
   Check the binary names produced (`narrivyd`, `narrivy-cli`, the GTK shell's
   `[[bin]]` name, `narrivy-transcribe-worker`) against the names
   `build_deb_installable.py` installs. The older recipe in
   `agent/baselines/recovery-phase-4q-c4-disposable-debian-candidate.md` used
   `-p narrivy-desktop`, which was the Tauri package; the shipped desktop is
   now `narrivy-desktop-gtk`.
4. **Locate the model** by its registry pin in
   `crates/narrivy-model-manager/seed/registry.json` (id from the unit's
   `NARRIVY_TRANSCRIBE_MODEL_ID`). Verify its SHA-256 and size against the pin
   before use; never trust a path alone. If the file is not in
   `~/.cache/narrivy`, stop and ask the owner where the pinned file is kept.
5. **Call the builder in clean-source mode** with every input it requires
   (see `--help`):
   `python3 scripts/build_deb_installable.py --clean-source --source-root "$PWD" --source-head "$SOURCE_HEAD" --source-date-epoch "$SOURCE_DATE_EPOCH" --version "$VERSION" ... --output artifacts/releases/narrivy_${VERSION}_amd64.deb`.
   Use the packaging assets from `packaging/deb/` and `packaging/linux/icons/`,
   as `tests/packaging/test_build_deb_candidate.py` does.
6. **Publish to the fixed path** by copying the output to
   `artifacts/releases/narrivy-linux-amd64.deb`. First move the previous
   fixed-path file to `artifacts/releases/narrivy-previous-amd64.deb`, which
   is the upgrade recorder's `PREVIOUS` input. Print both SHA-256 values.
7. **Build the supply-chain bundle** with
   `scripts/build_candidate_supply_chain.py` for the same package (read its
   `--help`). Output goes under `artifacts/candidates/`.
8. **Write a build record** to `artifacts/candidates/<version>-build.json`:
   source head, version, epoch, toolchain versions, every input's SHA-256, and
   the output SHA-256.

Inputs: `VERSION` (from D3, for example `1.0.0~rc1`) as the only argument.

**Acceptance:**
- running the script twice on the same commit produces byte-identical `.deb`
  files (the builder already claims reproducibility; prove it once);
- `dpkg-deb --info` shows the D3 version, the `wtype` recommendation, and no
  `Build-Using` field;
- `dpkg --compare-versions <new> gt 0.2.3` succeeds.

Add a short test in `tests/packaging/` for the script's argument checks and
dirty-tree refusal. Do not run the real build inside the test.

Document the script in `scripts/README.md` under "Debian recovery
candidates".

## T3.2 — Fix package metadata (F-M6)

In `scripts/build_deb_installable.py` (`_control`):
- remove `Build-Using`;
- set `Maintainer: Waqar Gulzar <iamwaqargulzar@gmail.com>` (D3) and `Homepage: https://github.com/iamwaqargulzar/narrivy` (D18); remove every `narrivy.local` value;
- leave the WebKitGTK floor as is for the Pop!_OS release (F-M5 moved to side quest SQ1).

Update `packaging/deb/control` and `packaging/linux/deb/control` to match, and
the tests that assert those fields.

Second-pass additions to the same task:
- **F-C4 (reconfirmed):** `Depends: wtype`. It's in Pop!_OS `noble/universe`
  at `0.4-3`. Assert it in the package test.
- **F-H15:** maintainer scripts.
  - `prerm remove`/`upgrade` stops running user instances (bounded; no home
    traversal).
  - `postinst configure` does **not** enable the unit for anyone (D5: off
    by default; users opt in through the app's "Start Narrivy at login").
    It only does a bounded user-manager `daemon-reload` and a restart of
    already-active instances.
  - The `postrm purge` message includes `~/.local/state/narrivy`.
  - Drop the call to the nonexistent `/usr/bin/narrivy-migrate`.
- **F-M12:** one app ID. Rename the desktop file to match the GTK application
  ID (or the reverse), and set `StartupWMClass` to it. Use one version source
  for the Cargo crates and the Debian `Version`.
- **F-M14:** the builder refuses a `narrivyd` built without
  `asr-transcribe-resident`, for example by checking the capability it
  reports in a bounded `--capabilities`-style probe (add the flag if none
  exists).
- **F-M16:** add the missing composition test for
  `build_deb_installable.py` `_inspect` and `build()`: root ownership, exact
  modes, the pinned model digest, and 0755 maintainer scripts.

## T3.3 — Native smoke of the installed package on COSMIC (F-H6)

This is a hands-on check, not evidence. Its purpose is to find breakage
before the formal campaign.

1. Build a candidate with T3.1.
2. **Ask the owner** before installing, because it replaces the installed
   0.2.3. Then run
   `sudo apt install ./artifacts/releases/narrivy-linux-amd64.deb`. The owner
   may need to type the sudo password; never handle it yourself.
3. Check each of these and write the result in `PROGRESS.md`:
   - `systemctl --user daemon-reload && systemctl --user start narrivy-daemon`
     leaves the service active, and `journalctl --user -u narrivy-daemon`
     shows no errors;
   - `narrivy-cli status` reports the routed lane, the model ready, and
     `wtype` available;
   - bind `/usr/bin/narrivy-cli toggle` to a COSMIC custom shortcut. With a
     text editor focused, toggle, speak a sentence, toggle, and the text
     appears **once** (owner-present step);
   - the same into a terminal, a browser text box, and LibreOffice (if
     installed);
   - with the daemon stopped, pressing the shortcut starts it (T2.5);
   - log out and back in. With "Start at login" **off** (the default), the
     first shortcut press starts the daemon on demand and dictation works.
     Turn the setting **on**, log out and back in, and the daemon is already
     running with `WAYLAND_DISPLAY` set (T2.5/D5);
   - `systemctl --user stop` completes within 3 s, with no leftover
     `narrivy-transcribe-worker` or `pw-record` processes (`pgrep -a narrivy`,
     `pgrep -a pw-record`);
   - `sudo apt remove narrivy` then reinstall works; `purge` behaves as
     `postrm` documents;
   - **second pass, each item tied to a finding:**
     - **F-C8, do this first:** focus a COSMIC Text window, then run
       `sleep 3; wtype 'hello café — it’s'` from a terminal. Readable text
       must appear, not numbers. Repeat through the installed daemon once
       F-C1 is fixed. If it fails, stop and implement the native
       virtual-keyboard rung (`09-cross-platform-architecture.md` §3) before
       any other delivery work.
     - **F-C5:** on a fresh user with no cache, click "Install model" in the
       app (or run the CLI equivalent). It reaches Ready with no restart.
     - **F-C6:** the first dictation after install produces text. The daemon
       log has no `model_unavailable`.
     - **F-H5:** with "Start at login" on, reboot, log in, and don't touch
       anything. `tr '\0' '\n' < /proc/$(pgrep -x narrivyd)/environ | grep WAYLAND_DISPLAY`
       prints a value, and the first shortcut press types text.
     - **D6/D11 History:** dictate twice. Both appear in History, and the
       limit defaults to 10. Make typing fail (stop wtype's access, or pick a
       target that rejects it): the entry is flagged "not delivered", and
       Copy and Retry work. Set the limit to 0: nothing new is stored.
     - **D13/D14:** an alias from the Dictionary screen is applied. The
       microphone picker switches the input device, and unplugging a USB mic
       gives a clear error; the next dictation after replugging works.
     - **F-H7:** `systemctl --user restart narrivy-daemon`, then immediately
       press the shortcut. It works on the first press (time it).
     - **F-H8:** dictate "café — it’s done". The accented letters and the
       curly apostrophe arrive intact.
     - **F-H9:** dictate "version three point one four at example dot com,
       no one". Clean output has no `3. 14` and no `No 1`.
     - **F-H11:** with the Narrivy window open (so the overlay shows) and
       COSMIC Text focused, dictate. Focus never leaves COSMIC Text, and the
       text lands there. If it doesn't, F-H11 becomes Critical and blocks
       the release.
     - **F-H12:** with the app open, `systemctl --user restart
       narrivy-daemon`, then dictate. The app shows the new session's
       states.
     - **F-H15:** upgrade to a rebuilt `.deb` (bumped version) while
       dictation is idle. `narrivy-cli status` shows the new daemon build.
       After `apt remove`, `pgrep -x narrivyd` finds nothing.
     - **F-M12:** the dock shows the Narrivy icon and name, with no
       duplicate entry.
     - **F-H13:** every visible Settings control changes daemon behaviour
       or is clearly read-only.
4. Every failure becomes a finding in `00-review-findings.md` and a fix task
   in WP2. Rebuild and repeat until the whole list passes.

## T3.4 — (moved) GNOME smoke

Moved to side quest SQ1. Not part of the first release.

---

## WP3 exit check

- T3.1's script works, and its reproducibility is proven;
- the T3.3 checklist passes on Pop!_OS COSMIC;
- all changes are committed, and the manifest is resealed.
