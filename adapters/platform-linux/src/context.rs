//! Linux context provider — isolated, sensitivity-declared, PrivacyZone-aware.
//! Skeleton for P8; full implementation will query AT-SPI / portals where
//! capability allows, always gated by PolicyEngine + PrivacyZone override.

/// Fields a Linux context provider may request (mirrors history crate vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxContextField {
    AppName,
    WindowTitle,
    SelectedText,
    SurroundingText,
    Clipboard,
}

/// Snapshot skeleton.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxContextSnapshot {
    pub fields: Vec<(LinuxContextField, String)>,
}

pub trait LinuxContextProvider: Send + Sync {
    fn requested_fields(&self) -> Vec<LinuxContextField>;
    fn sensitivity(&self) -> &'static str;
    fn snapshot(&self) -> LinuxContextSnapshot;
}

/// No-op provider (satisfies vacuous case when broad providers deferred).
pub struct NoopLinuxContextProvider;

impl LinuxContextProvider for NoopLinuxContextProvider {
    fn requested_fields(&self) -> Vec<LinuxContextField> {
        Vec::new()
    }
    fn sensitivity(&self) -> &'static str {
        "none"
    }
    fn snapshot(&self) -> LinuxContextSnapshot {
        LinuxContextSnapshot { fields: Vec::new() }
    }
}

/// Linux context provider (skeleton).
pub struct LinuxContextProviderImpl {
    inner: NoopLinuxContextProvider,
}

impl LinuxContextProviderImpl {
    pub fn new() -> Self {
        Self {
            inner: NoopLinuxContextProvider,
        }
    }
}

impl Default for LinuxContextProviderImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxContextProvider for LinuxContextProviderImpl {
    fn requested_fields(&self) -> Vec<LinuxContextField> {
        self.inner.requested_fields()
    }
    fn sensitivity(&self) -> &'static str {
        "none"
    }
    fn snapshot(&self) -> LinuxContextSnapshot {
        self.inner.snapshot()
    }
}
