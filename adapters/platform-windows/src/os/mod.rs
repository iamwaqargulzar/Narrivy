//! Win32 and COM bindings (Windows only). All of the crate's `unsafe` lives
//! under this module; every call states its invariants in a SAFETY comment.
//! The rules these bindings follow are in the portable modules.

mod clipboard;
mod com;
pub(crate) mod credentials;
pub(crate) mod gpu_counters;
mod hotkeys;
mod input;
mod microphone_access;
pub(crate) mod npu_adapters;
mod processed_capture;
mod restart;
mod screen_grab;
mod target;
mod wasapi;

pub use clipboard::{ClipboardSnapshot, Win32Clipboard};
pub use hotkeys::{ShortcutEvent, ShortcutListener, WindowsShortcuts};
pub use input::{INJECTED_MARKER, SendInputKeyboard, SendInputTextDelivery};
pub use microphone_access::microphone_blocked;
pub use processed_capture::{MicrophoneCapture, ProcessedCapture};
pub use restart::reopen_after_update;
pub use screen_grab::grab_monitors;
pub use target::ForegroundTarget;
pub use wasapi::{
    DeviceSelection, Endpoint, InputDevice, WasapiCapture, default_device_id, list_input_devices,
    list_output_devices, open_source,
};

use std::path::PathBuf;

use crate::delivery::{ClipboardPasteRung, DeliveryMethod, FileInbox, RoutedDelivery, TypingRung};

/// The production delivery port: typing and clipboard paste in the order of
/// the user's setting (paste only with the clipboard authorization), then
/// the recovery Inbox in `inbox_dir`.
#[must_use]
pub fn routed_delivery(
    method: DeliveryMethod,
    clipboard_authorized: bool,
    inbox_dir: PathBuf,
) -> RoutedDelivery {
    RoutedDelivery::new(
        method,
        clipboard_authorized,
        Box::new(TypingRung::new(SendInputKeyboard)),
        Box::new(ClipboardPasteRung::new(Win32Clipboard, SendInputKeyboard)),
        Box::new(FileInbox::new(inbox_dir)),
    )
}
