//! Windows stand-in for the Linux-only platform features the daemon composes
//! (PORT-007): the GlobalShortcuts and RemoteDesktop portals, the trusted
//! `pw-record` and `wtype` helpers, PipeWire echo cancellation, desktop
//! shortcut files, the evdev hold key, the Secret Service and the sysfs
//! accelerator counters.
//!
//! Each item keeps the shape of its `narrivy_platform_linux` counterpart so
//! the daemon modules compile unchanged, and every probe answers with the
//! typed "unavailable" outcome the Linux adapter itself uses when a feature
//! is missing. Clients therefore see these features as unavailable
//! capabilities; nothing here pretends to work. Windows features that do
//! exist (WASAPI capture, `SendInput` delivery) come from
//! `narrivy-platform-windows`, not from here; the exception is
//! [`echo_cancel`], which reads the WASAPI endpoints and runs "Ignore this
//! computer's sound" in-process.

#![forbid(unsafe_code)]

/// Short helpers run with a deadline. The Linux module is portable `std`
/// code, so it is compiled here unchanged rather than copied.
#[path = "../../../../adapters/platform-linux/src/bounded_command.rs"]
pub mod bounded_command;

/// The trusted helper processes of the strict Linux lanes.
pub mod recovery {
    use std::path::PathBuf;

    /// Same shape as the Linux `pw-record` status; never available here.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PipeWireRecorderStatus {
        Available(PathBuf),
        Missing,
        NonExecutable,
        Untrusted,
        IdentityChanged,
        UnsupportedPlatform,
    }

    /// Same shape as the Linux `wtype` status; never available here.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WtypeSubmitterStatus {
        Available(PathBuf),
        Missing,
        NonExecutable,
        Untrusted,
        IdentityChanged,
        UnsupportedPlatform,
    }

    #[must_use]
    pub fn pipewire_recorder_status() -> PipeWireRecorderStatus {
        PipeWireRecorderStatus::UnsupportedPlatform
    }

    #[must_use]
    pub fn wtype_submitter_status() -> WtypeSubmitterStatus {
        WtypeSubmitterStatus::UnsupportedPlatform
    }
}

/// The GlobalShortcuts portal probes.
pub mod trigger {
    /// Same shape as the Linux probe result. There is no portal on Windows;
    /// the daemon reports [`PortalInterfaceProbe::RuntimeUnavailable`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PortalInterfaceProbe {
        Present { version: u32 },
        Absent,
        RuntimeUnavailable,
        TimedOut,
    }

    #[must_use]
    pub fn probe_portal_interface() -> PortalInterfaceProbe {
        PortalInterfaceProbe::RuntimeUnavailable
    }

    /// Truthful result of probing one route.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum RouteAvailability {
        Available { note: String },
        Unavailable { reason: String },
    }

    impl RouteAvailability {
        pub fn is_available(&self) -> bool {
            matches!(self, Self::Available { .. })
        }

        pub fn unavailable_reason(&self) -> Option<&str> {
            match self {
                Self::Available { .. } => None,
                Self::Unavailable { reason } => Some(reason),
            }
        }

        pub fn available_note(&self) -> Option<&str> {
            match self {
                Self::Available { note } => Some(note),
                Self::Unavailable { .. } => None,
            }
        }
    }

    #[must_use]
    pub fn probe_portal_globalshortcuts() -> RouteAvailability {
        RouteAvailability::Unavailable {
            reason: "no GlobalShortcuts portal on this platform".to_owned(),
        }
    }
}

/// The RemoteDesktop/EIS portal eligibility probe.
pub mod portal_eis {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PortalProbeFailure {
        RuntimeUnavailable,
        TimedOut,
        QueryFailed,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PortalEisEligibility {
        InterfaceAbsent,
        VersionTooOld { version: u32 },
        KeyboardUnavailable { version: u32 },
        ReadyToRequestPermission { version: u32 },
        ProbeFailed(PortalProbeFailure),
    }

    /// There is no session bus to ask.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct SystemRemoteDesktopProbe {
        _private: (),
    }

    #[must_use]
    pub fn classify_portal_eis(_probe: &SystemRemoteDesktopProbe) -> PortalEisEligibility {
        PortalEisEligibility::ProbeFailed(PortalProbeFailure::RuntimeUnavailable)
    }
}

/// "Ignore this computer's sound" (PipeWire's WebRTC echo canceller on
/// Linux), run in-process on Windows; the device lists work (WASAPI).
///
/// No PipeWire module to load: the canceller is a processing setup that the
/// dictation capture applies in-process
/// (`narrivy_platform_windows::os::ProcessedCapture`: the microphone plus a
/// loopback of the speakers through WebRTC's echo canceller, noise
/// suppression and gain). [`echo_cancel::SOURCE`] is the capture target that
/// asks for it; [`echo_cancel::capture_for`] turns a target into the capture
/// to open.
pub mod echo_cancel {

    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use narrivy_platform_api::PlatformError;
    use narrivy_platform_windows::os::{
        DeviceSelection, MicrophoneCapture, ProcessedCapture, WasapiCapture,
    };
    use narrivy_platform_windows::voice_processing::{NoiseLevel, Processing};

    pub const SOURCE: &str = "narrivy.aec.source";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Noise {
        Off,
        Standard,
        Strong,
    }

    /// The running canceller's setup: the microphone and the processing.
    #[derive(Debug, Clone)]
    struct Active {
        id: u64,
        microphone: Option<String>,
        processing: Processing,
    }

    static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    /// The canceller as `daemon::echo` holds it. It opens nothing until a
    /// dictation records, so it starts at once; dropping it turns it off.
    pub struct EchoCancel {
        id: u64,
    }

    impl EchoCancel {
        pub fn start(
            dir: &Path,
            noise: Noise,
            microphone: Option<&str>,
        ) -> Result<Self, &'static str> {
            Self::start_with_gain(dir, noise, false, microphone)
        }

        pub fn start_with_gain(
            _dir: &Path,
            noise: Noise,
            gain: bool,
            microphone: Option<&str>,
        ) -> Result<Self, &'static str> {
            if microphone.is_some_and(|name| !valid_node_name(name)) {
                return Err("echo_cancel_microphone_invalid");
            }
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let processing = Processing {
                noise: match noise {
                    Noise::Off => NoiseLevel::Off,
                    Noise::Standard => NoiseLevel::Standard,
                    Noise::Strong => NoiseLevel::Strong,
                },
                auto_gain: gain,
            };
            let mut active = ACTIVE.lock().map_err(|_| "echo_cancel_unavailable")?;
            *active = Some(Active {
                id,
                microphone: microphone.map(str::to_owned),
                processing,
            });
            Ok(Self { id })
        }

        /// Running until dropped or replaced by a newer start.
        pub fn is_running(&mut self) -> bool {
            ACTIVE
                .lock()
                .is_ok_and(|active| active.as_ref().is_some_and(|a| a.id == self.id))
        }
    }

    impl Drop for EchoCancel {
        fn drop(&mut self) {
            if let Ok(mut active) = ACTIVE.lock()
                && active.as_ref().is_some_and(|a| a.id == self.id)
            {
                *active = None;
            }
        }
    }

    /// The capture for a dictation whose target is `target` (from
    /// `daemon::echo::capture_target_for`): [`SOURCE`] is the processed
    /// microphone, an endpoint id that microphone alone, `None` the default
    /// one. A [`SOURCE`] whose canceller stopped in between falls back to
    /// the plain default microphone.
    ///
    /// # Errors
    /// `capture_duration_invalid` for `seconds` outside the capture bound.
    pub fn capture_for(
        seconds: f32,
        target: Option<&str>,
    ) -> Result<MicrophoneCapture, PlatformError> {
        if target == Some(SOURCE) {
            let active = ACTIVE.lock().ok().and_then(|active| active.clone());
            if let Some(active) = active {
                let microphone = active
                    .microphone
                    .map_or(DeviceSelection::Default, DeviceSelection::Id);
                return ProcessedCapture::new(seconds, microphone, active.processing)
                    .map(MicrophoneCapture::Processed);
            }
        }
        let plain = WasapiCapture::new(seconds)?;
        Ok(MicrophoneCapture::Plain(match target {
            Some(id) if id != SOURCE => plain.with_device(DeviceSelection::Id(id.to_owned())),
            _ => plain,
        }))
    }

    /// A WASAPI recording endpoint id (`{0.0.1.00000000}.{GUID}`), which
    /// stands in for a PipeWire node name in the microphone choice.
    #[must_use]
    pub fn valid_node_name(name: &str) -> bool {
        name.starts_with('{')
            && name.len() <= 128
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"{}.-".contains(&b))
    }

    /// The active WASAPI recording endpoints, as `(endpoint id, friendly
    /// name)`. Unlike the rest of this module this one works: the microphone
    /// choice needs no PipeWire, only the device list.
    #[must_use]
    pub fn microphones() -> Vec<(String, String)> {
        narrivy_platform_windows::os::list_input_devices()
            .map(|devices| {
                devices
                    .into_iter()
                    .filter(|device| valid_node_name(&device.id))
                    .map(|device| (device.id, device.name))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The active WASAPI output endpoints (speakers, headphones), as
    /// `(endpoint id, friendly name)`: what a meeting's computer sound can be
    /// recorded from (a loopback).
    #[must_use]
    pub fn speakers() -> Vec<(String, String)> {
        narrivy_platform_windows::os::list_output_devices()
            .map(|devices| {
                devices
                    .into_iter()
                    .filter(|device| valid_node_name(&device.id))
                    .map(|device| (device.id, device.name))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Generated audio files (cue sounds): the per-user run directory.
    #[must_use]
    pub fn config_dir() -> PathBuf {
        super::super::host::runtime_dir().unwrap_or_else(|_| std::env::temp_dir().join("narrivy"))
    }
}

/// Press and hold: a low-level keyboard hook, with the Linux adapter's
/// keys and calls.
pub mod hold_key {
    pub use narrivy_platform_windows::hold_key::*;
}

/// Desktop shortcuts. Windows has no desktop shortcut store to write into,
/// so Narrivy keeps its own (`shortcuts.json` in the state directory) and
/// the daemon registers every entry itself with `RegisterHotKey`
/// ([`super::hotkey`]). The store has the shape of the Linux one, so the
/// product rules in `daemon::shortcuts` apply unchanged: an entry's command
/// is the CLI command the Linux desktop would run, and the daemon performs
/// it directly when the keys are pressed.
pub mod desktop_shortcuts {
    use std::path::PathBuf;

    use narrivy_platform_windows::shortcuts::Hotkey;
    pub use narrivy_platform_windows::shortcuts::Modifier;
    use serde::{Deserialize, Serialize};

    /// The store's file in the state directory.
    pub const STORE_FILE: &str = "shortcuts.json";
    /// Narrivy writes a handful of entries; anything larger is not its file.
    const MAX_STORE_BYTES: u64 = 64 * 1024;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Unmanaged {
        /// The state directory is unavailable.
        NoStateDirectory,
    }

    impl Unmanaged {
        #[must_use]
        pub fn reason(self) -> &'static str {
            "state_directory_unavailable"
        }
    }

    #[derive(Debug)]
    pub enum ShortcutError {
        InvalidCombination,
        InvalidCommand,
        UnrecognisedStore,
        StoreTooLarge,
        Io(std::io::Error),
    }

    /// One combination, as `RegisterHotKey` will register it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct KeyCombo(Hotkey);

    impl KeyCombo {
        /// # Errors
        /// [`ShortcutError::InvalidCombination`].
        pub fn new(modifiers: &[Modifier], key: &str) -> Result<Self, ShortcutError> {
            Hotkey::new(modifiers, key)
                .map(Self)
                .map_err(|_| ShortcutError::InvalidCombination)
        }

        #[must_use]
        pub fn modifiers(&self) -> &[Modifier] {
            self.0.modifiers()
        }

        #[must_use]
        pub fn key(&self) -> &str {
            self.0.key()
        }

        #[must_use]
        pub fn hotkey(&self) -> &Hotkey {
            &self.0
        }
    }

    pub enum Action {
        Spawn(String),
        Other(String),
    }

    pub struct Entry {
        pub combo: Option<KeyCombo>,
        pub action: Action,
    }

    pub enum Conflict {
        Custom(String),
        DesktopDefault(String),
    }

    pub enum AddOutcome {
        Added { replaced: Option<String> },
        Conflict(Conflict),
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(deny_unknown_fields)]
    struct Stored {
        /// `+`-joined, as [`Hotkey::parse`] reads it.
        keys: String,
        command: String,
    }

    pub struct DesktopShortcuts {
        path: PathBuf,
    }

    impl DesktopShortcuts {
        /// # Errors
        /// [`Unmanaged::NoStateDirectory`].
        pub fn detect() -> Result<Self, Unmanaged> {
            let dir = narrivy_platform_windows::instance::state_dir()
                .map_err(|_| Unmanaged::NoStateDirectory)?;
            Ok(Self::at(dir.join(STORE_FILE)))
        }

        #[must_use]
        pub fn at(path: PathBuf) -> Self {
            Self { path }
        }

        fn read(&self) -> Result<Vec<Stored>, ShortcutError> {
            match std::fs::metadata(&self.path) {
                Ok(meta) if meta.len() > MAX_STORE_BYTES => {
                    return Err(ShortcutError::StoreTooLarge);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(ShortcutError::Io(error)),
            }
            let bytes = std::fs::read(&self.path).map_err(ShortcutError::Io)?;
            serde_json::from_slice(&bytes).map_err(|_| ShortcutError::UnrecognisedStore)
        }

        fn write(&self, entries: &[Stored]) -> Result<(), ShortcutError> {
            let bytes =
                serde_json::to_vec_pretty(entries).map_err(|_| ShortcutError::UnrecognisedStore)?;
            let temporary = self.path.with_extension("json.tmp");
            std::fs::write(&temporary, bytes).map_err(ShortcutError::Io)?;
            std::fs::rename(&temporary, &self.path).map_err(ShortcutError::Io)
        }

        /// The entries Narrivy keeps, in order.
        ///
        /// # Errors
        /// The store cannot be read.
        pub fn list(&self) -> Result<Vec<Entry>, ShortcutError> {
            Ok(self
                .read()?
                .into_iter()
                .map(|stored| Entry {
                    combo: Hotkey::parse(&stored.keys).ok().map(KeyCombo),
                    action: Action::Spawn(stored.command),
                })
                .collect())
        }

        /// Bind `combo` to `command`. Keys bound to something else are a
        /// conflict unless `replace`; keys another program holds always are,
        /// because Windows gives a combination to one program only.
        ///
        /// # Errors
        /// The store cannot be read or written.
        pub fn add(
            &self,
            combo: &KeyCombo,
            command: &str,
            replace: bool,
        ) -> Result<AddOutcome, ShortcutError> {
            if command.is_empty() || command.contains('\n') {
                return Err(ShortcutError::InvalidCommand);
            }
            let keys = combo.hotkey().to_string();
            let mut entries = self.read()?;
            let previous = entries.iter().position(|entry| {
                Hotkey::parse(&entry.keys).is_ok_and(|held| held == *combo.hotkey())
            });
            let replaced = match previous {
                Some(index) if entries[index].command != command && !replace => {
                    return Ok(AddOutcome::Conflict(Conflict::Custom(
                        entries[index].command.clone(),
                    )));
                }
                Some(index) => Some(entries.remove(index).command),
                None => None,
            };
            let before = self.read()?;
            entries.push(Stored {
                keys,
                command: command.to_owned(),
            });
            self.write(&entries)?;
            // Windows decides whether the keys are free: register them now.
            if super::super::hotkey::reload_and_check(combo.hotkey()) == Err("shortcut_in_use") {
                self.write(&before)?;
                super::super::hotkey::reload();
                return Ok(AddOutcome::Conflict(Conflict::DesktopDefault(
                    "another program".to_owned(),
                )));
            }
            Ok(AddOutcome::Added {
                replaced: replaced.filter(|old| old != command),
            })
        }

        /// Remove the entry for `combo` when `is_ours` accepts its command.
        ///
        /// # Errors
        /// The store cannot be read or written.
        pub fn remove(
            &self,
            combo: &KeyCombo,
            is_ours: impl Fn(&str) -> bool,
        ) -> Result<bool, ShortcutError> {
            let mut entries = self.read()?;
            let before = entries.len();
            entries.retain(|entry| {
                !(Hotkey::parse(&entry.keys).is_ok_and(|held| held == *combo.hotkey())
                    && is_ours(&entry.command))
            });
            if entries.len() == before {
                return Ok(false);
            }
            self.write(&entries)?;
            super::super::hotkey::reload();
            Ok(true)
        }

        /// Nothing outside Narrivy's own store is ever replaced on Windows.
        ///
        /// # Errors
        /// Never.
        pub fn restore(&self, _raw: &str) -> Result<(), ShortcutError> {
            Ok(())
        }
    }
}

/// Secrets: the Windows Credential Manager (same calls as the Linux
/// Secret Service adapter).
pub mod secrets {
    pub use narrivy_platform_windows::secrets::*;
}

/// Compute discovery: no accelerator nodes and no probe programs are run,
/// so the planner sees a CPU-only host.
pub mod compute_probe {
    use std::time::Duration;

    use narrivy_compute::probe::{HostProbe, ProbeCommand, ProbeError, ProbeOutcome};

    #[derive(Debug, Clone, Copy, Default)]
    pub struct LinuxHostProbe;

    impl LinuxHostProbe {
        #[must_use]
        pub const fn new() -> Self {
            Self
        }
    }

    impl HostProbe for LinuxHostProbe {
        fn accel_nodes(&self) -> Result<Vec<String>, ProbeError> {
            Ok(Vec::new())
        }

        fn run_probe(
            &self,
            command: &ProbeCommand,
            _timeout: Duration,
        ) -> Result<ProbeOutcome, ProbeError> {
            Err(ProbeError::unavailable(
                command.program(),
                "probe programs are not run on this platform",
            ))
        }

        fn logical_cores(&self) -> Option<u32> {
            std::thread::available_parallelism()
                .ok()
                .and_then(|cores| u32::try_from(cores.get()).ok())
        }
    }
}

/// GPU busy counters: Windows' `GPU Engine` performance counters. No NPU
/// counter is read, so the support report shows `no_device` for it.
pub mod accelerator_usage {
    pub use narrivy_platform_windows::accelerator_usage::*;
}

/// The admission-time target binding of the Linux routed ladder. No
/// pre-activation target is bound on Windows either (typing goes to the
/// window focused at delivery time), so admission always yields the typed
/// unavailable binding, exactly as the installed Linux Start path does.
pub mod delivery {
    pub const PRE_ACTIVATION_BINDING_ABSENT: &str = "pre_activation_target_binding_absent";

    #[derive(Clone, PartialEq, Eq)]
    pub enum TargetBinding {
        Bound { application: String, window: String },
        Unavailable(&'static str),
    }

    impl std::fmt::Debug for TargetBinding {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // PRIVACY: never render captured application/window identity.
            match self {
                Self::Bound { .. } => formatter.write_str("TargetBinding::Bound(<redacted>)"),
                Self::Unavailable(reason) => {
                    write!(formatter, "TargetBinding::Unavailable({reason})")
                }
            }
        }
    }

    pub trait TargetBindingSource: Send {
        fn bind(&mut self) -> TargetBinding;
    }

    pub struct UnavailablePreActivationTargetBinding;

    impl TargetBindingSource for UnavailablePreActivationTargetBinding {
        fn bind(&mut self) -> TargetBinding {
            TargetBinding::Unavailable(PRE_ACTIVATION_BINDING_ABSENT)
        }
    }
}

/// Installed-package (trust-anchored) worker and model bindings: the
/// per-machine install's, from `narrivy-platform-windows` (an exception to
/// this module's stand-ins, like the microphone list).
pub mod installed_runtime {
    pub use narrivy_platform_windows::installed_runtime::*;
}

/// Offline import into the installed model cache (installed composition
/// only, which Windows does not have yet).
pub mod model_import {
    use std::path::Path;
    use std::time::Instant;

    use narrivy_model_manager::{BundledInstalledModelBinding, BundledModelRegistry, ModelStore};
    use narrivy_types::SessionCancellation;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum OfflineModelImportError {
        CancelledBeforePublication,
        TimedOutBeforePublication,
        UnknownModel,
        UnsafeCacheRoot,
        SourceUnavailable,
        SourceUnsafe,
        SourceChanged,
        BytesMismatch,
        ExistingInstallInvalid,
        Collision,
        CleanupFailedBeforePublication,
        DurabilityFailedBeforePublication,
        DurabilityFailedAfterPublication,
        VerificationFailedBeforePublication,
        VerificationFailedAfterPublication,
        Io,
    }

    pub fn import_bundled_model(
        _store: &ModelStore,
        _registry: &BundledModelRegistry,
        _id: &str,
        _source_path: &Path,
        _cancellation: &SessionCancellation,
        _deadline: Instant,
    ) -> Result<BundledInstalledModelBinding, OfflineModelImportError> {
        Err(OfflineModelImportError::UnsafeCacheRoot)
    }
}

#[cfg(test)]
mod tests {
    use super::echo_cancel::valid_node_name;

    #[test]
    fn only_endpoint_ids_are_valid_microphone_names() {
        assert!(valid_node_name(
            "{0.0.1.00000000}.{a1b2c3d4-0000-1111-2222-333344445555}"
        ));
        for bad in [
            "",
            "default",
            "C:\\x",
            "{0.0.1}.{x} ",
            "a\"b",
            &"a".repeat(129),
        ] {
            assert!(!valid_node_name(bad), "{bad:?}");
        }
    }
}
