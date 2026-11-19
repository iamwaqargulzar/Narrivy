//! `SendInput` for the typing and paste routes.

use narrivy_platform_api::{PlatformError, TextDelivery};
use narrivy_types::{SessionCancellation, TargetContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};

use crate::delivery::{DeliveryRung, KeyboardPort, RungOutcome, TypingRung};
use crate::error;
use crate::text_input::KeyStroke;

/// `dwExtraInfo` on every event Narrivy injects, so its own keyboard hook can
/// tell them from the user's keys ("NRVY").
pub const INJECTED_MARKER: usize = 0x4E52_5659;

fn to_input(stroke: KeyStroke) -> INPUT {
    let (vk, scan, flags) = match stroke {
        KeyStroke::Unicode { unit, up } => (
            VIRTUAL_KEY(0),
            unit,
            KEYEVENTF_UNICODE
                | if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
        ),
        KeyStroke::Virtual { vk, up } => {
            // SAFETY: pure table lookup, no pointers.
            let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) };
            (
                VIRTUAL_KEY(vk),
                u16::try_from(scan).unwrap_or(0),
                if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
            )
        }
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTED_MARKER,
            },
        },
    }
}

/// The real keyboard.
#[derive(Debug, Clone, Copy, Default)]
pub struct SendInputKeyboard;

impl KeyboardPort for SendInputKeyboard {
    fn send(&mut self, strokes: &[KeyStroke]) -> usize {
        if strokes.is_empty() {
            return 0;
        }
        let inputs: Vec<INPUT> = strokes.iter().copied().map(to_input).collect();
        let size = i32::try_from(std::mem::size_of::<INPUT>()).unwrap_or(i32::MAX);
        // SAFETY: `inputs` is a live slice of fully initialised keyboard
        // INPUTs and `size` is the size of one element. SendInput inserts
        // the events atomically into the input stream and returns how many
        // it inserted (0 when blocked, e.g. by UIPI).
        unsafe { SendInput(&inputs, size) as usize }
    }

    fn is_down(&mut self, vk: u16) -> bool {
        // SAFETY: plain value query.
        let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
        state as u16 & 0x8000 != 0
    }
}

/// The legacy [`TextDelivery`] contract: type into the focused window.
pub struct SendInputTextDelivery {
    rung: std::sync::Mutex<TypingRung<SendInputKeyboard>>,
}

impl Default for SendInputTextDelivery {
    fn default() -> Self {
        Self {
            rung: std::sync::Mutex::new(TypingRung::new(SendInputKeyboard)),
        }
    }
}

impl TextDelivery for SendInputTextDelivery {
    fn insert_text(&self, _target: &TargetContext, text: &str) -> Result<(), PlatformError> {
        let mut rung = self.rung.lock().map_err(|_| error("delivery_busy"))?;
        match rung.attempt(text, &SessionCancellation::new()) {
            RungOutcome::SubmittedUnverified => Ok(()),
            RungOutcome::CancelledBeforeDispatch => Err(error("cancelled")),
            RungOutcome::NotDispatched(_) => Err(error("delivery_not_dispatched")),
            RungOutcome::Failed(_) | RungOutcome::CopiedOnly => Err(error("delivery_failed")),
        }
    }
}
