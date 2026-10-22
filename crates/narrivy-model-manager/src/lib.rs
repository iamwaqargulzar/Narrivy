//! narrivy-model-manager: validated model registry, verified byte acquisition,
//! explicit installed-model observations, residency tracking, and cleanup for
//! Narrivy's local ASR models.
//!
//! Requirements covered:
//!
//! * SEC-002 — registry checksum verification: acquisition returns no verified
//!   observation without a matching sha256 ([`ModelStore::acquire`]). Runtime
//!   activation still requires the platform's immutable descriptor binding.
//! * SEC-007 — trusted registry resolution with immutable revision/hash pins;
//!   arbitrary URLs never gain trust ([`registry::RegistryEntry::resolve_url`]).
//! * PERF-007 — Hot/Warm/Cold residency with versioned invalidation
//!   ([`residency::ResidencyTracker`]).
//! * PERF-008 — cleanup keeps source + selected + pinned + CPU fallback
//!   ([`cleanup::remove_unreferenced`]).
//!
//! This crate is portable: it decides which cache paths must be owner-only and
//! what must be provable about them, and it delegates every platform-specific
//! way of establishing that to an injected
//! [`CacheFilePolicy`](policy::CacheFilePolicy). A 0600 model cache is a
//! privacy control, so there is no default policy: composition must say how
//! the host protects a file it created.

pub mod catalog;
pub mod cleanup;
pub mod download;
pub mod error;
pub mod manager;
pub mod policy;
pub mod registry;
pub mod residency;
pub mod store;
pub mod transport;

pub use catalog::{
    CatalogModel, CatalogRuntime, ENGINE_OPENVINO, ENGINE_TRANSCRIBE_CPP, model_catalog,
    runtime_catalog,
};
pub use cleanup::{CleanupReport, remove_unreferenced};
pub use download::{DownloadRefusal, DownloadTarget, PackTarget};
pub use error::{ModelManagerError, Result};
pub use manager::{
    CacheBudget, ModelManager, PrefetchReport, RuntimePackSuggestion, enforce_cache_budget,
};
pub use policy::{CacheEntryKind, CacheFilePolicy, CacheOwner};
pub use registry::{
    BundledModelIdentity, BundledModelRegistry, Capabilities, ModelRegistry, REGISTRY_VERSION,
    RegistryEntry, RegistryFile, SourceKind, SourceSpec,
};
pub use residency::{InvalidationKey, RESIDENCY_VERSION, Residency, ResidencyTracker};
pub use store::{
    BundledInstalledModelBinding, DEFAULT_FETCH_TIMEOUT, InstallState, InstalledModelBinding,
    ModelStore,
};
pub use transport::{FileTransport, NoNetworkTransport, Transport};
