//! Windows platform adapter (PORT-007, D36): the Windows counterpart of
//! `narrivy-platform-linux` for capture, text delivery, the foreground
//! target and global shortcuts, behind the contracts in
//! `narrivy-platform-api`.
//!
//! Layout:
//! - portable modules (`accelerator_usage`, `audio_format`, `capture`, `delivery`, `hold_key`, `secrets`,
//!   `shortcuts`, `stream_recorder`, `target`, `text_input`, [`WindowsCapabilities`]) hold every rule and
//!   state machine; they forbid `unsafe`, build on every target and are
//!   unit-tested on Linux too;
//! - [`os`] (Windows only) binds them to Win32 and COM: WASAPI capture,
//!   `SendInput`, the clipboard, the foreground window, `RegisterHotKey`, a
//!   low-level keyboard hook and the Credential Manager. All of the crate's `unsafe` is there, each
//!   call with a SAFETY comment.
//!
//! Privacy: no module logs, and no error or `Debug` output carries audio,
//! transcript, clipboard or window content; failures are short stable codes.

use narrivy_platform_api::{PlatformCapabilities, PlatformError};
use narrivy_types::Capability;

pub mod accelerator_usage;
pub mod audio_format;
pub mod capture;
pub mod delivery;
pub mod hold_key;
#[cfg(windows)]
pub mod installed_runtime;
pub mod instance;
pub mod screen_keeper;
pub mod secrets;
pub mod shortcuts;
pub mod stream_recorder;
pub mod target;
pub mod text_input;
pub mod voice_processing;

#[cfg(windows)]
pub mod os;

pub(crate) fn error(code: &'static str) -> PlatformError {
    PlatformError {
        code,
        message: code.to_owned(),
    }
}

/// What this adapter can do on Windows. Clients render these capabilities;
/// they never branch on the OS name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsCapabilities;

impl PlatformCapabilities for WindowsCapabilities {
    fn has(&self, capability: Capability) -> bool {
        match capability {
            // RegisterHotKey for toggle; a low-level hook sees the hold key's
            // release.
            Capability::GlobalShortcutPressRelease
            // Foreground window, title and executable.
            | Capability::ActiveApplication
            // SendInput (Unicode) and clipboard paste.
            | Capability::TextInsert => true,
            // Not implemented yet: UI Automation (selection and surrounding
            // text), the overlay (desktop app), media keys, Credential
            // Manager, WASAPI loopback.
            Capability::SelectedText
            | Capability::SurroundingText
            | Capability::TextReplaceSelection
            | Capability::Overlay
            | Capability::MediaControl
            | Capability::SecretStore
            | Capability::SystemAudioCapture => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_report_what_is_implemented() {
        let caps = WindowsCapabilities;
        assert!(caps.has(Capability::TextInsert));
        assert!(caps.has(Capability::GlobalShortcutPressRelease));
        assert!(caps.has(Capability::ActiveApplication));
        assert!(!caps.has(Capability::SelectedText));
        assert!(!caps.has(Capability::SystemAudioCapture));
    }
}
