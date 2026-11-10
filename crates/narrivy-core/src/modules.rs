//! Typed module identity/health registry (P1 module contract).
//!
//! Internal-evolving: activation, capture, audio processing, ASR/compute,
//! context and delivery implementations register descriptors here so they can
//! be replaced without domain coupling. No Rust dylib ABI is published.

use crate::CoreError;
use narrivy_policy::{ModulePermission, ModulePermissions};

/// Capability slots a module may implement. Mirrors the catalog vocabulary
/// (spec/catalogs/modules.json); unknown slots are rejected at registration.
pub const KNOWN_SLOTS: &[&str] = &[
    "activation.global-hotkey",
    "audio.capture",
    "audio.normalize",
    "audio.denoise",
    "speech.vad",
    "speech.endpoint",
    "speech.asr",
    "text.punctuation",
    "text.itn",
    "text.semantic-guard",
    "text.llm-postprocess",
    "delivery.direct",
    "delivery.clipboard",
    "delivery.inbox",
    "context.provider",
    "quality.qualifier",
];

/// Typed descriptor of one registered module implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDescriptor {
    /// Stable module id, e.g. `asr-mock` or a capture adapter id.
    pub id: String,
    /// Capability slot this implementation fills (one module fills one slot;
    /// register the same implementation under multiple ids for multi-slot).
    pub slot: String,
    /// Module version (semver-ish string; pinned at registration).
    pub version: String,
    /// What the module provides beyond the slot baseline (feature names).
    pub provides: Vec<String>,
    /// Last observed health.
    pub healthy: bool,
    /// Content-free health detail (never transcript/audio content).
    pub detail: String,
    /// Manifest-declared capabilities and the subset granted by the host.
    pub permissions: ModulePermissions,
}

#[derive(Default)]
pub struct ModuleRegistry {
    modules: Vec<ModuleDescriptor>,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, descriptor: ModuleDescriptor) -> Result<(), CoreError> {
        if !KNOWN_SLOTS.contains(&descriptor.slot.as_str()) {
            return Err(CoreError::Module(format!(
                "unknown slot {:?}; known: {KNOWN_SLOTS:?}",
                descriptor.slot
            )));
        }
        if descriptor.id.is_empty() || descriptor.version.is_empty() {
            return Err(CoreError::Module("id and version are required".to_string()));
        }
        if self.modules.iter().any(|m| m.id == descriptor.id) {
            return Err(CoreError::Module(format!(
                "duplicate module id {}",
                descriptor.id
            )));
        }
        self.modules.push(descriptor);
        Ok(())
    }

    pub fn deregister(&mut self, id: &str) -> Result<(), CoreError> {
        let before = self.modules.len();
        self.modules.retain(|m| m.id != id);
        if self.modules.len() == before {
            return Err(CoreError::Module(format!("unknown module id {id}")));
        }
        Ok(())
    }

    pub fn set_health(
        &mut self,
        id: &str,
        healthy: bool,
        detail: impl Into<String>,
    ) -> Result<(), CoreError> {
        let module = self
            .modules
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| CoreError::Module(format!("unknown module id {id}")))?;
        module.healthy = healthy;
        module.detail = detail.into();
        Ok(())
    }

    /// Replace the host-approved subset. Undeclared capabilities can never be
    /// granted through this boundary.
    pub fn set_permission_grants(
        &mut self,
        id: &str,
        granted: impl IntoIterator<Item = ModulePermission>,
    ) -> Result<(), CoreError> {
        let module = self
            .modules
            .iter_mut()
            .find(|module| module.id == id)
            .ok_or_else(|| CoreError::Module(format!("unknown module id {id}")))?;
        module
            .permissions
            .replace_grants(granted)
            .map_err(|error| CoreError::Module(error.to_string()))
    }

    /// Check one service-boundary operation before the module is invoked.
    pub fn authorize(&self, id: &str, permission: ModulePermission) -> Result<(), CoreError> {
        let module = self
            .modules
            .iter()
            .find(|module| module.id == id)
            .ok_or_else(|| CoreError::Module(format!("unknown module id {id}")))?;
        module
            .permissions
            .authorize(permission)
            .map_err(|error| CoreError::Module(error.to_string()))
    }

    /// Healthy modules first, stable order otherwise.
    #[must_use]
    pub fn by_slot(&self, slot: &str) -> Vec<ModuleDescriptor> {
        let mut out: Vec<ModuleDescriptor> = self
            .modules
            .iter()
            .filter(|m| m.slot == slot)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.healthy.cmp(&a.healthy).then(a.id.cmp(&b.id)));
        out
    }

    pub fn get(&self, id: &str) -> Option<&ModuleDescriptor> {
        self.modules.iter().find(|m| m.id == id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn desc(id: &str, slot: &str, healthy: bool) -> ModuleDescriptor {
        ModuleDescriptor {
            id: id.to_string(),
            slot: slot.to_string(),
            version: "0.1.0".to_string(),
            provides: vec![],
            healthy,
            detail: String::new(),
            permissions: ModulePermissions::default(),
        }
    }

    #[test]
    fn register_rejects_unknown_slot_and_duplicates() {
        let mut reg = ModuleRegistry::new();
        assert!(reg.register(desc("asr-mock", "speech.asr", true)).is_ok());
        assert!(reg.register(desc("bad", "warp.drive", true)).is_err());
        assert!(
            reg.register(desc("asr-mock", "audio.denoise", true))
                .is_err()
        );
    }

    #[test]
    fn by_slot_lists_healthy_first_with_stable_order() {
        let mut reg = ModuleRegistry::new();
        reg.register(desc("b-sick", "speech.asr", false)).unwrap();
        reg.register(desc("a-well", "speech.asr", true)).unwrap();
        reg.register(desc("c-well", "speech.asr", true)).unwrap();
        let ids: Vec<String> = reg
            .by_slot("speech.asr")
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, ["a-well", "c-well", "b-sick"]);
    }

    #[test]
    fn health_updates_are_truthful_and_typed() {
        let mut reg = ModuleRegistry::new();
        reg.register(desc("mic", "audio.capture", true)).unwrap();
        reg.set_health("mic", false, "device lost").unwrap();
        assert!(!reg.get("mic").unwrap().healthy);
        assert_eq!(reg.get("mic").unwrap().detail, "device lost");
        assert!(reg.set_health("nope", true, "").is_err());
        assert!(reg.deregister("nope").is_err());
        assert!(reg.deregister("mic").is_ok());
        assert!(reg.is_empty());
    }

    #[test]
    fn registry_enforces_declared_host_grants() {
        let mut reg = ModuleRegistry::new();
        let mut module = desc("remote-corrector", "text.llm-postprocess", true);
        module.permissions = ModulePermissions::new(
            [
                ModulePermission::TranscriptRead,
                ModulePermission::NetworkConnect,
            ],
            [ModulePermission::TranscriptRead],
        )
        .unwrap();
        reg.register(module).unwrap();

        assert!(
            reg.authorize("remote-corrector", ModulePermission::TranscriptRead)
                .is_ok()
        );
        assert!(
            reg.authorize("remote-corrector", ModulePermission::NetworkConnect)
                .is_err()
        );
        assert!(
            reg.authorize("remote-corrector", ModulePermission::AudioRead)
                .is_err()
        );

        reg.set_permission_grants(
            "remote-corrector",
            [
                ModulePermission::TranscriptRead,
                ModulePermission::NetworkConnect,
            ],
        )
        .unwrap();
        assert!(
            reg.authorize("remote-corrector", ModulePermission::NetworkConnect)
                .is_ok()
        );
    }
}
