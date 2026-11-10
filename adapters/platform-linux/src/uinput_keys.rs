//! A virtual keyboard through `/dev/uinput` that can press only Shift+Insert.
//!
//! COSMIC (cosmic-comp, October 2026) ignores the modifiers sent through the
//! Wayland virtual-keyboard protocol, so `wtype`'s Shift+Insert arrives as a
//! bare Insert and never pastes. Keys from a uinput device go through the
//! compositor's ordinary keyboard path, modifiers included.
//!
//! Least privilege (FUN-004, ADR-0023): uinput is never the default. This
//! keyboard is used only when the user chooses the `paste-keyboard` delivery
//! method, and only if the user can already open `/dev/uinput` (an ACL or
//! group the system set up); Narrivy never changes that permission. The
//! device declares two keys, Left Shift and Insert, so it cannot type text
//! or press any other key.
//!
//! The device is created on the first paste and kept for the life of the
//! daemon process: a newly created device takes a moment before the
//! compositor reads it, so creating one per paste would drop the first keys.
//! The kernel removes it when the process exits (its file is closed).
//!
//! Unsafe: the four `ioctl` calls that set the device up and remove it. Each
//! passes a valid open descriptor and, for `UI_DEV_SETUP`, a pointer to a
//! fully initialised `uinput_setup` that lives across the call.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use narrivy_types::SessionCancellation;

const DEVICE: &str = "/dev/uinput";
const NAME: &[u8] = b"Narrivy paste keys";

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const SYN_REPORT: u16 = 0;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_INSERT: u16 = 110;
const BUS_VIRTUAL: u16 = 0x06;

// `linux/uinput.h`: _IO('U', n) and _IOW('U', n, T).
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
const UI_DEV_SETUP: libc::c_ulong = ioc_write(3, size_of::<libc::uinput_setup>());
const UI_SET_EVBIT: libc::c_ulong = ioc_write(100, size_of::<libc::c_int>());
const UI_SET_KEYBIT: libc::c_ulong = ioc_write(101, size_of::<libc::c_int>());

/// How long a new device is given before its first key: the compositor
/// (libinput) picks it up from udev asynchronously.
const SETTLE: Duration = Duration::from_millis(250);
/// Between the key events of one chord, so the compositor sees each state.
const KEY_GAP: Duration = Duration::from_millis(8);

const fn ioc_write(nr: libc::c_ulong, size: usize) -> libc::c_ulong {
    (1 << 30) | ((size as libc::c_ulong) << 16) | (0x55 << 8) | nr
}

/// `struct input_event` on 64-bit Linux: two 64-bit time fields (left zero;
/// the input core stamps the event), type, code, value.
#[cfg(target_pointer_width = "64")]
fn event(kind: u16, code: u16, value: i32) -> [u8; 24] {
    let mut bytes = [0_u8; 24];
    bytes[16..18].copy_from_slice(&kind.to_ne_bytes());
    bytes[18..20].copy_from_slice(&code.to_ne_bytes());
    bytes[20..24].copy_from_slice(&value.to_ne_bytes());
    bytes
}

/// Whether `/dev/uinput` exists and this user may open it for writing.
/// Content-free: `None` when it can, else a stable reason code.
#[must_use]
pub fn unavailable_reason() -> Option<&'static str> {
    if !cfg!(target_pointer_width = "64") {
        return Some("uinput_unsupported_platform");
    }
    let path = std::path::Path::new(DEVICE);
    if !path.exists() {
        return Some("uinput_missing");
    }
    match rustix::fs::access(path, rustix::fs::Access::WRITE_OK) {
        Ok(()) => None,
        Err(_) => Some("uinput_no_access"),
    }
}

struct Keyboard {
    file: File,
}

impl Keyboard {
    fn create() -> Result<Self, &'static str> {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(DEVICE)
            .map_err(|_| "uinput_open_failed")?;
        let fd = file.as_raw_fd();
        let mut setup = libc::uinput_setup {
            id: libc::input_id {
                bustype: BUS_VIRTUAL,
                vendor: 0,
                product: 0,
                version: 1,
            },
            name: [0; libc::UINPUT_MAX_NAME_SIZE],
            ff_effects_max: 0,
        };
        for (slot, byte) in setup.name.iter_mut().zip(NAME) {
            *slot = *byte as libc::c_char;
        }
        // SAFETY: `fd` is the open uinput descriptor owned by `file`; the
        // int arguments are passed by value as `linux/uinput.h` defines.
        let bits_ok = unsafe {
            libc::ioctl(fd, UI_SET_EVBIT, libc::c_int::from(EV_KEY)) == 0
                && libc::ioctl(fd, UI_SET_KEYBIT, libc::c_int::from(KEY_LEFTSHIFT)) == 0
                && libc::ioctl(fd, UI_SET_KEYBIT, libc::c_int::from(KEY_INSERT)) == 0
        };
        if !bits_ok {
            return Err("uinput_setup_failed");
        }
        // SAFETY: `setup` is fully initialised and outlives the call; the
        // kernel only reads `size_of::<uinput_setup>()` bytes from it.
        let created = unsafe {
            libc::ioctl(fd, UI_DEV_SETUP, &raw mut setup) == 0
                && libc::ioctl(fd, UI_DEV_CREATE) == 0
        };
        if !created {
            return Err("uinput_create_failed");
        }
        std::thread::sleep(SETTLE);
        Ok(Self { file })
    }

    fn key(&mut self, code: u16, down: bool) -> Result<(), &'static str> {
        let mut bytes = [0_u8; 48];
        bytes[..24].copy_from_slice(&event(EV_KEY, code, i32::from(down)));
        bytes[24..].copy_from_slice(&event(EV_SYN, SYN_REPORT, 0));
        self.file
            .write_all(&bytes)
            .map_err(|_| "uinput_write_failed")?;
        std::thread::sleep(KEY_GAP);
        Ok(())
    }

    fn shift_insert(&mut self) -> Result<(), &'static str> {
        self.key(KEY_LEFTSHIFT, true)?;
        let pressed = self
            .key(KEY_INSERT, true)
            .and_then(|()| self.key(KEY_INSERT, false));
        // Shift is let go whatever happened, so it is never left held.
        let released = self.key(KEY_LEFTSHIFT, false);
        pressed.and(released)
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        // SAFETY: the descriptor is still open (owned by `self.file`).
        unsafe {
            libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY);
        }
    }
}

fn keyboard() -> &'static Mutex<Option<Keyboard>> {
    static KEYBOARD: OnceLock<Mutex<Option<Keyboard>>> = OnceLock::new();
    KEYBOARD.get_or_init(|| Mutex::new(None))
}

/// Press Shift+Insert once on the shared virtual keyboard, creating it on
/// first use. A failed write drops the device so the next paste makes a new
/// one. Returns a stable, content-free code on failure.
///
/// # Errors
/// `cancelled`, or a `uinput_*` code.
pub fn press_shift_insert(cancellation: &SessionCancellation) -> Result<(), &'static str> {
    if cancellation.is_cancelled() {
        return Err("cancelled");
    }
    if let Some(reason) = unavailable_reason() {
        return Err(reason);
    }
    let mut slot = keyboard().lock().map_err(|_| "uinput_unavailable")?;
    if slot.is_none() {
        *slot = Some(Keyboard::create()?);
    }
    let result = slot.as_mut().ok_or("uinput_unavailable")?.shift_insert();
    if result.is_err() {
        *slot = None;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_the_kernel_header() {
        assert_eq!(size_of::<libc::uinput_setup>(), 92);
        assert_eq!(UI_DEV_SETUP, 0x405c_5503);
        assert_eq!(UI_SET_EVBIT, 0x4004_5564);
        assert_eq!(UI_SET_KEYBIT, 0x4004_5565);
    }

    #[test]
    fn events_are_laid_out_as_input_event() {
        let bytes = event(EV_KEY, KEY_INSERT, 1);
        assert_eq!(&bytes[..16], &[0; 16]);
        assert_eq!(u16::from_ne_bytes([bytes[16], bytes[17]]), EV_KEY);
        assert_eq!(u16::from_ne_bytes([bytes[18], bytes[19]]), KEY_INSERT);
        assert_eq!(
            i32::from_ne_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            1
        );
    }

    /// Live, on a machine with `/dev/uinput` access: the device is created
    /// with its name and only its two keys. Presses nothing.
    #[test]
    #[ignore = "creates a real uinput device"]
    fn live_device_is_created_with_two_keys() -> Result<(), &'static str> {
        let keyboard = Keyboard::create()?;
        let devices =
            std::fs::read_to_string("/proc/bus/input/devices").map_err(|_| "devices_unreadable")?;
        let entry = devices
            .split("\n\n")
            .find(|entry| entry.contains("Narrivy paste keys"))
            .ok_or("device_not_listed")?;
        // KEY bitmap: bit 42 (Left Shift) and bit 110 (Insert) only.
        assert!(entry.contains("B: KEY=400000000000 40000000000"), "{entry}");
        drop(keyboard);
        Ok(())
    }

    #[test]
    fn a_cancelled_session_presses_nothing() {
        let cancellation = SessionCancellation::new();
        cancellation.cancel();
        assert_eq!(press_shift_insert(&cancellation), Err("cancelled"));
    }
}
