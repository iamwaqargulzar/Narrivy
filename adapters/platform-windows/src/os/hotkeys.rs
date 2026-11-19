//! Global shortcuts on a message-loop thread.
//!
//! Toggle shortcuts with a key are `RegisterHotKey` registrations (press
//! only, with `MOD_NOREPEAT`). The press-and-hold key and modifiers-only
//! shortcuts are watched by a low-level keyboard hook: it compares each
//! event's virtual key with the chosen hold key and feeds it to each
//! modifiers-only [`ChordTracker`], which keeps only the keys held right
//! now. Nothing is stored, logged or forwarded except the hold key's press
//! and release and a chord's firing. Events Narrivy injects itself are
//! ignored.
//!
//! The hook runs on the message loop, and Windows removes a hook that is slow
//! to return, so the loop never runs caller code: events go over a bounded
//! queue to a dispatcher thread that calls the callback.
//!
//! Lifetime: [`ShortcutListener::stop`] (or drop) posts `WM_QUIT`, which
//! unregisters every shortcut, removes the hook, reports a held key as
//! released, and joins both threads under a bound.

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use narrivy_platform_api::{PlatformError, ShortcutProvider};
use windows::Win32::Foundation::{
    ERROR_HOTKEY_ALREADY_REGISTERED, HINSTANCE, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_HOTKEY,
    WM_KEYUP, WM_QUIT, WM_SYSKEYUP, WM_USER,
};
use windows::core::{HRESULT, PCWSTR};

use super::input::INJECTED_MARKER;
use crate::error;
use crate::shortcuts::{ChordTracker, HoldEvent, HoldTracker, Hotkey};

const START_BOUND: Duration = Duration::from_secs(2);
const STOP_BOUND: Duration = Duration::from_secs(2);
/// Shortcut events waiting for the callback. Presses are human-paced; a full
/// queue means the callback is stuck, and further events are dropped rather
/// than stall the keyboard hook.
const EVENT_QUEUE: usize = 256;

/// One shortcut event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutEvent {
    /// The toggle shortcut at this index of the started list was pressed.
    Pressed(usize),
    /// The hold key went down or up.
    Hold(HoldEvent),
}

type Callback = Box<dyn Fn(ShortcutEvent) + Send>;

/// A running listener.
pub struct ShortcutListener {
    thread_id: u32,
    message_loop: Option<JoinHandle<()>>,
    dispatcher: Option<JoinHandle<()>>,
}

struct HookState {
    /// The hold key's virtual key and press/release tracker.
    hold: Option<(u32, HoldTracker)>,
    /// Modifiers-only shortcuts, by index of the started list.
    chords: Vec<(usize, ChordTracker)>,
    events: SyncSender<ShortcutEvent>,
}

thread_local! {
    /// The hook procedure's state, on the message-loop thread only.
    static HOOK: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

fn forward(events: &SyncSender<ShortcutEvent>, event: ShortcutEvent) {
    match events.try_send(event) {
        Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points to a KBDLLHOOKSTRUCT valid for
        // the duration of this call.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.dwExtraInfo != INJECTED_MARKER {
            HOOK.with(|state| {
                let Ok(mut state) = state.try_borrow_mut() else {
                    return;
                };
                let Some(state) = state.as_mut() else {
                    return;
                };
                let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
                if let Some((vk, tracker)) = state.hold.as_mut()
                    && info.vkCode == *vk
                    && let Some(event) = tracker.key_event(up)
                {
                    forward(&state.events, ShortcutEvent::Hold(event));
                }
                for (index, chord) in &mut state.chords {
                    if chord.key_event(info.vkCode, up) {
                        forward(&state.events, ShortcutEvent::Pressed(*index));
                    }
                }
            });
        }
    }
    // Never swallow the key: other applications see it as before.
    // SAFETY: passes the hook's own arguments through unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Result of one registration, by index of the started list.
pub type Registrations = Vec<Result<(), &'static str>>;

impl ShortcutListener {
    /// Register `hotkeys` (and watch `hold_vk`, one of
    /// [`crate::shortcuts::HOLD_KEYS`]) and call `on_event` for each event,
    /// on a dispatcher thread. A shortcut another application already holds
    /// is reported as `shortcut_in_use` in the returned list; the others
    /// still work.
    ///
    /// # Errors
    /// `shortcut_thread_failed`, or `shortcut_hook_failed` when the hold key
    /// cannot be watched.
    pub fn start(
        hotkeys: Vec<Hotkey>,
        hold_vk: Option<u16>,
        on_event: Callback,
    ) -> Result<(Self, Registrations), PlatformError> {
        let (events, received) = sync_channel::<ShortcutEvent>(EVENT_QUEUE);
        let dispatcher = std::thread::Builder::new()
            .name("narrivy-shortcut-dispatch".to_owned())
            .spawn(move || dispatch(&received, &on_event))
            .map_err(|_| error("shortcut_thread_failed"))?;

        let (ready, started) = sync_channel(1);
        let message_loop = std::thread::Builder::new()
            .name("narrivy-shortcuts".to_owned())
            .spawn(move || run_message_loop(&hotkeys, hold_vk, events, &ready))
            .map_err(|_| error("shortcut_thread_failed"))?;
        match started.recv_timeout(START_BOUND) {
            Ok(Ok((thread_id, registrations))) => Ok((
                Self {
                    thread_id,
                    message_loop: Some(message_loop),
                    dispatcher: Some(dispatcher),
                },
                registrations,
            )),
            Ok(Err(failure)) => {
                let _ = join_bounded(message_loop, Instant::now() + STOP_BOUND);
                let _ = join_bounded(dispatcher, Instant::now() + STOP_BOUND);
                Err(failure)
            }
            Err(_) => Err(error("shortcut_thread_failed")),
        }
    }

    /// Unregister everything and join the threads.
    ///
    /// # Errors
    /// `shortcut_stop_timeout` when a thread did not end in time.
    pub fn stop(mut self) -> Result<(), PlatformError> {
        self.stop_owned()
    }

    fn stop_owned(&mut self) -> Result<(), PlatformError> {
        let Some(message_loop) = self.message_loop.take() else {
            return Ok(());
        };
        // SAFETY: posting to the loop thread's queue (created before the
        // thread reported ready); harmless if the thread already ended.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        let deadline = Instant::now() + STOP_BOUND;
        let looped = join_bounded(message_loop, deadline);
        // The loop dropped the event sender, so the dispatcher ends too.
        let dispatched = self
            .dispatcher
            .take()
            .map_or(Ok(()), |dispatcher| join_bounded(dispatcher, deadline));
        looped.and(dispatched)
    }
}

impl Drop for ShortcutListener {
    fn drop(&mut self) {
        let _ = self.stop_owned();
    }
}

fn join_bounded(thread: JoinHandle<()>, deadline: Instant) -> Result<(), PlatformError> {
    while !thread.is_finished() {
        if Instant::now() >= deadline {
            return Err(error("shortcut_stop_timeout"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    thread.join().map_err(|_| error("shortcut_thread_panicked"))
}

fn dispatch(received: &Receiver<ShortcutEvent>, on_event: &Callback) {
    while let Ok(event) = received.recv() {
        on_event(event);
    }
}

type Ready = SyncSender<Result<(u32, Registrations), PlatformError>>;

fn run_message_loop(
    hotkeys: &[Hotkey],
    hold_vk: Option<u16>,
    events: SyncSender<ShortcutEvent>,
    ready: &Ready,
) {
    let mut msg = MSG::default();
    // SAFETY: creates this thread's message queue so PostThreadMessageW
    // works from the moment the thread id is published.
    unsafe {
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
    }
    // SAFETY: plain query.
    let thread_id = unsafe { GetCurrentThreadId() };

    let mut registered = Vec::new();
    let mut chords = Vec::new();
    let registrations: Registrations = hotkeys
        .iter()
        .enumerate()
        .map(|(index, hotkey)| {
            if hotkey.is_modifiers_only() {
                chords.push((index, ChordTracker::new(hotkey.modifiers())));
                return Ok(());
            }
            let id = i32::try_from(index + 1).map_err(|_| "shortcut_register_failed")?;
            let (modifiers, vk) = hotkey.win32();
            // SAFETY: thread-level registration (no window); ids are unique
            // within this thread and unregistered below on the same thread.
            match unsafe { RegisterHotKey(None, id, HOT_KEY_MODIFIERS(modifiers), vk) } {
                Ok(()) => {
                    registered.push(id);
                    Ok(())
                }
                Err(failure)
                    if failure.code() == HRESULT::from_win32(ERROR_HOTKEY_ALREADY_REGISTERED.0) =>
                {
                    Err("shortcut_in_use")
                }
                Err(_) => Err("shortcut_register_failed"),
            }
        })
        .collect();

    let mut hook: Option<HHOOK> = None;
    if hold_vk.is_some() || !chords.is_empty() {
        HOOK.with(|state| {
            *state.borrow_mut() = Some(HookState {
                hold: hold_vk.map(|vk| (u32::from(vk), HoldTracker::default())),
                chords,
                events: events.clone(),
            });
        });
        // SAFETY: this module's handle; the hook procedure lives in this
        // module for the life of the process. Removed below on this thread.
        let installed = unsafe {
            GetModuleHandleW(PCWSTR::null()).and_then(|module| {
                SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(keyboard_hook),
                    Some(HINSTANCE(module.0)),
                    0,
                )
            })
        };
        match installed {
            Ok(handle) => hook = Some(handle),
            Err(_) => {
                unregister(&registered);
                HOOK.with(|state| state.borrow_mut().take());
                let _ = ready.send(Err(error("shortcut_hook_failed")));
                return;
            }
        }
    }

    if ready.send(Ok((thread_id, registrations))).is_err() {
        cleanup(&registered, hook, &events);
        return;
    }

    loop {
        // SAFETY: out-pointer to a live MSG; thread messages included.
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        // 0: WM_QUIT. -1: error (no window filter, so not expected).
        if result.0 == 0 || result.0 == -1 || msg.message == WM_QUIT {
            break;
        }
        if msg.message == WM_HOTKEY {
            let index = msg.wParam.0.wrapping_sub(1);
            if index < hotkeys.len() {
                forward(&events, ShortcutEvent::Pressed(index));
            }
        }
    }
    cleanup(&registered, hook, &events);
}

fn unregister(ids: &[i32]) {
    for id in ids {
        // SAFETY: ids registered by this thread.
        let _ = unsafe { UnregisterHotKey(None, *id) };
    }
}

fn cleanup(registered: &[i32], hook: Option<HHOOK>, events: &SyncSender<ShortcutEvent>) {
    unregister(registered);
    if let Some(hook) = hook {
        // SAFETY: the hook this thread installed, removed once.
        let _ = unsafe { UnhookWindowsHookEx(hook) };
    }
    // A key still down counts as released, so a session never waits for a
    // release that will not come.
    let state = HOOK.with(|state| state.borrow_mut().take());
    if let Some(event) =
        state.and_then(|mut state| state.hold.as_mut().and_then(|(_, tracker)| tracker.reset()))
    {
        forward(events, ShortcutEvent::Hold(event));
    }
}

/// [`ShortcutProvider`] for the primary dictation shortcut: started on the
/// first `register_primary_trigger`, stopped on drop.
pub struct WindowsShortcuts {
    primary: Hotkey,
    hold_vk: Option<u16>,
    on_event: Arc<dyn Fn(ShortcutEvent) + Send + Sync>,
    listener: Mutex<Option<ShortcutListener>>,
}

impl WindowsShortcuts {
    pub fn new(
        primary: Hotkey,
        hold_vk: Option<u16>,
        on_event: Arc<dyn Fn(ShortcutEvent) + Send + Sync>,
    ) -> Self {
        Self {
            primary,
            hold_vk,
            on_event,
            listener: Mutex::new(None),
        }
    }
}

impl ShortcutProvider for WindowsShortcuts {
    fn register_primary_trigger(&self) -> Result<(), PlatformError> {
        let mut listener = self
            .listener
            .lock()
            .map_err(|_| error("shortcut_register_failed"))?;
        if listener.is_some() {
            return Ok(());
        }
        let on_event = Arc::clone(&self.on_event);
        let (started, registrations) = ShortcutListener::start(
            vec![self.primary.clone()],
            self.hold_vk,
            Box::new(move |event| on_event(event)),
        )?;
        if let Some(Err(code)) = registrations.first() {
            let _ = started.stop();
            return Err(error(code));
        }
        *listener = Some(started);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Registers a real global shortcut: run on the Windows test machine.
    #[test]
    #[ignore = "needs an interactive Windows session"]
    fn registers_and_stops_cleanly() {
        let hotkey = match Hotkey::parse("Ctrl+Alt+F23") {
            Ok(hotkey) => hotkey,
            Err(failure) => unreachable!("{failure}"),
        };
        let started = ShortcutListener::start(vec![hotkey.clone()], Some(0x7D), Box::new(|_| {}));
        let (listener, registrations) = match started {
            Ok(started) => started,
            Err(failure) => unreachable!("{failure:?}"),
        };
        assert_eq!(registrations, [Ok(())]);
        // A second registration of the same keys is reported, not fatal.
        let second = ShortcutListener::start(vec![hotkey], None, Box::new(|_| {}));
        match second {
            Ok((other, registrations)) => {
                assert_eq!(registrations, [Err("shortcut_in_use")]);
                assert!(other.stop().is_ok());
            }
            Err(failure) => unreachable!("{failure:?}"),
        }
        assert!(listener.stop().is_ok());
    }
}
