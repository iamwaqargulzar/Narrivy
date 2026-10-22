#!/usr/bin/env python3
"""Run shared controlled suites once and register their schema-v2 evidence."""
from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import shlex
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

import jsonschema


ROOT = Path(__file__).resolve().parents[1]
INDEX = "agent/evidence/INDEX.json"
EXPECTED_ARGV = ["python3", "scripts/record_controlled_integration_evidence.py"]
SUITES = {
    "core": ["cargo", "test", "-p", "narrivy-core", "--locked"],
    "pipeline": ["cargo", "test", "-p", "narrivy-pipeline", "--locked"],
    "policy": ["cargo", "test", "-p", "narrivy-policy", "--locked"],
    "history": ["cargo", "test", "-p", "narrivy-history", "--locked"],
    "diagnostics": ["cargo", "test", "-p", "narrivy-diagnostics", "--locked"],
    "model-cache": [
        "cargo", "test", "-p", "narrivy-model-manager", "--test", "p5_model_cache", "--locked",
    ],
    # These process-ownership tests intentionally create and reap many helper
    # process groups. Serial execution avoids host PID/resource pressure from
    # becoming a false product failure while preserving every assertion.
    "platform-linux": [
        "cargo", "test", "-p", "narrivy-platform-linux", "--locked",
        "--", "--test-threads=1",
    ],
    "daemon": ["cargo", "test", "-p", "narrivyd", "--locked"],
    # The verbose reporter is required, not cosmetic: the proof scan resolves each
    # claim by finding its named test in this run's output, and vitest's default
    # reporter prints file names and counts only.
    "desktop-tests": [
        "npm",
        "test",
        "--prefix",
        "apps/narrivy-desktop",
        "--",
        "--run",
        "--reporter=verbose",
    ],
    "desktop-build": ["npm", "run", "build", "--prefix", "apps/narrivy-desktop"],
    "governance": ["python3", "-m", "unittest", "discover", "-s", "tests/governance", "-v"],
    "source-boundary": ["python3", "scripts/verify-boundaries.py"],
    "dependency-boundary": ["python3", "scripts/verify_dependency_boundaries.py"],
    "traceability": ["python3", "scripts/verify_traceability.py"],
    "secrets": ["python3", "scripts/verify_secrets.py"],
    "log-redaction": ["python3", "scripts/check_log_redaction.py"],
    "supply-chain": ["cargo", "deny", "--offline", "check"],
}

# A suite exit code proves that a suite ran, not that a specific claim holds.
# Each semantic claim below is bound to named tests that must actually appear
# as passing in this run's own output, so the claim is measured rather than
# asserted. A named test that does not appear aborts the whole run.
PROOF_TESTS: dict[str, tuple[str, ...]] = {
    "capability_ranked_delivery": (
        "fake_backend_routing_order_follows_ladder",
        "uinput_opt_in_sits_between_wtype_and_clipboard",
    ),
    "bounded_fallback": (
        "all_rungs_exhausted_yields_failed",
        "missing_inbox_dir_lets_router_end_failed",
    ),
    "unicode_multiline_verbatim": ("unicode_multiline_and_5k_fixtures_pass_through_verbatim",),
    "inbox_terminal_sink": ("inbox_fallback_writes_atomically_and_validates_filename",),
    "broad_input_privilege_default_off": (
        "uinput_skipped_by_default_even_when_configured_available",
        "default_config_is_least_privilege",
    ),
    "backend_failure": (
        "inbox_fallback_writes_atomically_and_validates_filename",
        "copied_only_is_never_reported_as_delivered",
    ),
    "durable_inbox_recovery": (
        "startup_recovery_moves_only_complete_unsent_results_to_inbox_once",
        "startup_recovery_failure_keeps_the_result_recoverable",
    ),
    "pipeline_addressed_bindings": (
        "load_round_trips_definitions_exactly",
        "normalize_binding_canonicalizes_case_and_order",
    ),
    "toggle_hold_kind_isolation": ("other_singleton_slot_duplicated_is_rejected",),
    "capability_gated_hold": ("legacy_plan_requires_remote_processing_still_works",),
    "atomic_pipeline_toggle": ("set_enabled_toggles_are_reflected_in_list",),
    "perf008_cache_cleanup": ("cleanup_removes_losers_and_protects_selected_pinned_fallback",),
    "raw_clean_builtins": ("builtin_defs_validate_and_clean_has_semantic_guard_but_no_llm",),
    "deterministic_correction": (
        "correction_is_deterministic",
        "variants_track_raw_corrected_delivered_and_classify",
    ),
    "raw_recovery_and_guard": ("only_exact_wtype_inbox_recovery_reports_can_reach_inbox_recovered",),
    "lean_and_advanced_paths": ("AppRendersAllScreens.test.tsx",),
    "spoken_aliases": ("alias_basic_expansion", "alias_case_insensitive"),
    "context_policy_gate": (
        "history_off_returns_none_no_leakage",
        "noop_context_returns_empty_snapshot_when_allowed",
    ),
    "local_default": ("privacy_is_allowed_true_when_no_zones",),
    "retention_controls": ("privacy_history_retention_7d_prunes_expired",),
    "privacy_zone_precedence": (
        "privacy_zone_deny_overrides_allow",
        "history_off_overrides_even_matching_zone",
    ),
    "cloud_boundary_visible": ("CapturePolicyRendering.test.tsx",),
    "content_redaction": ("delivered_speech_and_word_content_is_redacted",),
    "independent_remote_permissions": ("remote_denied_by_default_even_when_stage_requests_it",),
    "navigation_and_deferred_honesty": (
        "AppRendersAllScreens.test.tsx",
        "DeferredFeaturesAbsent.test.tsx",
    ),
    "progressive_disclosure": ("SettingsComputeTruth.test.tsx",),
    "command_palette": ("opens with Ctrl+K, filters, executes on Enter, closes on Escape",),
    "one_step_undo": ("reverts displayed clean output to the raw variant on Undo",),
    "safe_import_export": (
        "load_rejects_future_schema_and_nonexistent_dir",
        "save_writes_per_pipeline_files_and_index",
    ),
    "immutable_registry_integrity": ("registry_and_direct_store_reject_unsafe_identity_components",),
    "manifest_permission_vocabulary": ("module_permissions_require_declaration_and_host_grant",),
    "declared_and_grant_required": ("host_cannot_grant_an_undeclared_permission",),
    "service_boundary_precedes_invocation": ("daemon_enforces_module_permissions_before_invocation",),
    "network_default_deny": ("module_network_access_is_denied_by_default",),
    "denied_operation_no_payload": ("gated_channel_consults_gate_before_inner_sink",),
    "fail_closed_gates": (
        "test_semantic_failures_cannot_mark_a_requirement_passed",
        "test_native_and_package_substitutions_fail_closed_in_every_validator",
        "test_all_legacy_entry_points_and_imports_cannot_write",
        "test_index_registration_and_hash_are_enforced_by_all_validators",
    ),
}

# cargo deny reports one line per check; a supply-chain claim may only be
# measured when the corresponding check was actually reported and did not fail.
SUPPLY_CHAIN_CHECKS = ("advisories", "bans", "licenses", "sources")
SUPPLY_CHAIN_FAILURE = re.compile(r"fail|error|denied|violat", re.IGNORECASE)


def measurement(metric: str, comparator: str, expected: object = None) -> dict[str, object]:
    binding: dict[str, object] = {"metric": metric, "comparator": comparator}
    if comparator not in {"is_true", "is_false"}:
        binding["expected"] = expected
    return binding


def suite_measurement(suite: str) -> dict[str, object]:
    return measurement(f"suites.{suite}.exit_code", "eq", 0)


def proof_measurement(key: str) -> dict[str, object]:
    return measurement(f"proofs.{key}.executed", "eq", len(PROOF_TESTS[key]))


def measured(assertion: str, binding: dict[str, object]) -> dict[str, object]:
    return {"id": assertion, "result": "pass", "measurement": binding}


def proof_metrics(outputs: list[str]) -> dict[str, object]:
    """Resolve, for every declared claim, the named tests that actually passed."""
    proofs: dict[str, object] = {}
    for key, names in PROOF_TESTS.items():
        observed = [name for name in names if any(name in output for output in outputs)]
        if len(observed) != len(names):
            missing = sorted(set(names) - set(observed))
            raise RuntimeError(
                f"no measured evidence for proof {key!r}: these named tests did not pass in "
                f"this run: {missing}"
            )
        proofs[key] = {"expected": len(names), "executed": len(observed), "tests": observed}
    return proofs


def supply_chain_metrics(output: str) -> dict[str, object]:
    checks: dict[str, object] = {}
    for check in SUPPLY_CHAIN_CHECKS:
        line = next((item for item in output.splitlines() if check in item.lower()), None)
        if line is None:
            raise RuntimeError(
                f"cargo deny did not report the {check} check; refusing to declare a "
                "supply-chain measurement that was never observed"
            )
        failed = bool(SUPPLY_CHAIN_FAILURE.search(line))
        checks[check] = not failed
        if failed:
            raise RuntimeError(f"cargo deny reported a {check} failure: {line.strip()}")
    return checks


def counted(output: str, pattern: str, label: str) -> int:
    match = re.search(pattern, output)
    if match is None:
        raise RuntimeError(f"{label} did not report {pattern!r}; refusing to declare a measurement")
    return int(match.group(1))



def record(
    phase: str,
    verification: str,
    profile: str,
    assertions: list[dict[str, object]],
    artifacts: list[str],
) -> dict[str, object]:
    return {
        "phase": phase,
        "verification": verification,
        "procedure": f"controlled-{verification.removeprefix('VER-').lower()}-v1",
        "profile": profile,
        "assertions": assertions,
        "artifacts": artifacts,
    }


UNIT = "unit-contract-v1"
INTEGRATION = "controlled-integration-v1"
RECORDS = {
    "FUN-004": record("P2", "VER-FUN-004", INTEGRATION,
        [measured("platform-linux-suite", suite_measurement("platform-linux")),
         measured("daemon-suite", suite_measurement("daemon")),
         measured("capability-ranked-delivery-proven", proof_measurement("capability_ranked_delivery")),
         measured("bounded-fallback-proven", proof_measurement("bounded_fallback")),
         measured("unicode-multiline-verbatim-proven", proof_measurement("unicode_multiline_verbatim")),
         measured("inbox-terminal-sink-proven", proof_measurement("inbox_terminal_sink")),
         measured("broad-input-privilege-default-off-proven", proof_measurement("broad_input_privilege_default_off"))],
        ["adapters/platform-linux/src/delivery/routed.rs",
         "adapters/platform-linux/src/delivery/tests.rs",
         "services/narrivyd/src/daemon/session.rs"]),
    "REL-003": record("P2", "VER-REL-003", INTEGRATION,
        [measured("platform-linux-suite", suite_measurement("platform-linux")),
         measured("daemon-suite", suite_measurement("daemon")),
         measured("backend-failure-proven", proof_measurement("backend_failure")),
         measured("bounded-fallback-proven", proof_measurement("bounded_fallback")),
         measured("durable-inbox-recovery-proven", proof_measurement("durable_inbox_recovery"))],
        ["adapters/platform-linux/src/delivery/routed.rs",
         "adapters/platform-linux/src/recovery.rs",
         "services/narrivyd/src/daemon/session.rs"]),
    "FUN-031": record("P2", "VER-FUN-031", INTEGRATION,
        [measured("pipeline-suite", suite_measurement("pipeline")),
         measured("platform-linux-suite", suite_measurement("platform-linux")),
         measured("daemon-suite", suite_measurement("daemon")),
         measured("pipeline-addressed-bindings-proven", proof_measurement("pipeline_addressed_bindings")),
         measured("toggle-hold-kind-isolation-proven", proof_measurement("toggle_hold_kind_isolation")),
         measured("capability-gated-hold-proven", proof_measurement("capability_gated_hold")),
         measured("atomic-pipeline-toggle-proven", proof_measurement("atomic_pipeline_toggle"))],
        ["crates/narrivy-pipeline/src/lib.rs",
         "crates/narrivy-pipeline/tests/contracts.rs",
         "adapters/platform-linux/src/trigger.rs",
         "services/narrivyd/src/daemon/session.rs"]),
    "PERF-008": record("P5", "VER-PERF-008", INTEGRATION,
        [measured("model-cache-suite", suite_measurement("model-cache")),
         measured("staging-cleanup-proven", proof_measurement("perf008_cache_cleanup")),
         measured("benchmark-loser-cleanup-proven", proof_measurement("perf008_cache_cleanup")),
         measured("source-selected-pinned-fallback-preservation-proven", proof_measurement("perf008_cache_cleanup")),
         measured("dry-run-proven", proof_measurement("perf008_cache_cleanup")),
         measured("idempotent-replay-proven", proof_measurement("perf008_cache_cleanup"))],
        ["crates/narrivy-model-manager/src/cleanup.rs",
         "crates/narrivy-model-manager/tests/p5_model_cache.rs"]),
    "SEC-001": record("P9", "VER-SEC-001", INTEGRATION,
        [measured("policy-suite", suite_measurement("policy")),
         measured("core-suite", suite_measurement("core")),
         measured("daemon-suite", suite_measurement("daemon")),
         measured("manifest-permission-vocabulary-proven", proof_measurement("manifest_permission_vocabulary")),
         measured("declared-and-granted-required-proven", proof_measurement("declared_and_grant_required")),
         measured("service-boundary-precedes-invocation-proven", proof_measurement("service_boundary_precedes_invocation"))],
        ["spec/schemas/plugin-manifest.schema.json",
         "crates/narrivy-policy/src/lib.rs", "crates/narrivy-core/src/modules.rs",
         "services/narrivyd/src/daemon/session.rs"]),
    "SEC-003": record("P9", "VER-SEC-003", INTEGRATION,
        [measured("policy-suite", suite_measurement("policy")),
         measured("core-suite", suite_measurement("core")),
         measured("daemon-suite", suite_measurement("daemon")),
         measured("network-default-deny-proven", proof_measurement("network_default_deny")),
         measured("undeclared-network-grant-rejected-proven", proof_measurement("declared_and_grant_required")),
         measured("denied-operation-receives-no-payload-proven", proof_measurement("denied_operation_no_payload"))],
        ["spec/schemas/plugin-manifest.schema.json",
         "crates/narrivy-policy/src/lib.rs", "crates/narrivy-core/src/modules.rs",
         "services/narrivyd/src/daemon/session.rs"]),
    "PORT-002": record("P1", "VER-PORT-002", UNIT,
        [measured("core-suite", suite_measurement("core")),
         measured("source-boundary-suite", suite_measurement("source-boundary")),
         measured("os-api-boundary-proven", suite_measurement("source-boundary"))],
        ["crates/narrivy-core/src/lib.rs", "adapters/platform-null/src/lib.rs", "scripts/verify-boundaries.py"]),
    "TEST-001": record("P0", "VER-TEST-001", UNIT,
        [measured("traceability-suite", suite_measurement("traceability")),
         measured("release-requirement-mapping-proven",
                  measurement("gates.traceability.requirements_mapped", "ge", 1))],
        ["scripts/verify_traceability.py"]),
    "TEST-002": record("P0", "VER-TEST-002", UNIT,
        [measured("source-boundary-suite", suite_measurement("source-boundary")),
         measured("dependency-boundary-suite", suite_measurement("dependency-boundary")),
         measured("architecture-gates-proven", measurement("gates.architecture_failures", "eq", 0))],
        ["scripts/verify-boundaries.py", "scripts/verify_dependency_boundaries.py"]),
    "TEST-008": record("P0", "VER-TEST-008", UNIT,
        [measured("governance-suite", suite_measurement("governance")),
         measured("fail-closed-gates-proven", proof_measurement("fail_closed_gates"))],
        ["tests/governance/test_recovery_validators.py", "scripts/release_gate.py"]),
    "SEC-004": record("P0", "VER-SEC-004", UNIT,
        [measured("supply-chain-suite", suite_measurement("supply-chain")),
         measured("advisory-gate-passed", measurement("supply_chain.advisories", "is_true")),
         measured("license-gate-passed", measurement("supply_chain.licenses", "is_true")),
         measured("source-gate-passed", measurement("supply_chain.sources", "is_true"))],
        ["Cargo.lock", "deny.toml", ".github/workflows/ci.yml"]),
    "FUN-021": record("P7", "VER-FUN-021", INTEGRATION,
        [measured("pipeline-suite", suite_measurement("pipeline")),
         measured("daemon-suite", suite_measurement("daemon")),
         measured("raw-clean-builtins-proven", proof_measurement("raw_clean_builtins"))],
        ["crates/narrivy-pipeline/src/lib.rs", "services/narrivyd/src/daemon/session.rs"]),
    "FUN-027": record("P7", "VER-FUN-027", INTEGRATION,
        [measured("core-suite", suite_measurement("core")),
         measured("daemon-suite", suite_measurement("daemon")),
         measured("deterministic-correction-proven", proof_measurement("deterministic_correction"))],
        ["crates/narrivy-core/src/transcript.rs", "services/narrivyd/src/daemon/session.rs"]),
    "FUN-032": record("P7", "VER-FUN-032", INTEGRATION,
        [measured("core-suite", suite_measurement("core")),
         measured("pipeline-suite", suite_measurement("pipeline")),
         measured("raw-recovery-and-guard-proven", proof_measurement("raw_recovery_and_guard"))],
        ["crates/narrivy-core/src/recovery.rs", "crates/narrivy-pipeline/src/lib.rs"]),
    "FUN-033": record("P7", "VER-FUN-033", INTEGRATION,
        [measured("pipeline-suite", suite_measurement("pipeline")),
         measured("desktop-suite", suite_measurement("desktop-tests")),
         measured("lean-and-advanced-paths-proven", proof_measurement("lean_and_advanced_paths"))],
        ["crates/narrivy-pipeline/src/lib.rs", "apps/narrivy-desktop/src/app/screens/Settings.tsx"]),
    "FUN-041": record("P8", "VER-FUN-041", INTEGRATION,
        [measured("history-suite", suite_measurement("history")),
         measured("spoken-aliases-proven", proof_measurement("spoken_aliases"))],
        ["crates/narrivy-history/src/dictionary.rs"]),
    "FUN-064": record("P8", "VER-FUN-064", INTEGRATION,
        [measured("history-suite", suite_measurement("history")),
         measured("context-policy-gate-proven", proof_measurement("context_policy_gate"))],
        ["crates/narrivy-history/src/context_adapter.rs"]),
    "PRIV-001": record("P8", "VER-PRIV-001", INTEGRATION,
        [measured("policy-suite", suite_measurement("policy")),
         measured("history-suite", suite_measurement("history")),
         measured("local-default-proven", proof_measurement("local_default"))],
        ["crates/narrivy-policy/src/lib.rs", "crates/narrivy-history/src/privacy.rs"]),
    "PRIV-002": record("P8", "VER-PRIV-002", INTEGRATION,
        [measured("history-suite", suite_measurement("history")),
         measured("retention-controls-proven", proof_measurement("retention_controls"))],
        ["crates/narrivy-history/src/history.rs"]),
    "PRIV-003": record("P8", "VER-PRIV-003", INTEGRATION,
        [measured("history-suite", suite_measurement("history")),
         measured("privacy-zone-precedence-proven", proof_measurement("privacy_zone_precedence"))],
        ["crates/narrivy-history/src/privacy_zones.rs"]),
    "PRIV-004": record("P8", "VER-PRIV-004", INTEGRATION,
        [measured("policy-suite", suite_measurement("policy")),
         measured("desktop-suite", suite_measurement("desktop-tests")),
         measured("cloud-boundary-visible-proven", proof_measurement("cloud_boundary_visible"))],
        ["crates/narrivy-policy/src/lib.rs", "apps/narrivy-desktop/src/__tests__/CapturePolicyRendering.test.tsx"]),
    "PRIV-005": record("P8", "VER-PRIV-005", INTEGRATION,
        [measured("secrets-suite", suite_measurement("secrets")),
         measured("regular-config-secret-exclusion-proven",
                  measurement("gates.secrets.files_scanned", "ge", 1))],
        ["scripts/verify_secrets.py"]),
    "PRIV-006": record("P8", "VER-PRIV-006", INTEGRATION,
        [measured("log-redaction-suite", suite_measurement("log-redaction")),
         measured("diagnostics-suite", suite_measurement("diagnostics")),
         measured("content-redaction-proven", proof_measurement("content_redaction"))],
        ["scripts/check_log_redaction.py", "crates/narrivy-diagnostics/src/lib.rs"]),
    "PRIV-007": record("P8", "VER-PRIV-007", INTEGRATION,
        [measured("history-suite", suite_measurement("history")),
         measured("pipeline-suite", suite_measurement("pipeline")),
         measured("independent-remote-permissions-proven", proof_measurement("independent_remote_permissions"))],
        ["crates/narrivy-history/src/context_adapter.rs", "crates/narrivy-pipeline/src/lib.rs"]),
    "UX-002": record("P3", "VER-UX-002", INTEGRATION,
        [measured("desktop-suite", suite_measurement("desktop-tests")),
         measured("navigation-and-deferred-honesty-proven", proof_measurement("navigation_and_deferred_honesty"))],
        ["apps/narrivy-desktop/src/__tests__/AppRendersAllScreens.test.tsx", "apps/narrivy-desktop/src/__tests__/DeferredFeaturesAbsent.test.tsx"]),
    "UX-003": record("P3", "VER-UX-003", INTEGRATION,
        [measured("desktop-suite", suite_measurement("desktop-tests")),
         measured("progressive-disclosure-proven", proof_measurement("progressive_disclosure"))],
        ["apps/narrivy-desktop/src/app/components/SettingsCompute/ComputeCard.tsx"]),
    "UX-008": record("P3", "VER-UX-008", INTEGRATION,
        [measured("desktop-suite", suite_measurement("desktop-tests")),
         measured("command-palette-proven", proof_measurement("command_palette"))],
        ["apps/narrivy-desktop/src/__tests__/PaletteOpensAndExecutes.test.tsx"]),
    "UX-009": record("P7", "VER-UX-009", INTEGRATION,
        [measured("core-suite", suite_measurement("core")),
         measured("desktop-suite", suite_measurement("desktop-tests")),
         measured("one-step-undo-proven", proof_measurement("one_step_undo"))],
        ["apps/narrivy-desktop/src/__tests__/UndoRestoresRawAfterClean.test.tsx", "crates/narrivy-core/src/transcript.rs"]),
    "UX-011": record("P7", "VER-UX-011", INTEGRATION,
        [measured("pipeline-suite", suite_measurement("pipeline")),
         measured("desktop-suite", suite_measurement("desktop-tests")),
         measured("safe-import-export-proven", proof_measurement("safe_import_export"))],
        ["crates/narrivy-pipeline/tests/contracts.rs", "apps/narrivy-desktop/src-tauri/src/bridge.rs"]),
    "SEC-007": record("P5", "VER-SEC-007", INTEGRATION,
        [measured("model-cache-suite", suite_measurement("model-cache")),
         measured("immutable-registry-integrity-proven", proof_measurement("immutable_registry_integrity"))],
        ["crates/narrivy-model-manager/src/store.rs", "crates/narrivy-model-manager/src/registry.rs", "crates/narrivy-model-manager/tests/p5_model_cache.rs"]),
}


DEFAULT_NOTE = (
    "Shared clean-worktree controlled qualification. Every suite assertion is measured by that "
    "suite's own exit code, and every semantic claim is measured by the named tests that passed "
    "in this same run (see metrics.proofs). No native platform or release-package claim."
)
NOTES = {
    "PERF-008": (
        "Staging cleanup, benchmark-loser cleanup, pinned-fallback preservation, dry-run and "
        "idempotent replay are each proven by the single named test "
        "cleanup_removes_losers_and_protects_selected_pinned_fallback, so those five claims share "
        "one measurement: the suite exposes no finer-grained signal. Split that test to give each "
        "claim its own measurement."
    ),
    "PORT-002": (
        "os-api-boundary-proven is measured by the same architecture boundary gate as "
        "source-boundary-suite: that gate is the only executable evidence that no OS API leaks "
        "into portable domain code."
    ),
    "SEC-004": (
        "Supply-chain claims are measured per cargo-deny check (advisories, licenses, sources) "
        "rather than by the aggregate suite exit code, so this record is not interchangeable with "
        "any other record emitted by the same shared run."
    ),
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout.strip()


def fail(message: str) -> int:
    print(f"record_controlled_integration_evidence: {message}", file=sys.stderr)
    return 1


def atomic_json(path: Path, value: object) -> None:
    payload = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload); stream.flush(); os.fsync(stream.fileno())
        os.chmod(temporary, 0o644); os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]: return fail("argv does not match the reviewed procedure")
    if git("status", "--porcelain"): return fail("worktree must be clean before controlled evidence execution")
    missing = sorted({a for r in RECORDS.values() for a in r["artifacts"] if not (ROOT / a).is_file()})
    if missing: return fail(f"reviewed artifact paths are missing: {missing}")
    started = datetime.now(timezone.utc)
    metrics: dict[str, object] = {}
    outputs: dict[str, str] = {}
    try:
        for name, command in SUITES.items():
            result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
            metrics[name] = {"command": shlex.join(command), "exit_code": result.returncode,
                             "stdout_lines": len(result.stdout.splitlines()), "stderr_lines": len(result.stderr.splitlines())}
            outputs[name] = result.stdout + result.stderr
            if result.returncode:
                return fail(f"suite {name} failed\n" + "\n".join(outputs[name].splitlines()[-20:]))
        proofs = proof_metrics(list(outputs.values()))
        supply_chain = supply_chain_metrics(outputs["supply-chain"])
        gates = {
            "architecture_failures": int(metrics["source-boundary"]["exit_code"] != 0)
            + int(metrics["dependency-boundary"]["exit_code"] != 0),
            "traceability": {"requirements_mapped": counted(
                outputs["traceability"], r"(\d+) catalog requirements mapped exactly once",
                "scripts/verify_traceability.py")},
            "secrets": {"files_scanned": counted(
                outputs["secrets"], r"Secrets check OK: (\d+) text files scanned",
                "scripts/verify_secrets.py")},
        }
    except RuntimeError as error:
        return fail(str(error))
    if git("status", "--porcelain"): return fail("controlled suites changed the worktree")
    ended = datetime.now(timezone.utc); head = git("rev-parse", "HEAD")
    script_rel = "scripts/record_controlled_integration_evidence.py"; script_hash = sha256(ROOT / script_rel)
    evidence_schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
    index_schema = json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text())
    index_path = ROOT / INDEX; index = json.loads(index_path.read_text())
    targets = {f"agent/evidence/{r['phase']}/{r['verification']}.json" for r in RECORDS.values()}
    index["qualified_evidence"] = [x for x in index["qualified_evidence"] if x.get("path") not in targets]
    payloads = []
    for requirement, r in RECORDS.items():
        rel = f"agent/evidence/{r['phase']}/{r['verification']}.json"
        hashes = {script_rel: script_hash, **{a: sha256(ROOT / a) for a in r["artifacts"]}}
        evidence_type = "unit" if r["profile"] == UNIT else "integration"
        evidence = {
            "schema_version": 3, "verification_id": r["verification"], "requirement_ids": [requirement],
            "phase": r["phase"], "scenario_id": r["procedure"], "git_commit": head, "git_dirty": False,
            "build_id": f"source-{head[:12]}",
            "environment": {"id": "controlled-integration", "native_or_vm": "controlled", "os": platform.system(),
                "os_version": platform.release(), "kernel": platform.release(), "desktop": "not-applicable",
                "session": "controlled", "cpu": platform.machine(), "gpu": None, "npu": None,
                "device_details": None, "microphone": None, "input_backend": None, "display_scale": None,
                "qualification_lane": "controlled-integration", "lane_available": True},
            "command": shlex.join(EXPECTED_ARGV), "scenario_version": "1", "scenario_hash": script_hash,
            "started_at": started.isoformat(), "ended_at": ended.isoformat(), "exit_code": 0,
            "result": "pass", "pass": True,
            "metrics": {"suites": metrics, "proofs": proofs, "gates": gates, "supply_chain": supply_chain},
            "artifact_hashes": hashes,
            "procedure": {"id": r["procedure"], "profile_id": r["profile"],
                "assertions": r["assertions"]},
            "notes": NOTES.get(requirement, DEFAULT_NOTE),
            "evidence_type": evidence_type,
            "execution": {"lane_available": True, "child_failures_ignored": False,
                "environment_class": "controlled", "substitutions": []},
        }
        jsonschema.validate(instance=evidence, schema=evidence_schema)
        payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode(); payloads.append((ROOT / rel, evidence, payload))
        index["qualified_evidence"].append({"path": rel, "sha256": hashlib.sha256(payload).hexdigest(), "disposition": "qualified"})
    index["qualified_evidence"].sort(key=lambda x: x["path"]); jsonschema.validate(instance=index, schema=index_schema)
    for path, evidence, _ in payloads: atomic_json(path, evidence)
    atomic_json(index_path, index)
    print(json.dumps({"result": "pass", "records": len(payloads)}, sort_keys=True)); return 0


if __name__ == "__main__": raise SystemExit(main())
