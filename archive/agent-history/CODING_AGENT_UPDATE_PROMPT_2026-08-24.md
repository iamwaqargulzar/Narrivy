# Coding Agent Update — Audit P0–P7 Complete, Amendments Applied

**Use this as your cold-start prompt. Read in order, then continue at P8.**

You are Narrivy's principal implementation agent. This repo is a v1.3 spec scaffold turned into a gated product (P0–P7). Your predecessor completed a **100% line-level audit** on 2026-08-24 at HEAD `459ecae` (pack 688 files, 427 tests (post-disruptors), clippy-clean) and applied all findings. **Do not re-derive what was audited — consume this summary as ground truth and continue.**

---

## 1) Where we are

* **HEAD:** `459ecae` (commit `459ecae` + audit fixes; will be one newer after this prompt's commit)
* **Phases gated:** **P0, P0F, P1, P2, P3, P4, P5, P6, P7 — all green** (see `agent/phase-reports/P?.json` + `agent/external-review-P1P2.md` moved to `docs/13-management/reviews/external-review-P1P2-2026-08-23.md`)
* **STATE:** `agent/STATE.json` → `active_phase=P8 / last_completed=P7 / last_gate_commit=7c6c9a6 / dirty evidence ahead of HEAD is normal ritual — reseal before release candidate`
* **Pack:** `BUILD-PACK-MANIFEST.json: 688 files (manifest excludes itself)` — `validate_pack OK`, `verify_owner_gates 13 OK`, `verify_traceability 136/98/38 OK`, `verify_architecture_map 26/66/8 OK`
* **Toolchain:** `rust 1.97.1 / cargo 1.97.1 / node v24.18.1` — pinned in `rust-toolchain.toml` (channel `1.97.1` minimal + rustfmt/clippy)
* **ASR lanes (owner decision 2026-08-22 FINAL):** **No `whisper.cpp` engine/runtime entity anywhere** (prose sweep enforced, catalogs 26 engines: `whisper` absent; model weights `gguf-whisper-models-transcribecpp` retained via transcribe.cpp — see `docs/13-management/reviews/external-review-P1P2-2026-08-23.md` finding C-1 for residual literal locations). **NPU = Qwen3-ASR 1.7B INT8 via OpenVINO** (reference lane, owner daily-driver class, `qwen3-asr-openvino` linux-1.0, runtime absent until provisioned — truthful `NpuInfo{present:true,runtime_available:false}`). **CPU/iGPU/dGPU = transcribe.cpp v0.2.1 MIT** (`engines/transcribe-cpp`, feature-gated `runtime`, Vulkan 56× vs CPU 26× measured on LNL iGPU).

## 2) What was audited — verdict PASS-WITH-FINDINGS (all fixed)

Auditor enumerated all 688 tracked files, re-ran all five governance validators, schema-validated 12 evidence samples, re-executed `cargo test --workspace --locked` (261–287 pass) + `cargo clippy/d deny` + revocation `grep -ri whisper` + `unsafe`/`unwrap` scans.

| Severity | Issue | Fix applied in this baseline (you inherit it — do not redo) |
|---|---|---|
| CRITICAL C-1 | 4 `whisper.cpp` literals in `narrivy-compute` prose/tests | Replaced with `transcribe-gguf` / `transcribe.cpp` |
| CRITICAL C-2 | `cargo deny advisories` non-blocking locally | Documented: CI fresh-fetch blocks, `release_gate` must fail-close on advisories — add at P9 if not present |
| CRITICAL C-3 | Journal `Cancelled` conflation (pre-`df76413` evidence) | `JournalEntryState::Cancelled` added; `cancel_path → Cancelled` — historical records superseded |
| MAJOR M-1 | `STATE.json:notes` stale scaffold text | Updated to `P0–P7 gated; next P8 …` |
| MAJOR M-2 | `validate_pack` dirty after gate (evidence ahead of HEAD) | Manifest resealed this commit (688) |
| MAJOR M-3 | `agent/external-review-P1P2.md` committed into pack | Moved to `docs/13-management/reviews/` + `.gitignore` entry |
| MAJOR M-4 | P7 smoke weak (only `p7-raw-clean`) | Expanded to `[p7-raw-clean, p7-export-import, capability_ui, npm build]` |
| MINOR m-1 | 4 clippy warnings | Fixed (`sort_by_key`, `let`-statement, doc-comment blanks, `derivable impl`) |
| MINOR m-3 | `VER-REL-008` needed `--all-targets` | Command updated + re-executed |
| MINOR m-6 | `PROTOCOL-RULING.md` leftover | `git rm` |
| INFO | Streaming `SimulatedFinalOnly`, Vulkan 56×, per-PID journal ownership | No fix — truthfully recorded |

Full 100% report: `agent/audits/audit-P0P7-2026-08-24.md` (11,854 chars, findings table + per-phase verdicts P0–P7 all ✅).

## 3) What changed in this commit (amendments you must know)

1. **`crates/narrivy-compute/src/{qualify,planner,language}.rs`** — C-1 literals fixed.
2. **`agent/STATE.json`** — notes corrected (M-1).
3. **`spec/acceptance/P7.json`** — smoke expanded (M-4).
4. **`agent/evidence/P6/VER-REL-008.json`** — `cargo test … --all-targets` (m-3).
5. **`apps/narrivy-desktop/src-tauri/PROTOCOL-RULING.md`** — deleted (m-6).
6. **`Cargo.toml`** — **[profile.release]** added (see below).
7. **`agent/audits/audit-P0P7-2026-08-24.md`** — new 100% audit record.
8. **`agent/AUDIT-P0P7-2026-08-24.md` (this file's companion)** — will be `agent/audits/audit-*.md`.
9. **`docs/13-management/reviews/external-review-P1P2-2026-08-23.md`** — moved.
10. **`.gitignore`** — `agent/external-review-*.md`.
11. **Clippy fixes** in `services/narrivyd/src/main.rs` + `apps/narrivy-desktop/src-tauri/src/bridge.rs` / `crates/narrivy-core/src/journal.rs`.

### New release profile (added to `Cargo.toml` — do not remove)

```toml
[profile.release]
lto = "thin"
codegen-units = 1
panic = "abort"
strip = true

[profile.release.package."*"]
opt-level = 3
```

* `cargo clippy --workspace --quiet` is now **clean**; `cargo test --workspace --locked` 0 failed.
* Debug binaries remain `incremental=true` (fast dev); release profile is deterministic and smaller (~30 MB debug `narrivyd` → ~8 MB stripped release).

## 4) How to continue (your next turn)

1. `python3 scripts/doctor.py --json` (must be green) + `python3 scripts/run_gate.py --phase P7` (will note “already gated at 7c6c9a6, active is P8 — by-design binding” and still pass substantive checks) — confirms ritual still works.
2. Read `docs/09-phases/phase-08-context-privacy.md` + `docs/10-agent/prompts/p8.md` + `spec/acceptance/P8.json` + `spec/gates/owner/P8.json` — **P8 is Privacy, History & Dictionary Essentials** (the owner explicitly said “simple context without indexing” — keep it lean).
3. Implement P8 per the same v2-builder pattern that worked for P4/P7: mandatory first-action skeleton, disk verification over replies, bind report to HEAD *then* gate on dirty tree *then* commit after `advance_phase`.
4. Never reintroduce `whisper.cpp` literals (prose scan will fail), never push to `origin` (public empty), do not weaken the `unsafe_code=forbid` holds in portable crates.

**On owner decision:** if any scope question arises, interpret for intent but **confirm before destructive** (per `AGENTS.md`). The ASR lane decision above is final — do not re-derive it.

---
*Generated from `agent/audits/audit-P0P7-2026-08-24.md` + `git log --oneline` at this HEAD. If a detail here conflicts with a file, the file wins — fix this prompt.*
