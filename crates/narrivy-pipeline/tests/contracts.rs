//! Integration tests for the first-class Pipeline contracts (ADR-0016/0017).
//!
//! Covers: definition validation matrix, `normalize_binding`, registry
//! versioning + enable toggles, cross-pipeline binding conflicts, on-disk
//! persistence (round-trip, corrupt-file skip, legacy migration), resource
//! identity/location separation, the mechanical slot I/O table, and serde wire
//! stability.

// Test code legitimately uses unwrap/expect/panic for assertions and failure
// reporting; the workspace production lints (unwrap_used, expect_used, panic)
// are relaxed for this integration-test target only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use narrivy_pipeline::*;
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

fn module_ref(name: &str) -> ResourceRef {
    ResourceRef::narrivy_module(name, ">=1.0.0")
}

fn node(id: &str, slot: CapabilitySlot) -> PipelineNode {
    PipelineNode {
        id: id.to_string(),
        slot,
        module: module_ref("some-module"),
        enabled: true,
        optional: false,
        device_preference: Vec::new(),
    }
}

/// Minimal legal pipeline: a single speech.asr node, manual-only.
fn minimal_def(id: &str) -> PipelineDefinition {
    PipelineDefinition::new(
        id,
        format!("Pipeline {id}"),
        vec![node("asr", CapabilitySlot::SpeechAsr)],
    )
}

fn hotkey_trigger(binding: &str, enabled: bool) -> TriggerBinding {
    TriggerBinding {
        kind: TriggerKind::HoldHotkey,
        binding: binding.to_string(),
        enabled,
    }
}

fn expect_err(def: &PipelineDefinition, needle: &str) -> PipelineError {
    let err = def.validate().expect_err("expected validate() to fail");
    let text = err.to_string();
    assert!(
        text.contains(needle),
        "error {text:?} did not mention {needle:?} (variant: {err:?})"
    );
    err
}

// ---------------------------------------------------------------------------
// 1. Validation matrix
// ---------------------------------------------------------------------------

#[test]
fn minimal_legal_pipeline_validates() {
    minimal_def("minimal")
        .validate()
        .expect("single asr node, manual-only");
}

#[test]
fn full_legal_dictation_chain_validates() {
    let mut def = minimal_def("dictation");
    def.manual_only = false;
    def.triggers = vec![TriggerBinding::hotkey(TriggerKind::HoldHotkey, "Ctrl+Space").unwrap()];
    def.nodes = vec![
        node("capture", CapabilitySlot::AudioCapture),
        node("denoise", CapabilitySlot::AudioDenoise),
        node("vad", CapabilitySlot::SpeechVad),
        node("endpoint", CapabilitySlot::SpeechEndpoint),
        node("asr", CapabilitySlot::SpeechAsr),
        node("punct", CapabilitySlot::TextPunctuation),
        node("itn", CapabilitySlot::TextItn),
        node("deliver", CapabilitySlot::DeliveryDirect),
    ];
    def.edges = vec![
        edge("capture", "denoise"),
        edge("denoise", "vad"),
        edge("vad", "endpoint"),
        edge("endpoint", "asr"),
        edge("asr", "punct"),
        edge("punct", "itn"),
        // itn -> deliver would be a mismatch (Transcript out, Text in), so the
        // delivery leg starts from the semantic-guard-style Text slots only.
    ];
    def.validate()
        .expect("audio->speech->transcript chain is kind-correct");
}

fn edge(from: &str, to: &str) -> PipelineEdge {
    PipelineEdge {
        from_node: from.to_string(),
        to_node: to.to_string(),
    }
}

#[test]
fn cycle_is_rejected() {
    let mut def = minimal_def("cyclic");
    def.nodes.push(node("cap", CapabilitySlot::AudioCapture));
    def.nodes.push(node("norm", CapabilitySlot::AudioNormalize));
    // audio->audio both ways: kinds are fine, topology is not.
    def.edges = vec![edge("cap", "norm"), edge("norm", "cap")];
    assert!(matches!(
        def.validate(),
        Err(PipelineError::CycleDetected { .. })
    ));
}

#[test]
fn self_loop_edge_is_rejected_as_cycle() {
    let mut def = minimal_def("selfloop");
    def.nodes
        .insert(0, node("cap", CapabilitySlot::AudioCapture));
    def.edges = vec![edge("cap", "cap")];
    assert!(matches!(
        def.validate(),
        Err(PipelineError::CycleDetected { .. })
    ));
}

#[test]
fn unknown_edge_endpoint_is_rejected() {
    let mut def = minimal_def("dangling-edge");
    def.edges = vec![edge("asr", "ghost")];
    assert!(matches!(
        def.validate(),
        Err(PipelineError::UnknownNode { ref node }) if node == "ghost"
    ));

    let mut def2 = minimal_def("dangling-edge-from");
    def2.edges = vec![edge("phantom", "asr")];
    assert!(matches!(
        def2.validate(),
        Err(PipelineError::UnknownNode { ref node }) if node == "phantom"
    ));
}

#[test]
fn audio_to_transcript_stream_mismatch_is_rejected() {
    let mut def = minimal_def("bad-audio-to-text");
    def.nodes
        .insert(0, node("cap", CapabilitySlot::AudioCapture));
    def.nodes
        .push(node("punct", CapabilitySlot::TextPunctuation));
    // audio.capture outputs Audio; text.punctuation expects Transcript.
    def.edges = vec![edge("cap", "punct")];
    assert!(matches!(
        def.validate(),
        Err(PipelineError::StreamMismatch { ref from, ref to, ref out, ref inp })
            if from == "cap" && to == "punct" && out == "Audio" && inp == "Transcript"
    ));
}

#[test]
fn text_to_audio_stream_mismatch_is_rejected() {
    let mut def = minimal_def("bad-text-to-audio");
    def.nodes
        .insert(0, node("deliver", CapabilitySlot::DeliveryDirect));
    def.nodes.push(node("vad", CapabilitySlot::SpeechVad));
    // delivery.direct outputs Text; speech.vad expects Audio.
    def.edges = vec![edge("deliver", "vad")];
    assert!(matches!(
        def.validate(),
        Err(PipelineError::StreamMismatch { ref from, ref to, ref out, ref inp })
            if from == "deliver" && to == "vad" && out == "Text" && inp == "Audio"
    ));
}

#[test]
fn two_asr_nodes_are_rejected() {
    let mut def = minimal_def("double-asr");
    def.nodes.push(node("asr-2", CapabilitySlot::SpeechAsr));
    // The exactly-one-asr rule fires before the general singleton scan.
    expect_err(&def, "exactly one speech.asr");
}

#[test]
fn zero_asr_nodes_are_rejected() {
    let def = PipelineDefinition::new(
        "no-asr",
        "No ASR",
        vec![node("cap", CapabilitySlot::AudioCapture)],
    );
    expect_err(&def, "found 0");
}

#[test]
fn other_singleton_slot_duplicated_is_rejected() {
    let mut def = minimal_def("double-capture");
    def.nodes
        .insert(0, node("cap-a", CapabilitySlot::AudioCapture));
    def.nodes
        .insert(1, node("cap-b", CapabilitySlot::AudioCapture));
    assert!(matches!(
        def.validate(),
        Err(PipelineError::SingletonViolation { ref slot, count }) if slot == "audio.capture" && count == 2
    ));
}

#[test]
fn duplicate_node_ids_are_rejected() {
    let mut def = minimal_def("dup-node-id");
    def.nodes.push(node("asr", CapabilitySlot::SpeechAsr));
    assert!(matches!(
        def.validate(),
        Err(PipelineError::DuplicateNodeId(ref id)) if id == "asr"
    ));
}

#[test]
fn empty_triggers_without_manual_only_is_rejected() {
    let mut def = minimal_def("needs-trigger");
    def.manual_only = false;
    assert!(matches!(
        def.validate(),
        Err(PipelineError::NoEnabledTrigger)
    ));
}

#[test]
fn empty_triggers_with_manual_only_is_accepted() {
    let def = minimal_def("manual-ok");
    assert!(def.manual_only);
    def.validate()
        .expect("manual-only pipelines need no trigger");
}

#[test]
fn all_disabled_triggers_without_manual_only_is_rejected() {
    let mut def = minimal_def("disabled-trigger-only");
    def.manual_only = false;
    def.triggers = vec![hotkey_trigger("ctrl+space", false)];
    assert!(matches!(
        def.validate(),
        Err(PipelineError::NoEnabledTrigger)
    ));
}

#[test]
fn un_normalized_enabled_binding_is_rejected() {
    // Constructed directly so the stored form is non-canonical.
    let mut def = minimal_def("non-canonical-binding");
    def.triggers = vec![hotkey_trigger("Ctrl+Space", true)];
    assert!(matches!(
        def.validate(),
        Err(PipelineError::InvalidBinding { ref raw, .. }) if raw == "Ctrl+Space"
    ));
}

#[test]
fn duplicate_enabled_bindings_within_one_pipeline_are_rejected() {
    let mut def = minimal_def("dup-binding");
    def.triggers = vec![
        hotkey_trigger("ctrl+space", true),
        hotkey_trigger("ctrl+space", true),
    ];
    assert!(matches!(
        def.validate(),
        Err(PipelineError::DuplicateBinding { ref binding }) if binding == "ctrl+space"
    ));
}

#[test]
fn same_binding_disabled_and_enabled_coexists() {
    // Disabled duplicates are exempt from the uniqueness rule.
    let mut def = minimal_def("dup-disabled-binding");
    def.triggers = vec![
        hotkey_trigger("ctrl+space", true),
        hotkey_trigger("ctrl+space", false),
    ];
    def.validate()
        .expect("a disabled duplicate must not conflict with the enabled one");
}

#[test]
fn structural_header_fields_are_validated() {
    let def = minimal_def("Bad_Slug_Upper");
    assert!(matches!(def.validate(), Err(PipelineError::InvalidId(_))));

    let def = minimal_def("");
    assert!(matches!(def.validate(), Err(PipelineError::InvalidId(_))));

    let mut def = minimal_def("version-zero");
    def.version = 0;
    assert!(matches!(
        def.validate(),
        Err(PipelineError::InvalidVersion(0))
    ));

    let mut def = minimal_def("empty-name");
    def.name = "   ".to_string();
    assert!(matches!(
        def.validate(),
        Err(PipelineError::EmptyField { .. })
    ));

    let mut def = minimal_def("no-nodes");
    def.nodes.clear();
    assert!(matches!(
        def.validate(),
        Err(PipelineError::EmptyField { .. })
    ));

    let mut def = minimal_def("future-schema");
    def.schema_version = PIPELINE_SCHEMA_VERSION + 1;
    assert!(matches!(
        def.validate(),
        Err(PipelineError::UnsupportedSchemaVersion { found: _, supported }) if supported == PIPELINE_SCHEMA_VERSION
    ));
}

#[test]
fn invalid_resource_refs_on_nodes_are_rejected() {
    let bad_ns = ResourceRef {
        namespace: "elsewhere".to_string(),
        kind: ResourceKind::Module,
        name: "m".to_string(),
        version_req: ">=1.0.0".to_string(),
    };
    let mut def = minimal_def("bad-namespace");
    def.nodes[0].module = bad_ns;
    assert!(matches!(
        def.validate(),
        Err(PipelineError::InvalidResourceRef(_))
    ));

    let mut def = minimal_def("empty-module-name");
    def.nodes[0].module.name = "  ".to_string();
    assert!(matches!(
        def.validate(),
        Err(PipelineError::EmptyField { .. })
    ));
}

// ---------------------------------------------------------------------------
// 2. normalize_binding grammar
// ---------------------------------------------------------------------------

#[test]
fn normalize_binding_canonicalizes_case_and_order() {
    assert_eq!(normalize_binding("Ctrl+Space").unwrap(), "ctrl+space");
    assert_eq!(
        normalize_binding("Shift+CTRL+Space").unwrap(),
        "ctrl+shift+space"
    );
    assert_eq!(normalize_binding("super+a").unwrap(), "super+a");
    assert_eq!(normalize_binding("SUPER+A").unwrap(), "super+a");
    assert_eq!(normalize_binding("Alt+Shift+K").unwrap(), "alt+shift+k");
    assert_eq!(normalize_binding("shift+alt+k").unwrap(), "alt+shift+k");
    // Whitespace around tokens is trimmed; modifiers deduplicated by value.
    assert_eq!(normalize_binding("Ctrl + Space").unwrap(), "ctrl+space");
    assert_eq!(normalize_binding("ctrl+space").unwrap(), "ctrl+space");
}

#[test]
fn normalize_binding_rejects_invalid_grammar() {
    for raw in [
        "",            // empty
        "   ",         // whitespace only
        "+",           // empty segments
        "Ctrl++Space", // empty segment between modifiers
        "ctrl+alt",    // modifier-only, no key
        "Space",       // no modifier is fine actually...
    ] {
        if raw == "Space" {
            continue; // single key with no modifier is grammatically valid
        }
        let err = normalize_binding(raw).expect_err(raw);
        assert!(
            matches!(err, PipelineError::InvalidBinding { .. }),
            "{err:?}"
        );
    }
    assert_eq!(normalize_binding("Space").unwrap(), "space");

    for bad in ["a+b", "ctrl+alt+x+y", "ctrl+shift+alt+super+f5+esc"] {
        let err = normalize_binding(bad).expect_err(bad);
        assert!(
            matches!(err, PipelineError::InvalidBinding { reason, .. } if reason.contains("one non-modifier key"))
        );
    }
}

#[test]
fn normalize_binding_rejects_duplicate_modifier_and_reports_raw() {
    let err = normalize_binding("Ctrl+CTRL+x").expect_err("duplicate modifier");
    match err {
        PipelineError::InvalidBinding { raw, reason } => {
            assert_eq!(raw, "Ctrl+CTRL+x");
            assert!(reason.contains("duplicate"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn trigger_hotkey_constructor_normalizes() {
    let t = TriggerBinding::hotkey(TriggerKind::ToggleHotkey, "SHIFT+ctrl+F9").unwrap();
    assert_eq!(t.binding, "ctrl+shift+f9");
    assert!(t.enabled, "hotkey constructor enables by default");
    assert_eq!(t.canonical().unwrap(), "ctrl+shift+f9");
}

#[test]
fn canonical_for_non_hotkey_kinds_is_trimmed_lowercase() {
    for kind in [
        TriggerKind::AppRule,
        TriggerKind::Cli,
        TriggerKind::WakePhrase,
    ] {
        let t = TriggerBinding {
            kind,
            binding: "  Some.Rule.ID  ".to_string(),
            enabled: true,
        };
        assert_eq!(t.canonical().unwrap(), "some.rule.id");
        let empty = TriggerBinding {
            kind,
            binding: "   ".to_string(),
            enabled: true,
        };
        assert!(matches!(
            empty.canonical(),
            Err(PipelineError::InvalidBinding { .. })
        ));
    }
}

#[test]
fn enabled_binding_keys_lists_only_enabled() {
    let mut def = minimal_def("keys");
    def.triggers = vec![
        hotkey_trigger("ctrl+space", true),
        hotkey_trigger("alt+t", false),
    ];
    assert_eq!(def.enabled_binding_keys().unwrap(), vec!["ctrl+space"]);
}

// ---------------------------------------------------------------------------
// 3. Registry behavior
// ---------------------------------------------------------------------------

#[test]
fn insert_then_get_list_roundtrip() {
    let mut reg = PipelineRegistry::new();
    reg.insert(minimal_def("alpha")).expect("insert alpha");
    reg.insert(minimal_def("beta")).expect("insert beta");

    assert!(reg.get("alpha").is_some());
    assert!(reg.get("unknown").is_none());

    let list = reg.list();
    assert_eq!(list.len(), 2);
    assert!(
        list.iter().all(|s| s.enabled),
        "fresh pipelines are enabled"
    );
    assert!(list.iter().all(|s| s.version == 1));

    // Insert stamps creation/update timestamps.
    let def = reg.get("alpha").unwrap();
    assert!(def.created_unix_ms > 0 && def.updated_unix_ms > 0);
}

#[test]
fn insert_rejects_duplicate_id_and_invalid_definition() {
    let mut reg = PipelineRegistry::new();
    reg.insert(minimal_def("same")).unwrap();

    assert!(matches!(
        reg.insert(minimal_def("same")),
        Err(PipelineError::DuplicatePipelineId(ref id)) if id == "same"
    ));

    // Validation runs before duplicate check: an invalid def is rejected even
    // when its id is fresh.
    let invalid =
        PipelineDefinition::new("Bad_Id", "x", vec![node("asr", CapabilitySlot::SpeechAsr)]);
    assert!(matches!(
        reg.insert(invalid),
        Err(PipelineError::InvalidId(_))
    ));
}

#[test]
fn update_requires_exact_version_bump() {
    let mut reg = PipelineRegistry::new();
    reg.insert(minimal_def("p")).unwrap();

    let bump = |reg: &mut PipelineRegistry, version: u32| {
        let mut def = minimal_def("p");
        def.version = version;
        reg.update(def)
    };

    // Same-version save rejected (version-bump-on-save).
    assert!(matches!(
        bump(&mut reg, 1),
        Err(PipelineError::VersionBumpRejected {
            expected: 2,
            actual: 1
        })
    ));
    // Skip-version save rejected.
    assert!(matches!(
        bump(&mut reg, 3),
        Err(PipelineError::VersionBumpRejected {
            expected: 2,
            actual: 3
        })
    ));
    assert_eq!(
        reg.get("p").unwrap().version,
        1,
        "failed updates keep old version"
    );

    // Exactly current + 1 accepted.
    bump(&mut reg, 2).expect("exact +1 bump");
    assert_eq!(reg.get("p").unwrap().version, 2);
    // Next bump must be 3 now.
    assert!(matches!(
        bump(&mut reg, 2),
        Err(PipelineError::VersionBumpRejected {
            expected: 3,
            actual: 2
        })
    ));

    // Updating an unknown id rejected.
    let mut ghost = minimal_def("ghost");
    ghost.version = 42;
    assert!(matches!(
        reg.update(ghost),
        Err(PipelineError::UnknownPipelineId(ref id)) if id == "ghost"
    ));
}

#[test]
fn update_preserves_created_timestamp_and_refreshes_updated() {
    let mut reg = PipelineRegistry::new();
    reg.insert(minimal_def("ts")).unwrap();
    let created = reg.get("ts").unwrap().created_unix_ms;

    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut def = minimal_def("ts");
    def.version = 2;
    reg.update(def).unwrap();

    let stored = reg.get("ts").unwrap();
    assert_eq!(
        stored.created_unix_ms, created,
        "created timestamp survives update"
    );
    assert!(stored.updated_unix_ms >= created + 5);
}

#[test]
fn set_enabled_toggles_are_reflected_in_list() {
    let mut reg = PipelineRegistry::new();
    reg.insert(minimal_def("on")).unwrap();
    reg.insert(minimal_def("off")).unwrap();

    reg.set_enabled("off", false).unwrap();
    reg.set_enabled("on", false).unwrap();
    reg.set_enabled("on", true).unwrap();

    let list: Vec<(String, bool)> = reg
        .list()
        .into_iter()
        .map(|s| (s.id.clone(), s.enabled))
        .collect();
    assert!(list.contains(&("on".to_string(), true)));
    assert!(list.contains(&("off".to_string(), false)));

    assert!(reg.is_enabled("on"));
    assert!(!reg.is_enabled("off"));

    assert!(matches!(
        reg.set_enabled("unknown", true),
        Err(PipelineError::UnknownPipelineId(ref id)) if id == "unknown"
    ));
}

// ---------------------------------------------------------------------------
// 4. Cross-pipeline binding conflicts
// ---------------------------------------------------------------------------

#[test]
fn cross_pipeline_conflict_rejects_same_normalized_enabled_binding() {
    let mut reg = PipelineRegistry::new();
    let mut first = minimal_def("first");
    first.triggers = vec![TriggerBinding::hotkey(TriggerKind::HoldHotkey, "Ctrl+Space").unwrap()];
    reg.insert(first).unwrap();

    // Same physical key, different raw spelling: still conflicts after
    // normalization.
    let mut second = minimal_def("second");
    second.triggers = vec![TriggerBinding::hotkey(TriggerKind::HoldHotkey, "CTRL+SPACE").unwrap()];
    match reg.insert(second).expect_err("conflict expected") {
        PipelineError::CrossPipelineBindingConflict { binding, a, b } => {
            assert_eq!(binding, "ctrl+space");
            assert_eq!(a, "second");
            assert_eq!(b, "first");
        }
        other => panic!("unexpected error: {other:?}"),
    }

    // A different binding inserts cleanly.
    let mut third = minimal_def("third");
    third.triggers = vec![TriggerBinding::hotkey(TriggerKind::ToggleHotkey, "alt+t").unwrap()];
    reg.insert(third).expect("distinct binding is fine");
}

#[test]
fn disabled_bindings_are_exempt_from_conflicts() {
    let mut defs = Vec::new();
    let mut first = minimal_def("first");
    first.triggers = vec![hotkey_trigger("ctrl+space", true)];
    defs.push(first);

    // Same binding but disabled in another pipeline: no conflict.
    let mut second = minimal_def("second");
    second.manual_only = true; // otherwise the disabled trigger fails validation
    second.triggers = vec![hotkey_trigger("ctrl+space", false)];
    defs.push(second);

    PipelineRegistry::validate_bindings_across(&defs).expect("disabled bindings never conflict");
}

#[test]
fn static_cross_check_accepts_distinct_and_flags_shared() {
    let mut a = minimal_def("a");
    a.triggers = vec![hotkey_trigger("ctrl+space", true)];

    let mut b = minimal_def("b");
    b.triggers = vec![hotkey_trigger("alt+t", true)];

    PipelineRegistry::validate_bindings_across(&[a.clone(), b.clone()])
        .expect("distinct bindings pass");

    b.triggers.push(hotkey_trigger("ctrl+space", true));
    assert!(matches!(
        PipelineRegistry::validate_bindings_across(&[a, b]),
        Err(PipelineError::CrossPipelineBindingConflict { .. })
    ));
}

// ---------------------------------------------------------------------------
// 5. Persistence
// ---------------------------------------------------------------------------

fn seeded_registry() -> PipelineRegistry {
    let mut reg = PipelineRegistry::new();
    let mut a = minimal_def("alpha-pipe");
    a.perf_budget = Some(PerfBudget {
        target_p95_ms: 250,
        memory_ceiling_mb: 512,
    });
    a.nodes.push(node("punct", CapabilitySlot::TextPunctuation));
    a.nodes
        .push(node("clip", CapabilitySlot::DeliveryClipboard));
    // Legal chain: audio -> audio -> speech -> transcript -> transcript.
    // (delivery.* takes Text input, and no slot converts Transcript -> Text,
    // so the delivery leg is intentionally left unconnected here.)
    a.edges = vec![
        edge("capture", "vad"),
        edge("vad", "asr"),
        edge("asr", "punct"),
    ];
    a.nodes
        .insert(0, node("capture", CapabilitySlot::AudioCapture));
    a.nodes.insert(1, node("vad", CapabilitySlot::SpeechVad));
    a.triggers = vec![TriggerBinding::hotkey(TriggerKind::HoldHotkey, "Ctrl+Space").unwrap()];

    let b = def_with_optional_node("beta-pipe");
    reg.insert(a).unwrap();
    reg.insert(b).unwrap();
    reg.set_enabled("beta-pipe", false).unwrap();
    reg
}

fn def_with_optional_node(id: &str) -> PipelineDefinition {
    let mut def = PipelineDefinition::new(id, format!("Pipeline {id}"), Vec::new());
    let mut n = node("asr", CapabilitySlot::SpeechAsr);
    n.optional = true;
    n.device_preference = vec!["npu".to_string(), "gpu".to_string(), "cpu".to_string()];
    def.nodes = vec![n];
    def
}

#[test]
fn save_writes_per_pipeline_files_and_index() {
    let dir = tempfile::tempdir().unwrap();
    let reg = seeded_registry();
    reg.save_to_dir(dir.path()).unwrap();

    for summary in reg.list() {
        let path = dir.path().join(format!("{}.json", summary.id));
        assert!(
            path.is_file(),
            "missing per-pipeline file {}",
            path.display()
        );
    }
    assert!(dir.path().join(REGISTRY_INDEX_FILE).is_file());

    // Files end with a trailing newline (pretty JSON).
    let text = fs::read_to_string(dir.path().join("alpha-pipe.json")).unwrap();
    assert!(text.ends_with('\n'));
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["id"], "alpha-pipe");
    assert_eq!(parsed["schema_version"], PIPELINE_SCHEMA_VERSION);

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join(REGISTRY_INDEX_FILE)).unwrap())
            .unwrap();
    assert_eq!(index["schema_version"], PIPELINE_SCHEMA_VERSION);
    let ids: Vec<&str> = index["pipelines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(ids.len(), reg.list().len());
}

#[test]
fn load_round_trips_definitions_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let reg = seeded_registry();
    reg.save_to_dir(dir.path()).unwrap();

    let report = PipelineRegistry::load_from_dir(dir.path()).unwrap();
    assert!(
        report.skipped.is_empty(),
        "unexpected skips: {:?}",
        report.skipped
    );
    assert!(report.migrated.is_empty());

    let loaded = report.registry;
    for summary in reg.list() {
        let original = reg.get(&summary.id).unwrap();
        let restored = loaded.get(&summary.id).expect(summary.id.as_str());
        assert_eq!(
            original, restored,
            "definition {} changed on disk round-trip",
            summary.id
        );
        assert_eq!(
            loaded.is_enabled(&summary.id),
            reg.is_enabled(&summary.id),
            "enabled flag lost for {}",
            summary.id
        );
    }
    assert_eq!(loaded.list().len(), reg.list().len());
}

#[test]
fn corrupt_file_is_skipped_others_load() {
    let dir = tempfile::tempdir().unwrap();
    let good_path = dir.path().join("good.json");
    let def_json = serde_json::to_string_pretty(&minimal_def("good")).unwrap();
    fs::write(&good_path, format!("{def_json}\n")).unwrap();
    fs::write(dir.path().join("broken.json"), "{not json at all").unwrap();
    // Valid JSON that violates schema rules (no nodes).
    fs::write(
        dir.path().join("invalid-def.json"),
        r#"{"schema_version":1,"id":"invalid-def","version":1,"name":"x"}"#,
    )
    .unwrap();

    let report = PipelineRegistry::load_from_dir(dir.path()).unwrap();
    assert_eq!(report.registry.list().len(), 1, "only good.json loads");
    assert!(report.registry.get("good").is_some());

    let skipped_paths: Vec<&str> = report.skipped.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        skipped_paths.len(),
        2,
        "skips reported: {:?}",
        report.skipped
    );
    assert!(skipped_paths.iter().any(|p| p.ends_with("broken.json")));
    assert!(
        skipped_paths
            .iter()
            .any(|p| p.ends_with("invalid-def.json"))
    );
    assert!(report.skipped.iter().all(|s| !s.reason.is_empty()));
}

#[test]
fn legacy_file_missing_schema_field_migrates_to_schema_1() {
    let dir = tempfile::tempdir().unwrap();
    // Hand-authored pre-schema file: no schema_version, no defaults.
    let legacy = r#"{
        "id": "legacy-pipe",
        "version": 7,
        "name": "Legacy Pipe",
        "nodes": [
            {"id": "asr", "slot": "speech.asr",
             "module": {"namespace": "narrivy", "kind": "module",
                        "name": "whisper-tiny", "version_req": ">=1.0.0"}}
        ],
        "manual_only": true
    }"#;
    fs::write(dir.path().join("legacy-pipe.json"), legacy).unwrap();

    let report = PipelineRegistry::load_from_dir(dir.path()).unwrap();
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(report.migrated, vec!["legacy-pipe".to_string()]);

    let def = report.registry.get("legacy-pipe").unwrap();
    assert_eq!(def.schema_version, 1);
    assert_eq!(def.version, 7, "user version untouched by migration");
    // Serde defaults filled in the omitted fields.
    assert!(def.nodes[0].enabled, "node enabled default is true");
    assert!(!def.nodes[0].optional, "node optional default is false");
    assert!(def.triggers.is_empty());
    assert!(
        def.created_unix_ms > 0 && def.updated_unix_ms > 0,
        "registry stamps zero timestamps when re-inserting a migrated file"
    );
}

#[test]
fn load_rejects_future_schema_and_nonexistent_dir() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("future.json"),
        r#"{"schema_version":99,"id":"future","version":1,"name":"f","nodes":[],"manual_only":true}"#,
    )
    .unwrap();
    let report = PipelineRegistry::load_from_dir(dir.path()).unwrap();
    assert_eq!(report.registry.list().len(), 0);
    assert_eq!(report.skipped.len(), 1);

    let missing = Path::new("/nonexistent/narrivy-store-definitely-missing");
    match PipelineRegistry::load_from_dir(missing) {
        Err(PipelineError::Io(io)) => assert_eq!(io.kind(), std::io::ErrorKind::NotFound),
        other => panic!("expected io NotFound, got {other:?}"),
    }
}

#[test]
fn corrupt_index_is_reported_but_does_not_block_load() {
    let dir = tempfile::tempdir().unwrap();
    let def_json = serde_json::to_string_pretty(&minimal_def("survivor")).unwrap();
    fs::write(dir.path().join("survivor.json"), format!("{def_json}\n")).unwrap();
    fs::write(dir.path().join(REGISTRY_INDEX_FILE), "}{ broken index").unwrap();

    let report = PipelineRegistry::load_from_dir(dir.path()).unwrap();
    assert_eq!(
        report.registry.get("survivor").map(|d| d.id.as_str()),
        Some("survivor")
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.path.ends_with(REGISTRY_INDEX_FILE))
    );
}

// ---------------------------------------------------------------------------
// 6. Resources: identity vs location separation
// ---------------------------------------------------------------------------

fn sample_identity() -> ResourceIdentity {
    ResourceIdentity {
        kind: ResourceKind::Model,
        namespace: "upstream".to_string(),
        name: "whisper-tiny".to_string(),
        version: "1.2.3".to_string(),
        dependencies: vec![ResourceRef::narrivy_module("runtime-base", ">=2.0.0")],
        compatibility: vec!["windows-x86_64".to_string()],
    }
}

#[test]
fn resource_identity_has_no_url_field_on_the_wire() {
    let json = serde_json::to_value(sample_identity()).unwrap();
    let obj = json.as_object().expect("identity serializes to an object");
    let keys: Vec<&String> = obj.keys().collect();
    for forbidden in ["url", "urls", "upstream_url", "mirror_url", "download_url"] {
        assert!(
            !obj.contains_key(forbidden),
            "ResourceIdentity leaked location field {forbidden:?}; keys were {keys:?}"
        );
    }
    assert!(obj.contains_key("kind"));
    assert!(obj.contains_key("namespace"));
    assert!(obj.contains_key("name"));
    assert!(obj.contains_key("version"));
}

#[test]
fn resolution_hint_carries_urls_separately() {
    let hint = ResolutionHint {
        identity: sample_identity(),
        upstream_url: Some("https://example.com/whisper-tiny-1.2.3.tar.zst".to_string()),
        mirror_url: Some("https://mirror.example.net/wt.tzst".to_string()),
        sha256: Some("deadbeef".to_string()),
    };
    let json = serde_json::to_value(&hint).unwrap();
    let obj = json.as_object().unwrap();
    assert!(obj.contains_key("upstream_url"));
    assert!(obj.contains_key("mirror_url"));
    assert!(obj.contains_key("sha256"));
    // And the identity nested inside still has no URL of its own.
    assert!(!hint.identity.dependencies.is_empty());
    let hint_json_text = serde_json::to_string(&hint).unwrap();
    assert!(hint_json_text.contains("\"upstream_url\""));

    // Omitted optional fields deserialize to None via serde defaults.
    let bare: ResolutionHint = serde_json::from_str(
        r#"{"identity":{"kind":"model","namespace":"upstream","name":"m","version":"1.0.0"}}"#,
    )
    .unwrap();
    assert!(bare.upstream_url.is_none());
    assert!(bare.mirror_url.is_none());
    assert!(bare.sha256.is_none());
    assert!(bare.identity.dependencies.is_empty());
    assert!(bare.identity.compatibility.is_empty());
}

#[test]
fn satisfies_requires_kind_name_and_version_match() {
    let identity = sample_identity(); // model / upstream / whisper-tiny / 1.2.3

    let req = |kind: ResourceKind, name: &str, vr: &str| ResourceRef {
        namespace: "upstream".to_string(),
        kind,
        name: name.to_string(),
        version_req: vr.to_string(),
    };

    // Exact version requirement.
    assert!(identity.satisfies(&req(ResourceKind::Model, "whisper-tiny", "1.2.3")));
    assert!(!identity.satisfies(&req(ResourceKind::Model, "whisper-tiny", "1.2.4")));
    // Minimum version requirement (>=).
    assert!(identity.satisfies(&req(ResourceKind::Model, "whisper-tiny", ">=1.2.3")));
    assert!(identity.satisfies(&req(ResourceKind::Model, "whisper-tiny", ">=1.0.0")));
    assert!(!identity.satisfies(&req(ResourceKind::Model, "whisper-tiny", ">=1.2.4")));

    // Kind mismatch.
    assert!(!identity.satisfies(&req(ResourceKind::Runtime, "whisper-tiny", "1.2.3")));
    // Name mismatch.
    assert!(!identity.satisfies(&req(ResourceKind::Model, "whisper-small", "1.2.3")));
    // Unparseable versions never satisfy.
    let weird = ResourceIdentity {
        kind: ResourceKind::Model,
        namespace: "upstream".to_string(),
        name: "whisper-tiny".to_string(),
        version: "beta-9".to_string(),
        dependencies: vec![],
        compatibility: vec![],
    };
    assert!(!weird.satisfies(&req(ResourceKind::Model, "whisper-tiny", ">=1.0.0")));
    assert!(!weird.satisfies(&req(ResourceKind::Model, "whisper-tiny", "1.0.0")));
}

#[test]
fn version_compare_zero_pads_components() {
    let identity = ResourceIdentity {
        kind: ResourceKind::Module,
        namespace: "narrivy".to_string(),
        name: "m".to_string(),
        version: "1.2".to_string(),
        dependencies: vec![],
        compatibility: vec![],
    };
    let req = |vr: &str| ResourceRef::narrivy_module("m", vr);
    assert!(
        identity.satisfies(&req(">=1.2.0")),
        "1.2 == 1.2.0 componentwise"
    );
    assert!(identity.satisfies(&req("1.2.0")));
    assert!(identity.satisfies(&req("1.2"))); // exact string shortcut
    assert!(!identity.satisfies(&req(">=1.2.1")));
}

#[test]
fn resource_ref_validation_rules() {
    let ok = ResourceRef::new("narrivy", ResourceKind::PipelinePack, "pack-x", "1.0.0");
    assert!(ok.validate().is_ok());
    let upstream_ok = ResourceRef::new("upstream", ResourceKind::Model, "model-y", ">=2.0.0");
    assert!(upstream_ok.validate().is_ok());

    let bad_ns = ResourceRef::new("github", ResourceKind::Module, "m", "1.0.0");
    assert!(matches!(
        bad_ns.validate(),
        Err(PipelineError::InvalidResourceRef(_))
    ));
    let blank_name = ResourceRef::new("narrivy", ResourceKind::Module, " ", "1.0.0");
    assert!(matches!(
        blank_name.validate(),
        Err(PipelineError::EmptyField { .. })
    ));
    let blank_req = ResourceRef::new("narrivy", ResourceKind::Module, "m", "");
    assert!(matches!(
        blank_req.validate(),
        Err(PipelineError::EmptyField { .. })
    ));
}

// ---------------------------------------------------------------------------
// 7. Slot I/O table mechanical completeness
// ---------------------------------------------------------------------------

#[test]
fn every_slot_has_a_typed_io_pair() {
    for slot in CapabilitySlot::ALL {
        let (inp, out) = slot_io(slot);
        // The pair must be built from the closed StreamKind vocabulary; the
        // call returning a concrete pair for each of the 16 variants is the
        // mechanical completeness check.
        for _ in [inp, out] {}

        // Wire names agree with the serde representation.
        let wire = serde_json::to_string(&slot).unwrap();
        assert_eq!(wire, format!("{:?}", slot.wire_name()));
        assert_eq!(slot.to_string(), slot.wire_name());
    }
    assert_eq!(CapabilitySlot::ALL.len(), 16);
    let distinct: std::collections::BTreeSet<_> = CapabilitySlot::ALL.into_iter().collect();
    assert_eq!(
        distinct.len(),
        16,
        "closed vocabulary holds 16 distinct slots"
    );
}

#[test]
fn known_io_pairs_spot_checked() {
    use StreamKind::{Audio, Speech, Text, Transcript};
    assert_eq!(
        slot_io(CapabilitySlot::ActivationGlobalHotkey),
        (Text, Text)
    );
    assert_eq!(slot_io(CapabilitySlot::AudioCapture), (Audio, Audio));
    assert_eq!(slot_io(CapabilitySlot::AudioNormalize), (Audio, Audio));
    assert_eq!(slot_io(CapabilitySlot::AudioDenoise), (Audio, Audio));
    assert_eq!(slot_io(CapabilitySlot::SpeechVad), (Audio, Speech));
    assert_eq!(slot_io(CapabilitySlot::SpeechEndpoint), (Speech, Speech));
    assert_eq!(slot_io(CapabilitySlot::SpeechAsr), (Speech, Transcript));
    assert_eq!(slot_io(CapabilitySlot::TextPunctuation), (Transcript, Text));
    assert_eq!(slot_io(CapabilitySlot::TextItn), (Text, Text));
    assert_eq!(slot_io(CapabilitySlot::TextSemanticGuard), (Text, Text));
    assert_eq!(slot_io(CapabilitySlot::TextLlmPostprocess), (Text, Text));
    assert_eq!(slot_io(CapabilitySlot::DeliveryDirect), (Text, Text));
    assert_eq!(slot_io(CapabilitySlot::DeliveryClipboard), (Text, Text));
    assert_eq!(slot_io(CapabilitySlot::DeliveryInbox), (Text, Text));
    assert_eq!(slot_io(CapabilitySlot::ContextProvider), (Text, Text));
    assert_eq!(
        slot_io(CapabilitySlot::QualityQualifier),
        (Transcript, Transcript)
    );
}

#[test]
fn singleton_slot_list_matches_catalog_subset() {
    assert_eq!(SINGLETON_SLOTS.len(), 6);
    for slot in SINGLETON_SLOTS {
        assert!(CapabilitySlot::ALL.contains(&slot));
    }
    // Spot-check membership rather than exact order (order is an internal detail).
    for expected in [
        CapabilitySlot::SpeechAsr,
        CapabilitySlot::ContextProvider,
        CapabilitySlot::QualityQualifier,
    ] {
        assert!(SINGLETON_SLOTS.contains(&expected));
    }
}

// ---------------------------------------------------------------------------
// 8. Serde wire stability
// ---------------------------------------------------------------------------

#[test]
fn definition_serialization_round_trip_is_lossless() {
    let reg = seeded_registry();
    for summary in reg.list() {
        let original = reg.get(&summary.id).unwrap();
        let json = serde_json::to_string(original).unwrap();
        let parsed: PipelineDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(&parsed, original);
    }
}

#[test]
fn deserializer_tolerates_unknown_fields() {
    let mut json = serde_json::to_string(&minimal_def("bogus-field")).unwrap();
    // Inject a bogus top-level field plus bogus fields inside a node object.
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let mut value = value;
    value["totally_unknown_top_level"] = serde_json::json!({"x": [1, 2, 3]});
    value["nodes"][0]["future_node_option"] = serde_json::json!("reserved");
    json = serde_json::to_string(&value).unwrap();

    let parsed: PipelineDefinition = serde_json::from_str(&json).expect("unknown fields tolerated");
    assert_eq!(parsed.id, "bogus-field");
    assert_eq!(parsed.version, 1);
}

#[test]
fn stream_kind_and_trigger_kind_wire_names() {
    assert_eq!(
        serde_json::to_string(&StreamKind::Audio).unwrap(),
        "\"audio\""
    );
    assert_eq!(
        serde_json::to_string(&StreamKind::Speech).unwrap(),
        "\"speech\""
    );
    assert_eq!(
        serde_json::to_string(&StreamKind::Transcript).unwrap(),
        "\"transcript\""
    );
    assert_eq!(
        serde_json::to_string(&StreamKind::Text).unwrap(),
        "\"text\""
    );

    assert_eq!(
        serde_json::to_string(&TriggerKind::HoldHotkey).unwrap(),
        "\"hold-hotkey\""
    );
    assert_eq!(
        serde_json::to_string(&TriggerKind::ToggleHotkey).unwrap(),
        "\"toggle-hotkey\""
    );
    assert_eq!(
        serde_json::to_string(&TriggerKind::AppRule).unwrap(),
        "\"app-rule\""
    );
    assert_eq!(serde_json::to_string(&TriggerKind::Cli).unwrap(), "\"cli\"");
    assert_eq!(
        serde_json::to_string(&TriggerKind::WakePhrase).unwrap(),
        "\"wake-phrase\""
    );
}

#[test]
fn perf_budget_round_trips_inline() {
    let mut def = minimal_def("budgeted");
    def.perf_budget = Some(PerfBudget {
        target_p95_ms: 300,
        memory_ceiling_mb: 256,
    });
    let json = serde_json::to_string(&def).unwrap();
    assert!(json.contains("\"perf_budget\":{\"target_p95_ms\":300,\"memory_ceiling_mb\":256}"));
    let parsed: PipelineDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.perf_budget, def.perf_budget);
}
