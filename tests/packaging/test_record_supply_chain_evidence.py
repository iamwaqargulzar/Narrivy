from __future__ import annotations

import hashlib
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import build_candidate_supply_chain as supply  # noqa: E402
import qualify_candidate_known_phrase as package_identity  # noqa: E402
import record_supply_chain_evidence as recorder  # noqa: E402

HEAD = "a" * 40
VERSION = "0.1.0~rc-test"
EMBEDDED = {key: {"sha256": chr(98 + i) * 64, "size_bytes": i + 1} for i, key in enumerate(supply.EMBEDDED_KEYS)}

class SupplyChainEvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(); self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        (self.root / "spec/schemas").mkdir(parents=True)
        for name in ("evidence.schema.json", "evidence-index.schema.json", "release-artifacts.schema.json"):
            shutil.copy2(ROOT / "spec/schemas" / name, self.root / "spec/schemas" / name)
        self.package = self.root / "artifacts/releases/narrivy.deb"
        self.package.parent.mkdir(parents=True); self.package.write_bytes(b"exact-package")
        self.bundle = self.root / "artifacts/candidates/bundle"; self.bundle.mkdir(parents=True)
        self.evidence = self.root / recorder.EVIDENCE
        self.index = self.root / recorder.INDEX; self.index.parent.mkdir(parents=True, exist_ok=True)
        self.index.write_text(json.dumps({"schema_version": 1, "policy": "release-evidence-v2-required",
            "legacy_default": {"disposition": "historical-quarantined", "eligible_for_release": False, "reason": "legacy"},
            "qualified_evidence": []}), encoding="utf-8")
        self.identity = supply.EmbeddedIdentity("narrivy", VERSION, HEAD, 1_700_000_000, EMBEDDED)
        self._write_bundle()

    @staticmethod
    def _sha(payload: bytes) -> str:
        return hashlib.sha256(payload).hexdigest()

    def _write_bundle(self) -> None:
        package_payload = self.package.read_bytes(); package_hash = self._sha(package_payload)
        package_path = self.package.relative_to(self.root).as_posix()
        bundle_rel = self.bundle.relative_to(self.root).as_posix()
        spdx = {"SPDXID": "SPDXRef-DOCUMENT", "spdxVersion": "SPDX-2.3",
            "packages": [
                {"SPDXID": "SPDXRef-Package-narrivy", "name": "narrivy", "versionInfo": VERSION,
                 "licenseDeclared": "MIT", "licenseConcluded": "MIT"},
                {"SPDXID": "SPDXRef-Package-dep", "name": "dep", "versionInfo": "1.0.0",
                 "licenseDeclared": "MIT", "licenseConcluded": "MIT",
                 "checksums": [{"algorithm": "SHA256", "checksumValue": "b" * 64}]},
            ], "relationships": [{"spdxElementId": "SPDXRef-Package-narrivy",
                "relationshipType": "DEPENDS_ON", "relatedSpdxElement": "SPDXRef-Package-dep"}]}
        provenance = {"_type": "https://in-toto.io/Statement/v1",
            "predicateType": "https://narrivy.local/provenance/unqualified-candidate/v1",
            "subject": [{"name": package_path, "digest": {"sha256": package_hash}}],
            "predicate": {"classification": supply.PROVENANCE_CLASSIFICATION,
                "package": supply.PACKAGE_NAME, "source_commit": HEAD, "version": VERSION,
                "embedded": EMBEDDED, "cargo_lock_hash": "c" * 64,
                "builder": {"os": "test", "image": "test", "image_digest": "test",
                    "toolchain": "test"}}}
        spdx_payload = (json.dumps(spdx, sort_keys=True) + "\n").encode()
        provenance_payload = (json.dumps(provenance, sort_keys=True) + "\n").encode()
        manifest = {"schema_version": 3, "version": VERSION,
            "build_identity": f"narrivy-{VERSION}+{HEAD[:7]}", "git_commit": HEAD, "git_dirty": False,
            "builder_identity": {"os": "test", "image": "test", "image_digest": "test",
                "toolchain": "test", "cargo_lock_hash": "c" * 64},
            "artifacts": [
                {"kind": "deb", "path": package_path, "sha256": package_hash, "size_bytes": len(package_payload)},
                {"kind": "sbom", "path": f"{bundle_rel}/spdx.json", "sha256": self._sha(spdx_payload), "size_bytes": len(spdx_payload)},
                {"kind": "provenance", "path": f"{bundle_rel}/provenance.json", "sha256": self._sha(provenance_payload), "size_bytes": len(provenance_payload)}],
            "sbom_path": f"{bundle_rel}/spdx.json", "provenance_path": f"{bundle_rel}/provenance.json",
            "signing": {"signed": False, "status": "not-supported", "key_id": None, "signature_path": None}}
        release_payload = (json.dumps(manifest, sort_keys=True) + "\n").encode()
        summary = {"classification": supply.BUNDLE_CLASSIFICATION, "candidate_sha256": package_hash,
            "candidate_size_bytes": len(package_payload), "git_commit": HEAD, "version": VERSION,
            "release_qualified": False, "requirement_evidence": False,
            "files": {"spdx.json": {"sha256": self._sha(spdx_payload), "size_bytes": len(spdx_payload)},
                "provenance.json": {"sha256": self._sha(provenance_payload), "size_bytes": len(provenance_payload)},
                "release-artifacts.json": {"sha256": self._sha(release_payload), "size_bytes": len(release_payload)}}}
        (self.bundle / "spdx.json").write_bytes(spdx_payload)
        (self.bundle / "provenance.json").write_bytes(provenance_payload)
        (self.bundle / "release-artifacts.json").write_bytes(release_payload)
        (self.bundle / "result.json").write_text(json.dumps(summary) + "\n", encoding="utf-8")

    LOCKFILE = (
        b'version = 4\n\n[[package]]\nname = "dep"\nversion = "1.0.0"\n'
        b'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
        b'checksum = "' + b"b" * 64 + b'"\n'
    )

    def _record(self, lockfile: bytes | None = None):
        with mock.patch.object(package_identity, "_require_clean_source", return_value=(self.root, HEAD)), \
             mock.patch.object(package_identity, "_prepare_package", return_value=(self.identity, {})), \
             mock.patch.object(supply, "_read_lockfile", return_value=lockfile or self.LOCKFILE):
            return recorder.record(self.package, self.bundle, self.root, self.evidence, self.index,
                ["python3", "scripts/record_supply_chain_evidence.py", "package", "bundle"])

    def _mutate(self, name: str, change) -> None:
        path = self.bundle / name; value = json.loads(path.read_text()); change(value)
        path.write_text(json.dumps(value) + "\n", encoding="utf-8")

    def _rebind_bundle_hash(self, name: str, kind: str) -> None:
        payload = (self.bundle / name).read_bytes()
        manifest_path = self.bundle / "release-artifacts.json"
        manifest = json.loads(manifest_path.read_text())
        artifact = next(item for item in manifest["artifacts"] if item["kind"] == kind)
        artifact.update(sha256=self._sha(payload), size_bytes=len(payload))
        release_payload = (json.dumps(manifest, sort_keys=True) + "\n").encode()
        manifest_path.write_bytes(release_payload)
        summary_path = self.bundle / "result.json"
        summary = json.loads(summary_path.read_text())
        summary["files"][name] = {"sha256": self._sha(payload), "size_bytes": len(payload)}
        summary["files"]["release-artifacts.json"] = {
            "sha256": self._sha(release_payload), "size_bytes": len(release_payload)}
        summary_path.write_text(json.dumps(summary) + "\n", encoding="utf-8")

    def test_success_registers_measured_schema_v3_evidence(self) -> None:
        evidence = self._record()
        self.assertEqual(evidence["schema_version"], 3)
        self.assertEqual(evidence["verification_id"], "VER-OPS-004")
        registered = json.loads(self.index.read_text())["qualified_evidence"]
        self.assertEqual(registered[0]["path"], recorder.EVIDENCE)
        self.assertEqual(registered[0]["sha256"], self._sha(self.evidence.read_bytes()))

    def test_every_recorded_claim_is_backed_by_a_resolved_measurement(self) -> None:
        import sys as _sys

        _sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
        from evidence_policy import assessment_errors_for_record  # noqa: PLC0415

        evidence = self._record()
        for assertion in evidence["procedure"]["assertions"]:
            with self.subTest(assertion=assertion["id"]):
                self.assertIn("measurement", assertion)
                self.assertEqual(
                    [],
                    assessment_errors_for_record(evidence, assertion_id=assertion["id"]),
                )
        self.assertEqual(
            sorted(recorder.ASSERTIONS),
            sorted(a["id"] for a in evidence["procedure"]["assertions"]),
        )

    def test_package_hash_tampering_fails_closed(self) -> None:
        self.package.write_bytes(b"tampered")
        with self.assertRaisesRegex(recorder.VerificationError, "package checksum"):
            self._record()
        self.assertFalse(self.evidence.exists())

    def test_provenance_subject_tampering_fails_closed(self) -> None:
        self._mutate("provenance.json", lambda value: value["subject"][0]["digest"].update(sha256="0" * 64))
        self._rebind_bundle_hash("provenance.json", "provenance")
        with self.assertRaisesRegex(recorder.VerificationError, "in-toto"):
            self._record()

    def test_empty_spdx_inventory_fails_closed(self) -> None:
        self._mutate("spdx.json", lambda value: value.update(packages=[]))
        self._rebind_bundle_hash("spdx.json", "sbom")
        with self.assertRaisesRegex(recorder.VerificationError, "SPDX dependency"):
            self._record()

    def test_claimed_signed_manifest_fails_closed(self) -> None:
        self._mutate("release-artifacts.json", lambda value: value["signing"].update(
            signed=True, status="signed", key_id="release-key",
            signature_path="artifacts/candidates/bundle/narrivy.deb.sig"))
        payload = (self.bundle / "release-artifacts.json").read_bytes()
        summary_path = self.bundle / "result.json"
        summary = json.loads(summary_path.read_text())
        summary["files"]["release-artifacts.json"] = {
            "sha256": self._sha(payload), "size_bytes": len(payload)}
        summary_path.write_text(json.dumps(summary) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(recorder.VerificationError, "signing state is inconsistent"):
            self._record()
        self.assertFalse(self.evidence.exists())

    def test_dirty_or_mismatched_source_fails_closed(self) -> None:
        with mock.patch.object(package_identity, "_require_clean_source", side_effect=package_identity.InputError("source worktree is dirty")):
            with self.assertRaisesRegex(recorder.VerificationError, "dirty"):
                recorder._verify_bundle(self.package, self.bundle, self.root)
        with mock.patch.object(package_identity, "_require_clean_source", return_value=(self.root, "f" * 40)), \
             mock.patch.object(package_identity, "_prepare_package", side_effect=package_identity.InputError("package build identity does not match source HEAD")):
            with self.assertRaisesRegex(recorder.VerificationError, "does not match"):
                recorder._verify_bundle(self.package, self.bundle, self.root)

    def _index_entry(self, sha: str) -> None:
        index = json.loads(self.index.read_text()); index["qualified_evidence"].append(
            {"path": recorder.EVIDENCE, "sha256": sha, "disposition": "qualified"})
        self.index.write_text(json.dumps(index))

    def test_nonhistorical_output_and_index_collisions_fail_closed(self) -> None:
        # An unindexed v2 file is not this recorder's indexed output.
        self.evidence.parent.mkdir(parents=True); self.evidence.write_text('{"schema_version":2}\n')
        with self.assertRaisesRegex(recorder.VerificationError, "not match its INDEX hash"):
            self._record()
        # An indexed v2 record whose bytes no longer match INDEX is refused.
        self._index_entry("a" * 64)
        with self.assertRaisesRegex(recorder.VerificationError, "not match its INDEX hash"):
            self._record()
        # A current (v3) indexed record is never replaced.
        payload = b'{"schema_version":3}\n'; self.evidence.write_bytes(payload)
        index = json.loads(self.index.read_text()); index["qualified_evidence"][0]["sha256"] = self._sha(payload)
        self.index.write_text(json.dumps(index))
        with self.assertRaisesRegex(recorder.VerificationError, "not match its INDEX hash"):
            self._record()
        self.assertEqual(self.evidence.read_bytes(), payload)

    def test_indexed_hash_matching_v2_output_is_replaced_in_one_transaction(self) -> None:
        # P-4: the recorder must be able to replace its own indexed v2 output.
        old = b'{"schema_version":2}\n'
        self.evidence.parent.mkdir(parents=True); self.evidence.write_bytes(old)
        self._index_entry(self._sha(old))
        evidence = self._record()
        self.assertEqual(evidence["schema_version"], 3)
        entries = [e for e in json.loads(self.index.read_text())["qualified_evidence"] if e["path"] == recorder.EVIDENCE]
        self.assertEqual(len(entries), 1)
        self.assertEqual(entries[0]["sha256"], self._sha(self.evidence.read_bytes()))

    def test_noassertion_licence_fails_closed(self) -> None:
        # P-4: NOASSERTION is not a licence for a shipped component.
        self._mutate("spdx.json", lambda value: value["packages"][1].update(licenseDeclared="NOASSERTION"))
        self._rebind_bundle_hash("spdx.json", "sbom")
        with self.assertRaisesRegex(recorder.VerificationError, "NOASSERTION"):
            self._record()
        self.assertFalse(self.evidence.exists())

    def test_sbom_checksum_must_match_cargo_lock(self) -> None:
        # P-4: an SBOM checksum is verified against Cargo.lock, not just its shape.
        lockfile = self.LOCKFILE.replace(b"b" * 64, b"d" * 64)
        with self.assertRaisesRegex(recorder.VerificationError, "do not match Cargo.lock"):
            self._record(lockfile)
        unknown = self.LOCKFILE.replace(b'name = "dep"', b'name = "other"')
        with self.assertRaisesRegex(recorder.VerificationError, "absent from Cargo.lock"):
            self._record(unknown)
        self.assertFalse(self.evidence.exists())

if __name__ == "__main__":
    unittest.main()
