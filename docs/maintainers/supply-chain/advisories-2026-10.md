# Dependency advisories, October 2026

Checked 2026-10-29, after the 2026-10-26 dependency refresh (see B-SUPPLY-010
in `agent/BLOCKERS.md`). Policy: `deny.toml`. Nothing was silenced and no gate
was changed.

## Commands and results

| Check | Result |
|---|---|
| `cargo deny check advisories bans licenses sources` (cargo-deny 0.20.2, advisory DB fetched 2026-10-29) | advisories FAILED (8 errors); bans ok; licenses ok; sources ok |
| `npm audit --omit=dev` in `apps/narrivy-desktop` | 0 vulnerabilities (a full `npm audit`, dev included, also reports 0) |
| `cargo update --dry-run --verbose` | no compatible update clears any of the 8 |
| `cargo test -p narrivy-grammar-harper` | 7 passed (baseline, no change) |
| `cargo test -p narrivy-platform-linux` | 310 passed, 4 ignored (baseline, no change) |

**No upgrade was applied.** `Cargo.lock`, the manifests, `deny.toml`,
`package-lock.json` and `THIRD_PARTY_NOTICES.md` are unchanged. Every
advisory below needs a fix upstream or an owner decision.

## Advisories

### 1–3. zip 8.6.0: RUSTSEC-2026-0367, RUSTSEC-2026-0368, RUSTSEC-2026-0369 (vulnerabilities)

- **What they are.** All three are in the zip crate's *extraction to disk*:
  0367 keeps SUID/SGID/sticky bits from the archive; 0368 creates symlinks
  whose targets point outside the destination; 0369 applies permissions
  through such symlinks. Affected calls: `ZipArchive::extract()` and
  `ZipStreamReader::extract()`. Fixed in zip ≥ 9.0.0 (latest is 9.1.3).
- **Path.** `narrivyd → narrivy-grammar-harper → harper-core =2.13.0 → zip 8.6.0`.
  zip is a required (non-optional) dependency of harper-core; no feature
  turns it off.
- **Reachable: no.** harper-core uses zip only in `src/weirpack/mod.rs`:
  `Weirpack::from_bytes` opens an in-memory archive (`ZipArchive::new` on a
  `Cursor`) and reads entries into strings with `by_index`/`read_to_string`;
  the writer builds an archive in memory. It never calls `extract()`, so it
  never writes files, symlinks or permissions to disk. Narrivy never calls
  the weirpack API at all: `engines/grammar-harper/src/lib.rs` uses only
  `Document`, `Dialect`, `FstDictionary`, `LintGroup`, `Lint` and
  `Suggestion`. No Narrivy code depends on zip directly.
- **Also in the lockfile:** zip 7.4.0, needed by `candle-core 0.9.2` through
  `burn-candle 0.19.1`. Burn's candle backend is not enabled, so this copy is
  never built (`cargo tree --target all --all-features -i zip@7.4.0` prints
  nothing, and cargo-deny does not flag it).
- **Why no upgrade.** harper-core 2.13.0 needs `zip ^8.6`, so zip 9 cannot be
  selected. harper-core 3.0.0, now published, *still* needs `zip ^8.6.0` (and
  `paste`). It is also a major version, which can change the API and what the
  writing checker reports, and it clears nothing. It was not adopted.

### 4. bincode 2.0.1: RUSTSEC-2025-0141 (unmaintained)

- **Path.** `harper-core → harper-brill → harper-pos-utils → burn 0.19.1 → burn-core → bincode 2.0.1`.
- **Reachable: compiled in, not used.** burn-core always depends on
  bincode for its bincode record formats. harper loads its chunker model
  with burn's MessagePack recorders (`NamedMpkBytesRecorder` in
  `harper-pos-utils/src/chunker/burn_chunker.rs`), from a `model.mpk` built
  into harper-brill with `include_bytes!`. No bincode recorder is used, and
  Narrivy never decodes bincode. The advisory is a maintenance notice only;
  it reports no vulnerability.
- **Why no upgrade.** burn 0.19 still uses bincode, and burn is a required
  dependency of `harper-pos-utils`. There is no harper release without it.

### 5. paste 1.0.15: RUSTSEC-2024-0436 (unmaintained)

- **Path.** `harper-core → paste` and `harper-pos-utils → burn-ndarray → paste`.
- **Reachable: compile time only.** paste is a proc-macro that builds
  identifiers while compiling. None of its code is in the binary that ships.
- **Why no upgrade.** It is a direct dependency of harper-core 2.13.0 and
  3.0.0. The maintained replacement is `pastey`, and the change has to be
  made upstream.

### 6–8. zvariant 5.15.1, zbus_names 4.3.5, zvariant_derive 5.15.1: RUSTSEC-2026-0370, RUSTSEC-2026-0371, RUSTSEC-2026-0372 (unmaintained)

- **What they are.** These crates are no longer published on their own.
  They were merged into zbus 6.0 (as `zbus::wire`, `zbus::names` and the
  zbus derive macros). The notices report no vulnerability.
- **Path.** `zbus =5.19.0`, needed by `narrivy-platform-linux` (workspace
  pin), `ashpd =0.12.3` (`narrivy-platform-linux`, `narrivy-desktop`) and
  `tauri-plugin-single-instance 2.5.4` (`narrivy-desktop`, Linux only).
- **Reachable: yes, in production on Linux.** `adapters/platform-linux` uses
  `zbus::zvariant` types directly for the portal and EIS code
  (`portal_eis_zbus.rs`, `portal_eis.rs`), Secret Service (`secrets.rs`) and
  the trigger code (`trigger.rs`). The app's single-instance check and portal
  calls also go through them. Being reachable matters less here, since no
  vulnerability is reported. The risk is that a future fix lands only in
  zbus 6.
- **Why no upgrade.** The newest ashpd (0.13.13) needs `zbus ^5.13`. Every
  tauri-plugin-single-instance release (2.5.5, and 3.0.0-alpha.3) needs
  `zbus ^5.9`. If only our own adapter moved to zbus 6, zbus 5 would still be
  in the graph through ashpd and the Tauri plugin, so all three notices would
  stay. It would also add a second D-Bus stack.

## What the owner must decide

`cargo deny check` will keep failing until upstream releases. The choices:

**A. zip vulnerabilities (0367/0368/0369): recommend a new, time-boxed owner
acceptance.** Add the three IDs to `deny.toml` `ignore`, each with a reason
that names this report, as a new owner decision (D22 says nothing else may
be added without one). Re-check them at every harper-core release. Remove
them as soon as harper-core needs zip ≥ 9. Also file an issue or PR on
Automattic/harper to bump zip to 9: harper reads only in-memory archives,
which should make the bump small. I recommend this because the vulnerable
function cannot be called from our build, so the advisories carry no risk
for Narrivy. Rejected alternatives:
- a `[patch.crates-io]` fork of harper-core: we would maintain a fork of a
  large crate for no real risk reduction;
- removing the writing checker (D26): this loses a feature;
- harper-core 3.0.0: it does not fix the advisories and changes behaviour.

**B. bincode and paste (unmaintained): recommend accepting them on the same
terms as the D22 GTK3 notices.** Add both to the ignore list with reasons,
and re-check at every harper-core release. paste never ships in the binary.
bincode is compiled in but never used.

**C. zvariant / zbus_names / zvariant_derive (unmaintained): recommend
accepting them until ashpd and tauri-plugin-single-instance support zbus 6.**
Then move `narrivy-platform-linux` to zbus 6 in the same change. That means
a reviewed migration: the zvariant imports become `zbus::wire`, and the
portal, EIS, secrets and trigger paths need retesting on COSMIC. Re-check at
every Tauri and ashpd release.

If the owner rejects these acceptances, the release gate stays blocked on
B-SUPPLY-010 until upstream releases, and none of these crates can be
cleared from this repository alone.
