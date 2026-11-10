//! Deterministic privacy and processing policy.
//!
//! Layered model:
//!
//! - [`PrivacyPolicy`] — the original two-flag document-level gate. Its public
//!   API (`remote_allowed`, `local_network_allowed`, `evaluate`) is unchanged.
//! - [`PolicyDocument`] — versioned policy document with per-data-class
//!   allowances and deny-by-default semantics.
//! - [`PolicyEngine`] — evaluates a [`ProcessingBoundary`] (`gate`) and a
//!   (data class, boundary) pair (`gate_data`).
//! - [`GatedChannel`] — enforcement-order helper proving the P1 privacy
//!   contract: a pipeline stage MUST consult the policy gate BEFORE handing
//!   data to any boundary implementation (TEST-003 seed).

use narrivy_types::ProcessingBoundary;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::marker::PhantomData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
}

/// A capability that an internal module or future extension must declare and
/// receive from the host before use. Names intentionally match the extension
/// manifest schema so the internal Linux 1.0 boundary can become the public
/// extension boundary without translating security concepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ModulePermission {
    #[serde(rename = "audio.read")]
    AudioRead,
    #[serde(rename = "audio.process")]
    AudioProcess,
    #[serde(rename = "transcript.read")]
    TranscriptRead,
    #[serde(rename = "app_identity.read")]
    AppIdentityRead,
    #[serde(rename = "selection.read")]
    SelectionRead,
    #[serde(rename = "surrounding_text.read")]
    SurroundingTextRead,
    #[serde(rename = "clipboard.read")]
    ClipboardRead,
    #[serde(rename = "clipboard.write")]
    ClipboardWrite,
    #[serde(rename = "project_metadata.read")]
    ProjectMetadataRead,
    #[serde(rename = "history.read")]
    HistoryRead,
    #[serde(rename = "history.write")]
    HistoryWrite,
    #[serde(rename = "filesystem.read")]
    FilesystemRead,
    #[serde(rename = "filesystem.write")]
    FilesystemWrite,
    #[serde(rename = "network.connect")]
    NetworkConnect,
    #[serde(rename = "notification.show")]
    NotificationShow,
    #[serde(rename = "text_delivery.request")]
    TextDeliveryRequest,
    #[serde(rename = "model.use")]
    ModelUse,
    #[serde(rename = "dictionary.read")]
    DictionaryRead,
    #[serde(rename = "secrets.use")]
    SecretsUse,
    #[serde(rename = "playback_reference.read")]
    PlaybackReferenceRead,
    #[serde(rename = "speaker_profile.read")]
    SpeakerProfileRead,
    #[serde(rename = "provider.invoke")]
    ProviderInvoke,
    #[serde(rename = "pipeline.read")]
    PipelineRead,
    #[serde(rename = "pipeline.write")]
    PipelineWrite,
}

/// Manifest declarations and the host-approved subset for one module.
///
/// Empty grants are the default, including for `network.connect`. A grant is
/// invalid unless the module declared it first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModulePermissions {
    declared: BTreeSet<ModulePermission>,
    granted: BTreeSet<ModulePermission>,
}

impl ModulePermissions {
    pub fn new(
        declared: impl IntoIterator<Item = ModulePermission>,
        granted: impl IntoIterator<Item = ModulePermission>,
    ) -> Result<Self, PolicyError> {
        let declared = declared.into_iter().collect::<BTreeSet<_>>();
        let granted = granted.into_iter().collect::<BTreeSet<_>>();
        if let Some(permission) = granted.difference(&declared).next().copied() {
            return Err(PolicyError::PermissionNotDeclared { permission });
        }
        Ok(Self { declared, granted })
    }

    pub fn declared(&self) -> &BTreeSet<ModulePermission> {
        &self.declared
    }

    pub fn granted(&self) -> &BTreeSet<ModulePermission> {
        &self.granted
    }

    pub fn replace_grants(
        &mut self,
        granted: impl IntoIterator<Item = ModulePermission>,
    ) -> Result<(), PolicyError> {
        let granted = granted.into_iter().collect::<BTreeSet<_>>();
        if let Some(permission) = granted.difference(&self.declared).next().copied() {
            return Err(PolicyError::PermissionNotDeclared { permission });
        }
        self.granted = granted;
        Ok(())
    }

    /// Enforce declaration and host grant before a service operation begins.
    pub fn authorize(&self, permission: ModulePermission) -> Result<(), PolicyError> {
        if !self.declared.contains(&permission) {
            return Err(PolicyError::PermissionNotDeclared { permission });
        }
        if !self.granted.contains(&permission) {
            return Err(PolicyError::PermissionNotGranted { permission });
        }
        Ok(())
    }
}

/// Original document-level privacy gate. Public API is frozen for
/// compatibility with the Phase-0 scaffold consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyPolicy {
    pub remote_allowed: bool,
    pub local_network_allowed: bool,
}

impl PrivacyPolicy {
    pub fn evaluate(self, boundary: ProcessingBoundary) -> PolicyDecision {
        match boundary {
            ProcessingBoundary::Local => PolicyDecision::Allow,
            ProcessingBoundary::LocalNetwork if self.local_network_allowed => PolicyDecision::Allow,
            ProcessingBoundary::Remote if self.remote_allowed => PolicyDecision::Allow,
            ProcessingBoundary::LocalNetwork | ProcessingBoundary::Remote => PolicyDecision::Deny,
        }
    }
}

/// Classification of the payload crossing a boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    AudioRaw,
    TranscriptRaw,
    TranscriptCorrected,
    Diagnostics,
}

/// Explicit allowance (or denial) for one data class at one boundary.
///
/// An explicitly listed pair overrides the document defaults in either
/// direction: `allowed: false` denies even when the document level allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataClassAllowance {
    pub class: DataClass,
    pub boundary: ProcessingBoundary,
    pub allowed: bool,
}

fn schema_version_default() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

/// Versioned policy document (serde-friendly, forward-compatible defaults).
///
/// Defaults encode deny-unless-allowed: `remote_allowed`,
/// `local_network_allowed`, and every data-class allowance start `false`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDocument {
    #[serde(default = "schema_version_default")]
    pub schema_version: u32,
    /// When true (the default), `gate_data(_, Remote)` denies unless an
    /// explicit data-class allowance permits the specific pair. When false,
    /// remote falls back to document-level `remote_allowed`.
    #[serde(default = "default_true")]
    pub default_deny_remote: bool,
    #[serde(default)]
    pub remote_allowed: bool,
    #[serde(default)]
    pub local_network_allowed: bool,
    #[serde(default)]
    pub data_class_allowances: Vec<DataClassAllowance>,
}

impl Default for PolicyDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            default_deny_remote: true,
            remote_allowed: false,
            local_network_allowed: false,
            data_class_allowances: Vec::new(),
        }
    }
}

/// Typed policy failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("data class {class:?} is denied at processing boundary {boundary:?}")]
    Denied {
        class: DataClass,
        boundary: ProcessingBoundary,
    },
    #[error("module permission {permission:?} was not declared")]
    PermissionNotDeclared { permission: ModulePermission },
    #[error("module permission {permission:?} was not granted by the host")]
    PermissionNotGranted { permission: ModulePermission },
}

/// Portable policy boundary consumed by session orchestration. Implementors
/// must decide before the payload is handed to the receiving component.
pub trait ProcessingPolicy: Send + Sync {
    fn allow_data(&self, class: DataClass, boundary: ProcessingBoundary)
    -> Result<(), PolicyError>;
}

/// Evaluates privacy decisions from a [`PolicyDocument`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEngine {
    document: PolicyDocument,
}

impl PolicyEngine {
    pub fn from_document(document: PolicyDocument) -> Self {
        Self { document }
    }

    /// The backing document (read-only view).
    pub fn document(&self) -> &PolicyDocument {
        &self.document
    }

    fn privacy(&self) -> PrivacyPolicy {
        PrivacyPolicy {
            remote_allowed: self.document.remote_allowed,
            local_network_allowed: self.document.local_network_allowed,
        }
    }

    /// Document-level boundary gate. Identical semantics to
    /// [`PrivacyPolicy::evaluate`] so existing callers stay compatible.
    pub fn gate(&self, boundary: ProcessingBoundary) -> PolicyDecision {
        self.privacy().evaluate(boundary)
    }

    /// Data-aware gate.
    ///
    /// Resolution order:
    /// 1. `Local` always allows.
    /// 2. An explicit `(class, boundary)` allowance decides (either way).
    /// 3. Otherwise `LocalNetwork` follows the document-level flag.
    /// 4. Otherwise `Remote` denies by default; with
    ///    `default_deny_remote = false` it falls back to the document-level
    ///    flag.
    pub fn gate_data(&self, class: DataClass, boundary: ProcessingBoundary) -> PolicyDecision {
        if boundary == ProcessingBoundary::Local {
            return PolicyDecision::Allow;
        }
        if let Some(explicit) = self
            .document
            .data_class_allowances
            .iter()
            .find(|a| a.class == class && a.boundary == boundary)
        {
            return if explicit.allowed {
                PolicyDecision::Allow
            } else {
                PolicyDecision::Deny
            };
        }
        match boundary {
            ProcessingBoundary::LocalNetwork => self.gate(boundary),
            ProcessingBoundary::Remote if !self.document.default_deny_remote => self.gate(boundary),
            _ => PolicyDecision::Deny,
        }
    }
}

impl ProcessingPolicy for PolicyEngine {
    fn allow_data(
        &self,
        class: DataClass,
        boundary: ProcessingBoundary,
    ) -> Result<(), PolicyError> {
        match self.gate_data(class, boundary) {
            PolicyDecision::Allow => Ok(()),
            PolicyDecision::Deny => Err(PolicyError::Denied { class, boundary }),
        }
    }
}

/// Target that receives data at a fixed processing boundary.
///
/// Implementations wrap one concrete boundary (local stage, network client,
/// ...). [`GatedChannel`] sits in front of it to enforce gating order.
pub trait BoundarySink<T> {
    /// The boundary this sink hands data to.
    fn boundary(&self) -> ProcessingBoundary;
    /// Receive the value. Must not be called before the gate allows.
    fn push(&mut self, value: T);
}

/// Enforcement-order channel: consults the policy engine FIRST and only then
/// hands the value to the inner sink.
///
/// This type is the TEST-003 privacy-enforcement seed: any future boundary
/// implementation is wrapped here (or proven equivalent) so a denied boundary
/// can never observe the data.
#[derive(Debug)]
pub struct GatedChannel<'e, T, S: BoundarySink<T>> {
    sink: S,
    engine: &'e PolicyEngine,
    class: DataClass,
    _value: PhantomData<T>,
}

impl<'e, T, S: BoundarySink<T>> GatedChannel<'e, T, S> {
    pub fn new(sink: S, engine: &'e PolicyEngine, class: DataClass) -> Self {
        Self {
            sink,
            engine,
            class,
            _value: PhantomData,
        }
    }

    /// Gate, then deliver. Denied payloads never reach the inner sink.
    pub fn send(&mut self, value: T) -> Result<(), PolicyError> {
        let boundary = self.sink.boundary();
        match self.engine.gate_data(self.class, boundary) {
            PolicyDecision::Allow => {
                self.sink.push(value);
                Ok(())
            }
            PolicyDecision::Deny => Err(PolicyError::Denied {
                class: self.class,
                boundary,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn module_permissions_require_declaration_and_host_grant() -> TestResult {
        let mut permissions = ModulePermissions::new(
            [
                ModulePermission::TranscriptRead,
                ModulePermission::NetworkConnect,
            ],
            [ModulePermission::TranscriptRead],
        )?;

        assert_eq!(
            permissions.authorize(ModulePermission::TranscriptRead),
            Ok(())
        );
        assert_eq!(
            permissions.authorize(ModulePermission::NetworkConnect),
            Err(PolicyError::PermissionNotGranted {
                permission: ModulePermission::NetworkConnect,
            })
        );
        assert_eq!(
            permissions.authorize(ModulePermission::AudioRead),
            Err(PolicyError::PermissionNotDeclared {
                permission: ModulePermission::AudioRead,
            })
        );

        permissions.replace_grants([
            ModulePermission::TranscriptRead,
            ModulePermission::NetworkConnect,
        ])?;
        assert_eq!(
            permissions.authorize(ModulePermission::NetworkConnect),
            Ok(())
        );
        Ok(())
    }

    #[test]
    fn module_network_access_is_denied_by_default() -> TestResult {
        let permissions = ModulePermissions::new([ModulePermission::NetworkConnect], [])?;
        assert_eq!(
            permissions.authorize(ModulePermission::NetworkConnect),
            Err(PolicyError::PermissionNotGranted {
                permission: ModulePermission::NetworkConnect,
            })
        );
        assert!(permissions.granted().is_empty());
        Ok(())
    }

    #[test]
    fn host_cannot_grant_an_undeclared_permission() {
        assert_eq!(
            ModulePermissions::new([], [ModulePermission::NetworkConnect]),
            Err(PolicyError::PermissionNotDeclared {
                permission: ModulePermission::NetworkConnect,
            })
        );
    }

    #[test]
    fn local_is_always_allowed() {
        let p = PrivacyPolicy {
            remote_allowed: false,
            local_network_allowed: false,
        };
        assert_eq!(p.evaluate(ProcessingBoundary::Local), PolicyDecision::Allow);
    }

    #[test]
    fn remote_respects_policy() {
        let p = PrivacyPolicy {
            remote_allowed: false,
            local_network_allowed: true,
        };
        assert_eq!(p.evaluate(ProcessingBoundary::Remote), PolicyDecision::Deny);
    }

    #[test]
    fn remote_denied_by_default_even_when_stage_requests_it() {
        // A pipeline stage requesting Remote cannot override the default-deny
        // document: the engine answers Deny regardless of the requester.
        let engine = PolicyEngine::from_document(PolicyDocument::default());
        assert_eq!(
            engine.gate_data(DataClass::TranscriptRaw, ProcessingBoundary::Remote),
            PolicyDecision::Deny
        );
        assert_eq!(
            engine.gate_data(DataClass::AudioRaw, ProcessingBoundary::Remote),
            PolicyDecision::Deny
        );
        assert_eq!(
            engine.gate(ProcessingBoundary::Remote),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn opt_in_document_level_remote_is_allowed() {
        let doc = PolicyDocument {
            remote_allowed: true,
            ..PolicyDocument::default()
        };
        let engine = PolicyEngine::from_document(doc);
        assert_eq!(
            engine.gate(ProcessingBoundary::Remote),
            PolicyDecision::Allow
        );
        assert_eq!(
            engine.gate(ProcessingBoundary::Local),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn local_is_always_allowed_for_every_data_class() {
        let engine = PolicyEngine::from_document(PolicyDocument::default());
        for class in [
            DataClass::AudioRaw,
            DataClass::TranscriptRaw,
            DataClass::TranscriptCorrected,
            DataClass::Diagnostics,
        ] {
            assert_eq!(
                engine.gate_data(class, ProcessingBoundary::Local),
                PolicyDecision::Allow
            );
        }
    }

    #[test]
    fn data_class_default_deny_until_explicitly_listed() {
        let doc = PolicyDocument {
            data_class_allowances: vec![DataClassAllowance {
                class: DataClass::TranscriptCorrected,
                boundary: ProcessingBoundary::Remote,
                allowed: true,
            }],
            ..PolicyDocument::default()
        };
        let engine = PolicyEngine::from_document(doc);
        assert_eq!(
            engine.gate_data(DataClass::TranscriptCorrected, ProcessingBoundary::Remote),
            PolicyDecision::Allow
        );
        // Sibling classes stay denied: the allowance is per data class.
        assert_eq!(
            engine.gate_data(DataClass::TranscriptRaw, ProcessingBoundary::Remote),
            PolicyDecision::Deny
        );
        assert_eq!(
            engine.gate_data(DataClass::Diagnostics, ProcessingBoundary::Remote),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn explicit_denial_overrides_document_level_opt_in() {
        let doc = PolicyDocument {
            default_deny_remote: false,
            remote_allowed: true,
            data_class_allowances: vec![DataClassAllowance {
                class: DataClass::AudioRaw,
                boundary: ProcessingBoundary::Remote,
                allowed: false,
            }],
            ..PolicyDocument::default()
        };
        let engine = PolicyEngine::from_document(doc);
        assert_eq!(
            engine.gate_data(DataClass::AudioRaw, ProcessingBoundary::Remote),
            PolicyDecision::Deny
        );
        assert_eq!(
            engine.gate_data(DataClass::TranscriptRaw, ProcessingBoundary::Remote),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn default_deny_remote_false_falls_back_to_document_flag() {
        let doc = PolicyDocument {
            default_deny_remote: false,
            remote_allowed: false,
            ..PolicyDocument::default()
        };
        let engine = PolicyEngine::from_document(doc);
        assert_eq!(
            engine.gate_data(DataClass::Diagnostics, ProcessingBoundary::Remote),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn local_network_follows_document_flag() {
        let engine = PolicyEngine::from_document(PolicyDocument::default());
        assert_eq!(
            engine.gate_data(DataClass::Diagnostics, ProcessingBoundary::LocalNetwork),
            PolicyDecision::Deny
        );
        let doc = PolicyDocument {
            local_network_allowed: true,
            ..PolicyDocument::default()
        };
        let engine = PolicyEngine::from_document(doc);
        assert_eq!(
            engine.gate_data(DataClass::Diagnostics, ProcessingBoundary::LocalNetwork),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn policy_document_defaults_are_deny_all_and_serialize_with_defaults() -> TestResult {
        let doc = PolicyDocument::default();
        let json = serde_json::to_string(&doc)?;
        let round: PolicyDocument = serde_json::from_str(&json)?;
        assert_eq!(round, PolicyDocument::default());
        // An empty JSON object deserializes to the deny-all document.
        let empty: PolicyDocument = serde_json::from_str("{}")?;
        assert_eq!(empty.schema_version, 1);
        assert!(empty.default_deny_remote);
        assert!(!empty.remote_allowed);
        assert!(!empty.local_network_allowed);
        assert!(empty.data_class_allowances.is_empty());
        Ok(())
    }

    #[derive(Clone)]
    struct MockSink {
        boundary: ProcessingBoundary,
        pushed: Rc<RefCell<Vec<String>>>,
    }

    impl BoundarySink<String> for MockSink {
        fn boundary(&self) -> ProcessingBoundary {
            self.boundary
        }

        fn push(&mut self, value: String) {
            self.pushed.borrow_mut().push(value);
        }
    }

    #[test]
    fn gated_channel_consults_gate_before_inner_sink() -> TestResult {
        // Enforcement order contract: the gate runs FIRST. Proof: a denied
        // send returns Err(Denied{class, boundary}) and the inner boundary
        // implementation remains untouched (nothing recorded in its log).
        let engine = PolicyEngine::from_document(PolicyDocument::default());
        let log = Rc::new(RefCell::new(Vec::new()));
        let remote_sink = MockSink {
            boundary: ProcessingBoundary::Remote,
            pushed: Rc::clone(&log),
        };
        let mut channel: GatedChannel<String, MockSink> =
            GatedChannel::new(remote_sink, &engine, DataClass::TranscriptRaw);

        let err = match channel.send(String::from("transcript body")) {
            Err(error) => error,
            Ok(()) => return Err("remote policy unexpectedly allowed transcript".into()),
        };
        assert_eq!(
            err,
            PolicyError::Denied {
                class: DataClass::TranscriptRaw,
                boundary: ProcessingBoundary::Remote,
            }
        );
        assert!(
            log.borrow().is_empty(),
            "inner sink observed data before the gate denied it"
        );

        // Allowed flow: Local always passes and the inner sink receives the
        // value only after the gate allowed it.
        let local_log = Rc::new(RefCell::new(Vec::new()));
        let local_sink = MockSink {
            boundary: ProcessingBoundary::Local,
            pushed: Rc::clone(&local_log),
        };
        let mut local_channel: GatedChannel<String, MockSink> =
            GatedChannel::new(local_sink, &engine, DataClass::TranscriptRaw);
        local_channel.send(String::from("kept local"))?;
        assert_eq!(*local_log.borrow(), vec![String::from("kept local")]);
        Ok(())
    }

    #[test]
    fn gated_channel_delivers_after_explicit_opt_in() -> TestResult {
        let doc = PolicyDocument {
            data_class_allowances: vec![DataClassAllowance {
                class: DataClass::TranscriptCorrected,
                boundary: ProcessingBoundary::Remote,
                allowed: true,
            }],
            ..PolicyDocument::default()
        };
        let engine = PolicyEngine::from_document(doc);
        let log = Rc::new(RefCell::new(Vec::new()));
        let sink = MockSink {
            boundary: ProcessingBoundary::Remote,
            pushed: Rc::clone(&log),
        };
        let mut channel: GatedChannel<String, MockSink> =
            GatedChannel::new(sink, &engine, DataClass::TranscriptCorrected);
        channel.send(String::from("polished"))?;
        assert_eq!(*log.borrow(), vec![String::from("polished")]);
        Ok(())
    }
}
