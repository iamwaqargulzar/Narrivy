//! Reopen the app after an installer upgrade.

use windows::Win32::System::Recovery::{
    RESTART_NO_CRASH, RESTART_NO_HANG, RESTART_NO_REBOOT, RegisterApplicationRestart,
};
use windows::core::PCWSTR;

/// Ask Windows to start this program again when an installer (through the
/// Restart Manager) closes it to replace its files, so an MSI upgrade
/// reopens Narrivy as the Linux package's restart does. Not after a crash, a
/// hang or a reboot. A refusal only means it stays closed after an upgrade.
pub fn reopen_after_update() {
    // SAFETY: a null command line (start with no arguments) and plain flags;
    // Windows copies what it keeps.
    let _ = unsafe {
        RegisterApplicationRestart(
            PCWSTR::null(),
            RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_REBOOT,
        )
    };
}
