//! Declarative pipelines (ADR-0016) and resource identity (ADR-0017).
//!
//! This crate owns the first-class, versioned Pipeline schema: a typed
//! capability graph with a closed stream-kind vocabulary, a mechanical
//! per-slot I/O contract, trigger bindings with a canonical grammar, and a
//! registry that validates, versions, and persists definitions.
//!
//! Design points (see `IMPLEMENTATION-NOTES.md`):
//!
//! - Capability slots use the catalog vocabulary (`spec/catalogs/`), serialized
//!   as dotted wire names such as `"speech.asr"`.
//! - [`StreamKind`] is a closed enum; edge validation is the mechanical
//!   comparison `slot_io(from.slot).1 == slot_io(to.slot).0`.
//! - [`ResourceIdentity`] never carries a download URL; location lives in
//!   [`ResolutionHint`] (identity vs location separation, ADR-0017).
//! - Node `enabled` (user on/off toggle) and `optional` (skippable on failure)
//!   are distinct axes (gaps register B4).
//! - Pipelines are individually versioned; the registry enforces
//!   version-bump-on-save and cross-pipeline trigger-binding conflicts.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub mod legacy;

/// Current pipeline schema version produced and accepted by this crate.
pub const PIPELINE_SCHEMA_VERSION: u32 = 1;

/// Name of the registry index file inside a pipeline store directory.
pub const REGISTRY_INDEX_FILE: &str = "registry.json";

// ---------------------------------------------------------------------------
// Capability slots
// ---------------------------------------------------------------------------

/// Capability slots a pipeline node can occupy.
///
/// The set is the pipeline-relevant subset of the capability vocabulary in
/// `spec/catalogs/modules.json` + `spec/catalogs/pipelines.json`. Serde wire
/// names are the dotted catalog capability names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CapabilitySlot {
    #[serde(rename = "activation.global-hotkey")]
    ActivationGlobalHotkey,
    #[serde(rename = "audio.capture")]
    AudioCapture,
    #[serde(rename = "audio.normalize")]
    AudioNormalize,
    #[serde(rename = "audio.denoise")]
    AudioDenoise,
    #[serde(rename = "speech.vad")]
    SpeechVad,
    #[serde(rename = "speech.endpoint")]
    SpeechEndpoint,
    #[serde(rename = "speech.asr")]
    SpeechAsr,
    #[serde(rename = "text.punctuation")]
    TextPunctuation,
    #[serde(rename = "text.itn")]
    TextItn,
    #[serde(rename = "text.semantic-guard")]
    TextSemanticGuard,
    #[serde(rename = "text.llm-postprocess")]
    TextLlmPostprocess,
    #[serde(rename = "delivery.direct")]
    DeliveryDirect,
    #[serde(rename = "delivery.clipboard")]
    DeliveryClipboard,
    #[serde(rename = "delivery.inbox")]
    DeliveryInbox,
    #[serde(rename = "context.provider")]
    ContextProvider,
    #[serde(rename = "quality.qualifier")]
    QualityQualifier,
}

impl CapabilitySlot {
    /// Catalog wire name (same string serde uses).
    pub fn wire_name(self) -> &'static str {
        match self {
            CapabilitySlot::ActivationGlobalHotkey => "activation.global-hotkey",
            CapabilitySlot::AudioCapture => "audio.capture",
            CapabilitySlot::AudioNormalize => "audio.normalize",
            CapabilitySlot::AudioDenoise => "audio.denoise",
            CapabilitySlot::SpeechVad => "speech.vad",
            CapabilitySlot::SpeechEndpoint => "speech.endpoint",
            CapabilitySlot::SpeechAsr => "speech.asr",
            CapabilitySlot::TextPunctuation => "text.punctuation",
            CapabilitySlot::TextItn => "text.itn",
            CapabilitySlot::TextSemanticGuard => "text.semantic-guard",
            CapabilitySlot::TextLlmPostprocess => "text.llm-postprocess",
            CapabilitySlot::DeliveryDirect => "delivery.direct",
            CapabilitySlot::DeliveryClipboard => "delivery.clipboard",
            CapabilitySlot::DeliveryInbox => "delivery.inbox",
            CapabilitySlot::ContextProvider => "context.provider",
            CapabilitySlot::QualityQualifier => "quality.qualifier",
        }
    }

    /// Every slot in the closed vocabulary.
    pub const ALL: [CapabilitySlot; 16] = [
        CapabilitySlot::ActivationGlobalHotkey,
        CapabilitySlot::AudioCapture,
        CapabilitySlot::AudioNormalize,
        CapabilitySlot::AudioDenoise,
        CapabilitySlot::SpeechVad,
        CapabilitySlot::SpeechEndpoint,
        CapabilitySlot::SpeechAsr,
        CapabilitySlot::TextPunctuation,
        CapabilitySlot::TextItn,
        CapabilitySlot::TextSemanticGuard,
        CapabilitySlot::TextLlmPostprocess,
        CapabilitySlot::DeliveryDirect,
        CapabilitySlot::DeliveryClipboard,
        CapabilitySlot::DeliveryInbox,
        CapabilitySlot::ContextProvider,
        CapabilitySlot::QualityQualifier,
    ];
}

impl std::fmt::Display for CapabilitySlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire_name())
    }
}

/// Slots a pipeline may contain at most once.
pub const SINGLETON_SLOTS: [CapabilitySlot; 6] = [
    CapabilitySlot::AudioCapture,
    CapabilitySlot::SpeechAsr,
    CapabilitySlot::SpeechEndpoint,
    CapabilitySlot::ContextProvider,
    CapabilitySlot::DeliveryDirect,
    CapabilitySlot::QualityQualifier,
];

// ---------------------------------------------------------------------------
// Stream kinds and per-slot I/O contract
// ---------------------------------------------------------------------------

/// Closed stream-kind vocabulary carried by edges (gaps register B5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    Audio,
    Speech,
    Transcript,
    Text,
}

/// Mechanical I/O contract: `(input kind, output kind)` per slot.
///
/// `activation.global-hotkey` is a control-only slot: it conceptually passes
/// the control token through, so it is typed `(Text, Text)`.
pub fn slot_io(slot: CapabilitySlot) -> (StreamKind, StreamKind) {
    match slot {
        CapabilitySlot::ActivationGlobalHotkey => (StreamKind::Text, StreamKind::Text),
        CapabilitySlot::AudioCapture => (StreamKind::Audio, StreamKind::Audio),
        CapabilitySlot::AudioNormalize => (StreamKind::Audio, StreamKind::Audio),
        CapabilitySlot::AudioDenoise => (StreamKind::Audio, StreamKind::Audio),
        CapabilitySlot::SpeechVad => (StreamKind::Audio, StreamKind::Speech),
        CapabilitySlot::SpeechEndpoint => (StreamKind::Speech, StreamKind::Speech),
        CapabilitySlot::SpeechAsr => (StreamKind::Speech, StreamKind::Transcript),
        // Deterministic punctuation is the canonical TRANSCRIPT-RENDERING
        // entry stage (ADR-0016 reference default: ASR -> punctuation -> ITN
        // -> guard -> delivery). It converts the engine transcript into the
        // final text stream; every later text stage consumes/produces Text,
        // making full dictation chains (ASR -> ... -> delivery) expressible.
        CapabilitySlot::TextPunctuation => (StreamKind::Transcript, StreamKind::Text),
        CapabilitySlot::TextItn => (StreamKind::Text, StreamKind::Text),
        CapabilitySlot::TextSemanticGuard => (StreamKind::Text, StreamKind::Text),
        CapabilitySlot::TextLlmPostprocess => (StreamKind::Text, StreamKind::Text),
        CapabilitySlot::DeliveryDirect => (StreamKind::Text, StreamKind::Text),
        CapabilitySlot::DeliveryClipboard => (StreamKind::Text, StreamKind::Text),
        CapabilitySlot::DeliveryInbox => (StreamKind::Text, StreamKind::Text),
        CapabilitySlot::ContextProvider => (StreamKind::Text, StreamKind::Text),
        CapabilitySlot::QualityQualifier => (StreamKind::Transcript, StreamKind::Transcript),
    }
}

// ---------------------------------------------------------------------------
// Resource references and identity
// ---------------------------------------------------------------------------

/// The four distinct pack kinds (ADR-0017 / PORT-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    Module,
    Runtime,
    Model,
    PipelinePack,
}

/// Reference from a pipeline/node to a required resource.
///
/// `namespace` is `"narrivy"` or `"upstream"`. `version_req` uses the simple
/// grammar `"x.y.z"` (exact) or `">=x.y.z"` (minimum).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceRef {
    pub namespace: String,
    pub kind: ResourceKind,
    pub name: String,
    pub version_req: String,
}

impl ResourceRef {
    pub fn new(
        namespace: impl Into<String>,
        kind: ResourceKind,
        name: impl Into<String>,
        version_req: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            kind,
            name: name.into(),
            version_req: version_req.into(),
        }
    }

    /// Convenience constructor for Narrivy-owned module packs.
    pub fn narrivy_module(name: impl Into<String>, version_req: impl Into<String>) -> Self {
        Self::new("narrivy", ResourceKind::Module, name, version_req)
    }

    /// Structural validation of the reference itself.
    pub fn validate(&self) -> Result<(), PipelineError> {
        if self.namespace != "narrivy" && self.namespace != "upstream" {
            return Err(PipelineError::InvalidResourceRef(format!(
                "namespace must be \"narrivy\" or \"upstream\", got {:?}",
                self.namespace
            )));
        }
        if self.name.trim().is_empty() {
            return Err(PipelineError::EmptyField {
                field: "resource name".to_string(),
            });
        }
        if self.version_req.trim().is_empty() {
            return Err(PipelineError::EmptyField {
                field: "resource version_req".to_string(),
            });
        }
        Ok(())
    }
}

/// Stable identity of an installable resource.
///
/// Deliberately carries NO download URL: identity is what a resource IS,
/// location is where a copy may be fetched from ([`ResolutionHint`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceIdentity {
    pub kind: ResourceKind,
    pub namespace: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub dependencies: Vec<ResourceRef>,
    #[serde(default)]
    pub compatibility: Vec<String>,
}

impl ResourceIdentity {
    /// Does this installed identity satisfy the given reference?
    ///
    /// Requires kind + name match and a version satisfying `version_req`
    /// (`"x.y.z"` exact or `">=x.y.z"` minimum, componentwise numeric compare
    /// with zero-padding). Unparseable versions never satisfy.
    pub fn satisfies(&self, req: &ResourceRef) -> bool {
        if req.kind != self.kind || req.name != self.name {
            return false;
        }
        version_satisfies(&self.version, &req.version_req)
    }
}

/// Where a copy of a resource may be obtained, plus expected integrity data.
///
/// Kept strictly separate from [`ResourceIdentity`] per ADR-0017: a mutable
/// URL is never sufficient identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionHint {
    pub identity: ResourceIdentity,
    #[serde(default)]
    pub upstream_url: Option<String>,
    #[serde(default)]
    pub mirror_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Componentwise numeric version comparison; `None` when unparseable.
fn cmp_versions(a: &str, b: &str) -> Option<Ordering> {
    let parse =
        |s: &str| -> Option<Vec<u64>> { s.split('.').map(|p| p.parse::<u64>().ok()).collect() };
    let (pa, pb) = (parse(a)?, parse(b)?);
    let len = pa.len().max(pb.len());
    for i in 0..len {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            other => return Some(other),
        }
    }
    Some(Ordering::Equal)
}

/// `"x.y.z"` exact or `">=x.y.z"` minimum semantics.
fn version_satisfies(version: &str, version_req: &str) -> bool {
    let req = version_req.trim();
    if let Some(minimum) = req.strip_prefix(">=") {
        return matches!(
            cmp_versions(version, minimum.trim()),
            Some(Ordering::Greater | Ordering::Equal)
        );
    }
    if req == version {
        return true;
    }
    matches!(cmp_versions(version, req), Some(Ordering::Equal))
}

// ---------------------------------------------------------------------------
// Trigger bindings
// ---------------------------------------------------------------------------

/// How a pipeline is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TriggerKind {
    HoldHotkey,
    ToggleHotkey,
    AppRule,
    Cli,
    WakePhrase,
}

/// Canonical modifier order: ctrl < alt < shift < super, then the key.
const MODIFIER_ORDER: [&str; 4] = ["ctrl", "alt", "shift", "super"];

fn is_modifier(token: &str) -> bool {
    MODIFIER_ORDER.contains(&token)
}

/// Normalize a hotkey binding to the canonical grammar.
///
/// Grammar: `modifier+key`, modifiers lowercased, deduplicated, and emitted in
/// canonical order (`ctrl`, `alt`, `shift`, `super`), followed by exactly one
/// non-modifier key, all joined with `+`. Example: `"Shift+CTRL+Space"` ->
/// `"ctrl+shift+space"`.
pub fn normalize_binding(raw: &str) -> Result<String, PipelineError> {
    let invalid = |reason: &str| PipelineError::InvalidBinding {
        raw: raw.to_string(),
        reason: reason.to_string(),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid("binding is empty"));
    }
    let mut modifiers: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for token in trimmed.split('+') {
        let lowered = token.trim().to_ascii_lowercase();
        if lowered.is_empty() {
            return Err(invalid("empty segment around '+'"));
        }
        if is_modifier(&lowered) {
            if modifiers.contains(&lowered) {
                return Err(invalid("duplicate modifier"));
            }
            modifiers.push(lowered);
        } else {
            keys.push(lowered);
        }
    }
    if keys.len() != 1 {
        return Err(invalid("exactly one non-modifier key is required"));
    }
    let mut parts: Vec<String> = MODIFIER_ORDER
        .iter()
        .filter(|m| modifiers.iter().any(|modi| modi == *m))
        .map(|m| (*m).to_string())
        .collect();
    parts.push(keys.remove(0));
    Ok(parts.join("+"))
}

/// One trigger bound to a pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TriggerBinding {
    pub kind: TriggerKind,
    /// Stored in normalized form (see [`normalize_binding`] for hotkey kinds).
    pub binding: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl TriggerBinding {
    pub fn hotkey(kind: TriggerKind, binding: &str) -> Result<Self, PipelineError> {
        Ok(Self {
            kind,
            binding: normalize_binding(binding)?,
            enabled: true,
        })
    }

    /// Canonical form for this binding given its kind. Hotkey kinds use the
    /// strict grammar; `AppRule`/`Cli`/`WakePhrase` use trimmed lowercase.
    pub fn canonical(&self) -> Result<String, PipelineError> {
        match self.kind {
            TriggerKind::HoldHotkey | TriggerKind::ToggleHotkey => normalize_binding(&self.binding),
            TriggerKind::AppRule | TriggerKind::Cli | TriggerKind::WakePhrase => {
                let trimmed = self.binding.trim().to_ascii_lowercase();
                if trimmed.is_empty() {
                    return Err(PipelineError::InvalidBinding {
                        raw: self.binding.clone(),
                        reason: "binding is empty".to_string(),
                    });
                }
                Ok(trimmed)
            }
        }
    }
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Pipeline definition
// ---------------------------------------------------------------------------

/// Optional performance budget (gaps register B2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerfBudget {
    pub target_p95_ms: u32,
    pub memory_ceiling_mb: u32,
}

/// Directed edge between two nodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PipelineEdge {
    pub from_node: String,
    pub to_node: String,
}

/// One stage in the pipeline graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineNode {
    pub id: String,
    pub slot: CapabilitySlot,
    pub module: ResourceRef,
    /// User on/off toggle. Distinct from `optional` (gaps register B4).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Skippable on failure without failing the pipeline.
    #[serde(default)]
    pub optional: bool,
    /// Ordered device preference, e.g. `["npu", "gpu", "cpu"]`.
    #[serde(default)]
    pub device_preference: Vec<String>,
}

/// A first-class, versioned pipeline definition (ADR-0016).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineDefinition {
    /// Storage schema version. Defaults to 1 on load; 0/missing is migrated.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Slug `[a-z0-9-]{1,64}`.
    pub id: String,
    /// Per-pipeline monotonic version, starts at 1 (gaps register B1).
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub nodes: Vec<PipelineNode>,
    #[serde(default)]
    pub edges: Vec<PipelineEdge>,
    #[serde(default)]
    pub triggers: Vec<TriggerBinding>,
    /// When true, the pipeline is invoked explicitly and may have no triggers.
    #[serde(default)]
    pub manual_only: bool,
    #[serde(default)]
    pub perf_budget: Option<PerfBudget>,
    #[serde(default)]
    pub created_unix_ms: u64,
    #[serde(default)]
    pub updated_unix_ms: u64,
}

fn default_schema_version() -> u32 {
    PIPELINE_SCHEMA_VERSION
}

impl PipelineDefinition {
    /// Minimal constructor: schema 1, version 1, no edges/triggers,
    /// manual-only until a trigger is bound, zero timestamps (stamped by the
    /// registry on insert).
    pub fn new(id: impl Into<String>, name: impl Into<String>, nodes: Vec<PipelineNode>) -> Self {
        Self {
            schema_version: PIPELINE_SCHEMA_VERSION,
            id: id.into(),
            version: 1,
            name: name.into(),
            nodes,
            edges: Vec::new(),
            triggers: Vec::new(),
            manual_only: true,
            perf_budget: None,
            created_unix_ms: 0,
            updated_unix_ms: 0,
        }
    }

    fn is_valid_slug(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    /// Full structural + graph validation.
    pub fn validate(&self) -> Result<(), PipelineError> {
        if !Self::is_valid_slug(&self.id) {
            return Err(PipelineError::InvalidId(self.id.clone()));
        }
        if self.version < 1 {
            return Err(PipelineError::InvalidVersion(self.version));
        }
        if self.name.trim().is_empty() {
            return Err(PipelineError::EmptyField {
                field: "name".to_string(),
            });
        }
        if self.schema_version > PIPELINE_SCHEMA_VERSION {
            return Err(PipelineError::UnsupportedSchemaVersion {
                found: self.schema_version,
                supported: PIPELINE_SCHEMA_VERSION,
            });
        }
        if self.nodes.is_empty() {
            return Err(PipelineError::EmptyField {
                field: "nodes".to_string(),
            });
        }

        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for node in &self.nodes {
            if node.id.trim().is_empty() {
                return Err(PipelineError::EmptyField {
                    field: "node id".to_string(),
                });
            }
            if !seen.insert(node.id.as_str()) {
                return Err(PipelineError::DuplicateNodeId(node.id.clone()));
            }
            node.module.validate()?;
        }

        let asr_count = self
            .nodes
            .iter()
            .filter(|n| n.slot == CapabilitySlot::SpeechAsr)
            .count();
        if asr_count != 1 {
            return Err(PipelineError::MissingOrMultipleAsr { found: asr_count });
        }

        for slot in SINGLETON_SLOTS {
            let count = self.nodes.iter().filter(|n| n.slot == slot).count();
            if count > 1 {
                return Err(PipelineError::SingletonViolation {
                    slot: slot.wire_name().to_string(),
                    count,
                });
            }
        }

        let by_id: BTreeMap<&str, &PipelineNode> =
            self.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        for edge in &self.edges {
            let from =
                by_id
                    .get(edge.from_node.as_str())
                    .ok_or_else(|| PipelineError::UnknownNode {
                        node: edge.from_node.clone(),
                    })?;
            let to =
                by_id
                    .get(edge.to_node.as_str())
                    .ok_or_else(|| PipelineError::UnknownNode {
                        node: edge.to_node.clone(),
                    })?;
            let (_, out) = slot_io(from.slot);
            let (inp, _) = slot_io(to.slot);
            if out != inp {
                return Err(PipelineError::StreamMismatch {
                    from: from.id.clone(),
                    to: to.id.clone(),
                    out: format!("{out:?}"),
                    inp: format!("{inp:?}"),
                });
            }
        }
        self.ensure_acyclic(&by_id)?;
        self.validate_triggers()?;
        Ok(())
    }

    /// Kahn topological check; reports the first node left unprocessed.
    fn ensure_acyclic(&self, by_id: &BTreeMap<&str, &PipelineNode>) -> Result<(), PipelineError> {
        let mut indegree: BTreeMap<&str, usize> = by_id.keys().map(|k| (*k, 0)).collect();
        // Edge endpoints were validated against known node ids above.
        for edge in &self.edges {
            if let Some(degree) = indegree.get_mut(edge.to_node.as_str()) {
                *degree += 1;
            }
        }
        let mut queue: Vec<&str> = indegree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(k, _)| *k)
            .collect();
        let mut processed = 0usize;
        while let Some(id) = queue.pop() {
            processed += 1;
            for edge in &self.edges {
                if edge.from_node == id {
                    if let Some(d) = indegree.get_mut(edge.to_node.as_str()) {
                        *d -= 1;
                        if *d == 0 {
                            queue.push(edge.to_node.as_str());
                        }
                    }
                }
            }
        }
        if processed != by_id.len() {
            let stuck = by_id
                .keys()
                .find(|k| indegree.get(*k).copied().unwrap_or(0) > 0)
                .copied()
                .unwrap_or_default();
            return Err(PipelineError::CycleDetected {
                node: stuck.to_string(),
            });
        }
        Ok(())
    }

    fn validate_triggers(&self) -> Result<(), PipelineError> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for trigger in &self.triggers {
            let canonical = trigger.canonical()?;
            if trigger.enabled && trigger.binding != canonical {
                return Err(PipelineError::InvalidBinding {
                    raw: trigger.binding.clone(),
                    reason: format!("binding must be stored in canonical form {canonical:?}"),
                });
            }
            if trigger.enabled && !seen.insert(canonical) {
                return Err(PipelineError::DuplicateBinding {
                    binding: trigger.binding.clone(),
                });
            }
        }
        if !self.manual_only && !self.triggers.iter().any(|t| t.enabled) {
            return Err(PipelineError::NoEnabledTrigger);
        }
        Ok(())
    }

    /// Normalized bindings of enabled triggers (for conflict checks).
    pub fn enabled_binding_keys(&self) -> Result<Vec<String>, PipelineError> {
        let mut keys = Vec::new();
        for trigger in &self.triggers {
            if trigger.enabled {
                keys.push(trigger.canonical()?);
            }
        }
        Ok(keys)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(
        "pipeline id {0:?} is not a valid slug (lowercase alphanumerics and '-', 1..=64 chars)"
    )]
    InvalidId(String),
    #[error("field `{field}` must not be empty")]
    EmptyField { field: String },
    #[error("duplicate node id {0:?}")]
    DuplicateNodeId(String),
    #[error("pipeline must contain exactly one speech.asr node, found {found}")]
    MissingOrMultipleAsr { found: usize },
    #[error("singleton capability {slot:?} appears {count} times (at most 1 allowed)")]
    SingletonViolation { slot: String, count: usize },
    #[error("edge references unknown node {node:?}")]
    UnknownNode { node: String },
    #[error("cycle detected in pipeline graph (involving node {node:?})")]
    CycleDetected { node: String },
    #[error("edge {from:?} -> {to:?}: output stream {out} does not match input stream {inp}")]
    StreamMismatch {
        from: String,
        to: String,
        out: String,
        inp: String,
    },
    #[error("pipeline has no enabled trigger and manual_only is false")]
    NoEnabledTrigger,
    #[error("duplicate enabled trigger binding {binding:?}")]
    DuplicateBinding { binding: String },
    #[error("invalid trigger binding {raw:?}: {reason}")]
    InvalidBinding { raw: String, reason: String },
    #[error("pipeline version must be >= 1, got {0}")]
    InvalidVersion(u32),
    #[error("schema_version {found} is newer than supported version {supported}")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    #[error("invalid resource reference: {0}")]
    InvalidResourceRef(String),
    #[error("duplicate pipeline id {0:?}")]
    DuplicatePipelineId(String),
    #[error("unknown pipeline id {0:?}")]
    UnknownPipelineId(String),
    #[error("version bump rejected: expected version {expected}, got {actual}")]
    VersionBumpRejected { expected: u32, actual: u32 },
    #[error("cross-pipeline binding conflict on {binding:?} between {a:?} and {b:?}")]
    CrossPipelineBindingConflict {
        binding: String,
        a: String,
        b: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("corrupt pipeline file {path:?}: {reason}")]
    CorruptFile { path: String, reason: String },
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Summary row returned by [`PipelineRegistry::list`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineSummary {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub enabled: bool,
}

/// In-memory registry of pipeline definitions with versioning and conflict
/// rules.
#[derive(Debug, Default)]
pub struct PipelineRegistry {
    pipelines: BTreeMap<String, PipelineDefinition>,
    enabled: BTreeMap<String, bool>,
}

impl PipelineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate and insert a new pipeline. Rejects duplicate ids and any
    /// enabled-binding conflict with pipelines already present.
    pub fn insert(&mut self, mut def: PipelineDefinition) -> Result<(), PipelineError> {
        def.validate()?;
        if self.pipelines.contains_key(&def.id) {
            return Err(PipelineError::DuplicatePipelineId(def.id.clone()));
        }
        let others: Vec<PipelineDefinition> = self.pipelines.values().cloned().collect();
        check_bindings_across_pair(&def, &others)?;
        if def.created_unix_ms == 0 {
            def.created_unix_ms = now_unix_ms();
        }
        if def.updated_unix_ms == 0 {
            def.updated_unix_ms = def.created_unix_ms;
        }
        self.enabled.entry(def.id.clone()).or_insert(true);
        self.pipelines.insert(def.id.clone(), def);
        Ok(())
    }

    /// Replace an existing pipeline. Enforces the version-bump-on-save rule:
    /// the new version must be exactly current + 1.
    pub fn update(&mut self, mut def: PipelineDefinition) -> Result<(), PipelineError> {
        def.validate()?;
        let current = self
            .pipelines
            .get(&def.id)
            .ok_or_else(|| PipelineError::UnknownPipelineId(def.id.clone()))?;
        let expected = current.version + 1;
        if def.version != expected {
            return Err(PipelineError::VersionBumpRejected {
                expected,
                actual: def.version,
            });
        }
        let others: Vec<PipelineDefinition> = self
            .pipelines
            .values()
            .filter(|p| p.id != def.id)
            .cloned()
            .collect();
        check_bindings_across_pair(&def, &others)?;
        def.created_unix_ms = current.created_unix_ms;
        def.updated_unix_ms = now_unix_ms();
        self.pipelines.insert(def.id.clone(), def);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&PipelineDefinition> {
        self.pipelines.get(id)
    }

    /// Cloned view of every stored definition, ordered by id (additive P7
    /// helper for versioned export without exposing internal maps).
    pub fn definitions(&self) -> Vec<PipelineDefinition> {
        self.pipelines.values().cloned().collect()
    }

    pub fn list(&self) -> Vec<PipelineSummary> {
        self.pipelines
            .values()
            .map(|def| PipelineSummary {
                id: def.id.clone(),
                name: def.name.clone(),
                version: def.version,
                enabled: self.is_enabled(&def.id),
            })
            .collect()
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled.get(id).copied().unwrap_or(true)
    }

    /// Toggle a pipeline's user-facing enabled flag (does not touch node
    /// `enabled` flags).
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), PipelineError> {
        if !self.pipelines.contains_key(id) {
            return Err(PipelineError::UnknownPipelineId(id.to_string()));
        }
        self.enabled.insert(id.to_string(), enabled);
        Ok(())
    }

    /// Cross-pipeline conflict check over a batch of definitions: the same
    /// normalized ENABLED binding must not appear in two different pipelines.
    pub fn validate_bindings_across(defs: &[PipelineDefinition]) -> Result<(), PipelineError> {
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        for def in defs {
            for key in def.enabled_binding_keys()? {
                if let Some(first) = seen.get(&key) {
                    if first != &def.id {
                        return Err(PipelineError::CrossPipelineBindingConflict {
                            binding: key,
                            a: first.clone(),
                            b: def.id.clone(),
                        });
                    }
                } else {
                    seen.insert(key, def.id.clone());
                }
            }
        }
        Ok(())
    }

    /// Persist one pretty JSON file per pipeline plus the registry index.
    pub fn save_to_dir(&self, dir: &Path) -> Result<(), PipelineError> {
        fs::create_dir_all(dir)?;
        for def in self.pipelines.values() {
            let path = dir.join(format!("{}.json", def.id));
            let json = serde_json::to_string_pretty(def)?;
            fs::write(path, format!("{json}\n"))?;
        }
        let index = RegistryIndex {
            schema_version: PIPELINE_SCHEMA_VERSION,
            pipelines: self
                .list()
                .into_iter()
                .map(|s| RegistryIndexEntry {
                    id: s.id,
                    version: s.version,
                    enabled: s.enabled,
                })
                .collect(),
        };
        let index_path = dir.join(REGISTRY_INDEX_FILE);
        let json = serde_json::to_string_pretty(&index)?;
        fs::write(index_path, format!("{json}\n"))?;
        Ok(())
    }

    /// Load a store previously written by [`save_to_dir`] (or hand-authored).
    ///
    /// Never panics: corrupt/invalid files are skipped and reported in the
    /// returned [`LoadReport`]. Files with `schema_version` 0 or missing
    /// fields are migrated via serde defaults and bumped to schema 1.
    pub fn load_from_dir(dir: &Path) -> Result<LoadReport, PipelineError> {
        if !dir.is_dir() {
            return Err(PipelineError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("pipeline store directory not found: {}", dir.display()),
            )));
        }
        let mut registry = PipelineRegistry::new();
        let mut skipped: Vec<SkippedFile> = Vec::new();
        let mut migrated: Vec<String> = Vec::new();
        let mut entries: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                entries.push(path);
            }
        }
        entries.sort();
        for path in entries {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if name == "registry" {
                continue; // index handled below
            }
            match load_pipeline_file(&path) {
                Ok((def, was_migrated)) => {
                    if was_migrated {
                        migrated.push(name);
                    }
                    if let Err(err) = registry.insert(def) {
                        skipped.push(SkippedFile {
                            path: path.display().to_string(),
                            reason: err.to_string(),
                        });
                    }
                }
                Err(err) => skipped.push(SkippedFile {
                    path: path.display().to_string(),
                    reason: err.to_string(),
                }),
            }
        }
        // Best-effort index: restores enabled flags; a corrupt index is
        // reported but does not fail the load.
        let index_path = dir.join(REGISTRY_INDEX_FILE);
        if index_path.exists() {
            match fs::read_to_string(&index_path)
                .map_err(PipelineError::from)
                .and_then(|s| Ok(serde_json::from_str::<RegistryIndex>(&s)?))
            {
                Ok(index) => {
                    for entry in index.pipelines {
                        if registry.pipelines.contains_key(&entry.id) {
                            registry.enabled.insert(entry.id, entry.enabled);
                        }
                    }
                }
                Err(err) => skipped.push(SkippedFile {
                    path: index_path.display().to_string(),
                    reason: err.to_string(),
                }),
            }
        }
        Ok(LoadReport {
            registry,
            skipped,
            migrated,
        })
    }
}

fn check_bindings_across_pair(
    def: &PipelineDefinition,
    others: &[PipelineDefinition],
) -> Result<(), PipelineError> {
    let keys = def.enabled_binding_keys()?;
    for other in others {
        for key in other.enabled_binding_keys()? {
            if keys.contains(&key) {
                return Err(PipelineError::CrossPipelineBindingConflict {
                    binding: key,
                    a: def.id.clone(),
                    b: other.id.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Load and migrate one `<id>.json` file. Returns the definition plus whether
/// a schema migration (0 -> 1) was applied.
fn load_pipeline_file(path: &Path) -> Result<(PipelineDefinition, bool), PipelineError> {
    let text = fs::read_to_string(path)?;
    let mut value: serde_json::Value = serde_json::from_str(&text)?;
    let mut was_migrated = false;
    let schema = value.get("schema_version");
    match schema {
        None | Some(serde_json::Value::Null) => {
            value["schema_version"] = serde_json::json!(PIPELINE_SCHEMA_VERSION);
            was_migrated = true;
        }
        Some(serde_json::Value::Number(n)) => {
            let found = n.as_u64().unwrap_or(u64::MAX) as u32;
            if found == 0 {
                value["schema_version"] = serde_json::json!(PIPELINE_SCHEMA_VERSION);
                was_migrated = true;
            } else if found > PIPELINE_SCHEMA_VERSION {
                return Err(PipelineError::UnsupportedSchemaVersion {
                    found,
                    supported: PIPELINE_SCHEMA_VERSION,
                });
            }
        }
        Some(_) => {
            return Err(PipelineError::CorruptFile {
                path: path.display().to_string(),
                reason: "schema_version is not a number".to_string(),
            });
        }
    }
    let def: PipelineDefinition = serde_json::from_value(value)?;
    Ok((def, was_migrated))
}

/// Result of [`PipelineRegistry::load_from_dir`].
#[derive(Debug)]
pub struct LoadReport {
    pub registry: PipelineRegistry,
    /// Files skipped because they were unreadable, unparseable, invalid, or
    /// newer than the supported schema.
    pub skipped: Vec<SkippedFile>,
    /// File stems migrated from schema 0/missing to schema 1.
    pub migrated: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RegistryIndex {
    schema_version: u32,
    pipelines: Vec<RegistryIndexEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RegistryIndexEntry {
    id: String,
    version: u32,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
