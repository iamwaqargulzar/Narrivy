//! Press and hold: one key, read from the keyboards' input devices.
//!
//! COSMIC's custom shortcuts run a command on key press only, and it has no
//! GlobalShortcuts portal, so nothing reports the key's release. Reading the
//! keyboards' event devices (`/dev/input/event*`) does. That needs the user
//! in the `input` group, which lets their processes observe every keystroke,
//! so the feature is opt-in and the user is told so (T2.13).
//!
//! What this module does with that access:
//!
//! - it reads key events from devices that are keyboards (key and repeat
//!   events, per `/proc/bus/input/devices`);
//! - it keeps only press and release of the **one** chosen key code: every
//!   other event is dropped as soon as it is read, and nothing is logged or
//!   stored;
//! - keyboards plugged in later are picked up (rescanned every few
//!   seconds); a keyboard that goes away while the key is down counts as a
//!   release, so a session never stays open on a lost release.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const EV_KEY: u16 = 1;
/// `struct input_event` on 64-bit Linux: two 64-bit time fields, type,
/// code, value.
const EVENT_BYTES: usize = 24;
const RESCAN: Duration = Duration::from_secs(3);

/// Keys offered for press and hold: ones that do nothing much alone.
pub const KEYS: [(u16, &str); 8] = [
    (97, "Right Ctrl"),
    (100, "Right Alt"),
    (126, "Right Super"),
    (127, "Menu"),
    (70, "Scroll Lock"),
    (119, "Pause"),
    (183, "F13"),
    (184, "F14"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldEvent {
    Pressed,
    Released,
}

/// One key event, when it is the wanted key's press or release.
fn parse(event: &[u8; EVENT_BYTES], wanted: u16) -> Option<HoldEvent> {
    let kind = u16::from_ne_bytes([event[16], event[17]]);
    let code = u16::from_ne_bytes([event[18], event[19]]);
    let value = i32::from_ne_bytes([event[20], event[21], event[22], event[23]]);
    if kind != EV_KEY || code != wanted {
        return None;
    }
    match value {
        1 => Some(HoldEvent::Pressed),
        0 => Some(HoldEvent::Released),
        // 2 is auto-repeat while held.
        _ => None,
    }
}

/// The keyboards' event devices, from the kernel's device list.
fn keyboards(list: &str) -> Vec<PathBuf> {
    list.split("\n\n")
        .filter_map(|block| {
            let ev = block
                .lines()
                .find_map(|l| l.strip_prefix("B: EV="))
                .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())?;
            // Key events and auto-repeat: a keyboard, not a power button.
            if ev & 0x2 == 0 || ev & 0x10_0000 == 0 {
                return None;
            }
            let handlers = block.lines().find_map(|l| l.strip_prefix("H: Handlers="))?;
            handlers
                .split_whitespace()
                .find(|h| h.starts_with("event") && h[5..].bytes().all(|b| b.is_ascii_digit()))
                .map(|h| PathBuf::from("/dev/input").join(h))
        })
        .collect()
}

struct Shared {
    wanted: u16,
    stop: AtomicBool,
    pressed: AtomicBool,
    /// Keyboards being read now.
    open: AtomicUsize,
    /// A keyboard could not be opened for lack of permission.
    denied: AtomicBool,
    watched: Mutex<BTreeSet<PathBuf>>,
}

/// The running key reader. Dropping it stops reporting.
pub struct HoldKey {
    shared: Arc<Shared>,
}

impl HoldKey {
    /// Start reading `code` (a Linux key code, such as one of [`KEYS`]).
    /// `on_event` runs on reader threads for each press and release.
    ///
    /// # Errors
    /// `hold_key_unknown` for a code outside [`KEYS`].
    pub fn start(
        code: u16,
        on_event: impl Fn(HoldEvent) + Send + Sync + 'static,
    ) -> Result<Self, &'static str> {
        if !KEYS.iter().any(|(k, _)| *k == code) {
            return Err("hold_key_unknown");
        }
        let shared = Arc::new(Shared {
            wanted: code,
            stop: AtomicBool::new(false),
            pressed: AtomicBool::new(false),
            open: AtomicUsize::new(0),
            denied: AtomicBool::new(false),
            watched: Mutex::new(BTreeSet::new()),
        });
        let on_event: Arc<dyn Fn(HoldEvent) + Send + Sync> = Arc::new(on_event);
        let scanner = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("narrivy-hold-scan".into())
            .spawn(move || {
                while !scanner.stop.load(Ordering::Relaxed) {
                    let list = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
                    for device in keyboards(&list) {
                        let fresh = scanner
                            .watched
                            .lock()
                            .is_ok_and(|mut watched| watched.insert(device.clone()));
                        if fresh {
                            watch(Arc::clone(&scanner), device, Arc::clone(&on_event));
                        }
                    }
                    std::thread::sleep(RESCAN);
                }
            })
            .map_err(|_| "hold_key_unavailable")?;
        Ok(Self { shared })
    }

    /// Whether at least one keyboard is being read.
    #[must_use]
    pub fn is_reading(&self) -> bool {
        self.shared.open.load(Ordering::Relaxed) > 0
    }

    /// Whether a keyboard could not be read for lack of permission (the
    /// user is not in the `input` group).
    #[must_use]
    pub fn is_denied(&self) -> bool {
        self.shared.denied.load(Ordering::Relaxed)
    }
}

impl Drop for HoldKey {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

fn watch(shared: Arc<Shared>, device: PathBuf, on_event: Arc<dyn Fn(HoldEvent) + Send + Sync>) {
    let spawned = std::thread::Builder::new()
        .name("narrivy-hold-read".into())
        .spawn(move || {
            let file = std::fs::File::open(&device);
            let mut file = match file {
                Ok(file) => file,
                Err(err) => {
                    if err.kind() == std::io::ErrorKind::PermissionDenied {
                        shared.denied.store(true, Ordering::Relaxed);
                    }
                    forget(&shared, &device);
                    return;
                }
            };
            shared.open.fetch_add(1, Ordering::Relaxed);
            let mut event = [0u8; EVENT_BYTES];
            let mut held_here = false;
            while file.read_exact(&mut event).is_ok() {
                if shared.stop.load(Ordering::Relaxed) {
                    break;
                }
                match parse(&event, shared.wanted) {
                    Some(HoldEvent::Pressed) => {
                        held_here = true;
                        if !shared.pressed.swap(true, Ordering::AcqRel) {
                            on_event(HoldEvent::Pressed);
                        }
                    }
                    Some(HoldEvent::Released) => {
                        held_here = false;
                        if shared.pressed.swap(false, Ordering::AcqRel) {
                            on_event(HoldEvent::Released);
                        }
                    }
                    None => {}
                }
                event = [0u8; EVENT_BYTES];
            }
            // The keyboard went away (or reading stopped) with the key down:
            // that is a release.
            if held_here
                && !shared.stop.load(Ordering::Relaxed)
                && shared.pressed.swap(false, Ordering::AcqRel)
            {
                on_event(HoldEvent::Released);
            }
            shared.open.fetch_sub(1, Ordering::Relaxed);
            forget(&shared, &device);
        });
    if spawned.is_err() {
        // Picked up again at the next scan.
    }
}

fn forget(shared: &Shared, device: &PathBuf) {
    if let Ok(mut watched) = shared.watched.lock() {
        watched.remove(device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: u16, code: u16, value: i32) -> [u8; EVENT_BYTES] {
        let mut e = [0u8; EVENT_BYTES];
        e[16..18].copy_from_slice(&kind.to_ne_bytes());
        e[18..20].copy_from_slice(&code.to_ne_bytes());
        e[20..24].copy_from_slice(&value.to_ne_bytes());
        e
    }

    #[test]
    fn only_the_chosen_keys_press_and_release_count() {
        assert_eq!(parse(&event(EV_KEY, 97, 1), 97), Some(HoldEvent::Pressed));
        assert_eq!(parse(&event(EV_KEY, 97, 0), 97), Some(HoldEvent::Released));
        // Auto-repeat, other keys and other event types are dropped.
        assert_eq!(parse(&event(EV_KEY, 97, 2), 97), None);
        assert_eq!(parse(&event(EV_KEY, 30, 1), 97), None);
        assert_eq!(parse(&event(4, 97, 1), 97), None);
    }

    #[test]
    fn keyboards_are_told_apart_from_buttons() {
        let list = "I: Bus=0019\nN: Name=\"Power Button\"\nH: Handlers=kbd event1 \nB: EV=3\n\n\
                    I: Bus=0011\nN: Name=\"AT keyboard\"\nH: Handlers=sysrq kbd event2 leds \nB: EV=120013\n";
        assert_eq!(keyboards(list), vec![PathBuf::from("/dev/input/event2")]);
    }

    #[test]
    fn only_offered_keys_can_be_chosen() {
        assert!(HoldKey::start(30, |_| {}).is_err());
    }
}
