"""harness_journal_torn_write — torn-write + .tmp quarantine (P9).

Extends foundation-hardening proof: 500 entries with 50 truncated at random
offsets, stray .tmp ignored, survivors ordered and readable after restart.
Retains 100/15 and 10/3 compatibility for skeleton tests.

Fail-closed, no transcript leak.
"""
from __future__ import annotations
import json
import random
import tempfile
from pathlib import Path

from .common import (
    write_journal_entry_atomic,
    journal_dir,
    corrupt_tail,
    quarantine_corrupt,
    recoverable_entries,
    harness_result,
    assert_no_transcript_leak,
)

def _corrupt_random_offsets(jdir: Path, entry_ids: list[str], seed: int = 42) -> list[Path]:
    """Truncate each entry's file at a random offset to simulate power-loss torn write."""
    rnd = random.Random(seed)
    corrupted = []
    for eid in entry_ids:
        p = jdir / f"{eid}.json"
        if not p.exists():
            continue
        data = p.read_bytes()
        # random keep bytes: half, empty, 1 byte, random mid, etc. Choose among distribution
        choice = rnd.choice(["half", "empty", "one", "quarter", "random"])
        if choice == "half":
            keep = len(data) // 2
        elif choice == "empty":
            keep = 0
        elif choice == "one":
            keep = 1
        elif choice == "quarter":
            keep = len(data) // 4
        else:
            keep = rnd.randint(0, max(0, len(data)-1))
            # also show random offset distribution: sometimes truncate not at truncation but garble
            # we just truncate for torn-write
        # also randomize keep to be truly random offset, not just those 5
        # mix: 70% use choice, 30% fully random
        if rnd.random() < 0.3:
            keep = rnd.randint(0, len(data))
        corrupted.append(p)
        p.write_bytes(data[:keep])
    return corrupted

def run(*, stub: bool = True, entries: int = 10, torn: int = 3, verbose: bool = False) -> dict:
    notes = []
    metrics: dict = {}
    passed = False
    diagnostic = ""
    td = tempfile.TemporaryDirectory(prefix="p9-torn-")
    try:
        rt = Path(td.name) / "xdg"
        jdir = journal_dir(rt)
        # write entries: sorted by created_at, time-ordered (monotonic)
        for i in range(entries):
            eid = f"e-{i:04d}" if entries > 999 else f"e-{i:03d}"
            # monotonic created_at: zero-padded seconds ensures lexicographic == numeric order
            write_journal_entry_atomic(jdir, eid, {
                "id": eid,
                "created_at": f"2026-01-01T00:00:{i:06d}Z",
                "text_hash": f"h{i}",
                "delivered": False
            })
        # create stray tmp files (should be ignored, never quarantined as .corrupt)
        (jdir / ".stray.tmp").write_text("partial")
        (jdir / ".orphan.tmp").write_text("{}")
        (jdir / ".tx-temp.tmp").write_text("incomplete json {")
        metrics["written"] = entries
        metrics["torn_injected"] = torn

        # choose which entries to corrupt: random sample for large, deterministic last N for small (but still pass counts)
        # To satisfy mission: 500 entries truncate 50 at random offsets
        # We use random sample with seed 0xP9 for reproducibility
        rnd = random.Random(0x509)
        all_ids = [f"e-{i:04d}" if entries > 999 else f"e-{i:03d}" for i in range(entries)]
        if entries >= 500 or torn >= 20:
            # random sample for large
            to_corrupt = rnd.sample(all_ids, torn)
            # use random offsets per file
            for eid in to_corrupt:
                p = jdir / f"{eid}.json"
                if p.exists():
                    data = p.read_bytes()
                    # random keep between 0 and len//2 inclusive, sometimes 0,1
                    keep = rnd.randint(0, max(1, len(data)//2))
                    # 10% chance keep 0, 10% keep 1
                    r = rnd.random()
                    if r < 0.1:
                        keep = 0
                    elif r < 0.2:
                        keep = 1
                    p.write_bytes(data[:keep])
            metrics["torn_ids"] = sorted(to_corrupt)
        else:
            # deterministic last N for backward compat (skeleton expects last N)
            # but still use random keep_bytes=10 for those (original behavior)
            # To also satisfy random offsets requirement for 100/15 proof extension, we do mixed: for entries==100 torn==15 use same half/empty/one pattern as journal.rs test?
            # For 10/3, keep 10 bytes
            for i in range(entries - torn, entries):
                eid = f"e-{i:04d}" if entries > 999 else f"e-{i:03d}"
                p = jdir / f"{eid}.json"
                if p.exists():
                    corrupt_tail(p, keep_bytes=10)
            # use same eid formatting as above
            metrics["torn_ids"] = [f"e-{i:04d}" if entries>999 else f"e-{i:03d}" for i in range(entries - torn, entries)]

        # For entries ==500 we also did random offsets, but need to ensure 50 truncated
        # For entries==100 torn==15, we did deterministic, but we should also have tested random offsets variant for 500.
        # So above covers.

        # recovery: quarantine corrupt json, survive others
        quarantined = quarantine_corrupt(jdir)
        survivors = recoverable_entries(jdir)
        metrics["quarantined_count"] = len(quarantined)
        metrics["survivor_count"] = len(survivors)
        metrics["survivor_ids"] = [s["id"] for s in survivors[:10]]  # first 10 for brevity; full count matters
        # full survivor ids for verification (not truncated in metrics for large)
        if entries > 100:
            metrics["survivor_ids_full"] = [s["id"] for s in survivors]
        # stray .tmp files must not appear as survivors and must not crash
        stray_survives = any("stray" in s["id"] for s in survivors)
        ordered = survivors == sorted(survivors, key=lambda d: d["id"])
        # Additional checks for 500/50: survivors must be exactly entries-torn, sorted, no torn in survivors
        if entries >= 500:
            # ensure quarantined are exactly torn and survivors are entries-torn
            expected_survivors = entries - torn
            torn_set = set(metrics.get("torn_ids", []))
            survivor_set = set(s["id"] for s in survivors)
            overlap = torn_set & survivor_set
            metrics["overlap_torn_survivor"] = len(overlap)
            metrics["expected_survivors"] = expected_survivors
            # also check that stray .tmp not quarantined as .corrupt
            stray_corrupt = (jdir / "stray.corrupt").exists() or (jdir / ".stray.tmp.corrupt").exists()
            metrics["stray_corrupt_exists"] = stray_corrupt
            # need to ensure truncated files left .corrupt
            metrics["quarantined_names"] = [p.name for p in quarantined]
            passed = (
                len(survivors) == expected_survivors
                and not stray_survives
                and ordered
                and len(quarantined) == torn
                and len(overlap) == 0
                and not stray_corrupt
            )
            notes.append(f"500/50 extended proof: entries {entries} torn {torn} survivors {len(survivors)} quarantined {len(quarantined)} random_offsets ok={passed}")
        else:
            passed = (
                len(survivors) == entries - torn
                and not stray_survives
                and ordered
                and len(quarantined) == torn
            )
            notes.append(f"entries {entries} torn {torn} survivors {len(survivors)} quarantined {len(quarantined)} ordered={ordered}")
        diagnostic = f"survivors={metrics['survivor_ids']} quarantined={[p.name for p in quarantined][:5]} torn_ids={metrics.get('torn_ids',[])[:5]}"
        # Also verify recoverable after restart (new Journal open would see same)
        # Simulate by re-reading via recoverable_entries again
        survivors2 = recoverable_entries(jdir)
        if survivors != survivors2:
            passed = False
            notes.append("restart recoverable mismatch")
        assert_no_transcript_leak(diagnostic)
        assert_no_transcript_leak(" ".join(notes))
        if verbose:
            notes.append(diagnostic)
    except Exception as e:
        import traceback
        tb = traceback.format_exc()
        notes.append(f"exception {e}")
        diagnostic = tb[:3900]
        passed = False
    finally:
        try:
            td.cleanup()
        except Exception:
            pass
    return harness_result("journal_torn_write", passed, metrics, " ".join(notes), diagnostic)


if __name__ == "__main__":
    import json, sys
    # test all proof sizes
    for ents, torn_n in [(10,3),(100,15),(500,50)]:
        res = run(entries=ents, torn=torn_n, verbose=True)
        print(json.dumps(res, indent=2))
        assert res["pass"], f"failed {ents}/{torn_n}"
