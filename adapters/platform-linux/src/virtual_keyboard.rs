//! Typing through the Wayland virtual-keyboard protocol
//! (`zwp_virtual_keyboard_v1`), in process, with no external helper.
//!
//! The text gets its own keymap: every character sits on its own key with a
//! single level, so no modifier is ever pressed. All key events are written
//! at once and the compositor delivers them in order; one sync at the end
//! confirms it has taken them all. `wtype` waits for the compositor after
//! every key, which costs about 5 ms a character.
//!
//! Only key codes of plain character keys are used. Applications read the
//! key code as well as the keysym, so a character placed on the code of
//! Ctrl, Shift or Esc is treated as that key and lost. When a text needs more
//! distinct characters than there are such codes, a fresh keymap is sent
//! between runs.
//!
//! # Privacy
//! The text only goes to the compositor. It is never logged and never
//! rendered in an error.
//!
//! # Bounds
//! Connecting, the setup sync and the closing sync share one deadline
//! ([`TYPING_BOUND`]).

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::time::{Duration, Instant};

use narrivy_types::SessionCancellation;
use rustix::event::{PollFd, PollFlags, poll};
use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::{wl_callback, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

/// Time allowed for one text, from connecting to the closing sync.
pub const TYPING_BOUND: Duration = Duration::from_secs(5);

/// `wl_keyboard.keymap_format.xkb_v1`.
const KEYMAP_FORMAT_XKB_V1: u32 = 1;
const RETURN_CODE: u32 = 28;
const TAB_CODE: u32 = 15;
const SPACE_CODE: u32 = 57;

/// What happened to one text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualKeyboardTyping {
    /// Every key was sent and the compositor confirmed it took them all.
    /// Whether the focused application kept the text is not known.
    Typed,
    /// Nothing was sent; another way of typing may be tried.
    NotDispatched(&'static str),
    /// Cancelled before any key was sent.
    CancelledBeforeDispatch,
    /// Keys may have been sent; trying another way could type them twice.
    Failed(&'static str),
}

/// Types `text` into the focused window.
#[must_use]
pub fn type_text(text: &str, cancellation: &SessionCancellation) -> VirtualKeyboardTyping {
    if cancellation.is_cancelled() {
        return VirtualKeyboardTyping::CancelledBeforeDispatch;
    }
    let Some(runs) = plan(text) else {
        return VirtualKeyboardTyping::NotDispatched("virtual_keyboard_unsupported_character");
    };
    let deadline = Instant::now() + TYPING_BOUND;
    let mut session = match Session::connect(deadline) {
        Ok(session) => session,
        Err(reason) => return VirtualKeyboardTyping::NotDispatched(reason),
    };
    if cancellation.is_cancelled() {
        return VirtualKeyboardTyping::CancelledBeforeDispatch;
    }
    let mut keymaps = Vec::with_capacity(runs.len());
    for run in &runs {
        match keymap_fd(&run.keymap()) {
            Ok(fd) => keymaps.push(fd),
            Err(()) => {
                return VirtualKeyboardTyping::NotDispatched("virtual_keyboard_keymap_failed");
            }
        }
    }
    // From the first keymap on, keys may reach the focused window.
    match session.send(&runs, &keymaps, deadline) {
        Ok(()) => VirtualKeyboardTyping::Typed,
        Err(reason) => VirtualKeyboardTyping::Failed(reason),
    }
}

/// Part of a text that fits one keymap.
#[derive(Debug, PartialEq, Eq)]
struct Run {
    /// (character, evdev code), in order of first use.
    keys: Vec<(char, u32)>,
    /// Evdev code of each character of the run, in typing order.
    codes: Vec<u32>,
}

impl Run {
    fn keymap(&self) -> String {
        let mut keycodes = String::new();
        let mut symbols = String::new();
        for (character, code) in &self.keys {
            let xkb = code + 8;
            keycodes.push_str(&format!("<K{xkb}> = {xkb};\n"));
            symbols.push_str(&format!(
                "key <K{xkb}> {{ [ {} ] }};\n",
                keysym_name(*character)
            ));
        }
        format!(
            "xkb_keymap {{\n\
             xkb_keycodes \"narrivy\" {{ minimum = 8; maximum = 255;\n{keycodes}}};\n\
             xkb_types \"narrivy\" {{ include \"complete\" }};\n\
             xkb_compatibility \"narrivy\" {{ include \"complete\" }};\n\
             xkb_symbols \"narrivy\" {{\n{symbols}}};\n}};\n"
        )
    }
}

/// Evdev codes of the plain character keys of a standard keyboard: the
/// digit row, the three letter rows and the extra ISO key.
fn character_codes() -> impl Iterator<Item = u32> {
    (2..=13)
        .chain(16..=27)
        .chain(30..=41)
        .chain(43..=53)
        .chain([86])
}

fn fixed_code(character: char) -> Option<u32> {
    match character {
        '\n' => Some(RETURN_CODE),
        '\t' => Some(TAB_CODE),
        ' ' => Some(SPACE_CODE),
        _ => None,
    }
}

fn keysym_name(character: char) -> String {
    match character {
        '\n' => "Return".to_owned(),
        '\t' => "Tab".to_owned(),
        _ => format!("U{:04X}", u32::from(character)),
    }
}

/// Splits `text` into runs. `None` when it holds a control character other
/// than newline and tab, which a single-level key cannot carry.
fn plan(text: &str) -> Option<Vec<Run>> {
    let pool: Vec<u32> = character_codes().collect();
    let mut runs = Vec::new();
    let mut assigned: HashMap<char, u32> = HashMap::new();
    let mut run = Run {
        keys: Vec::new(),
        codes: Vec::new(),
    };
    let mut used = 0;
    for character in text.chars() {
        if character.is_control() && fixed_code(character).is_none() {
            return None;
        }
        let code = match assigned.get(&character) {
            Some(code) => *code,
            None => {
                let code = match fixed_code(character) {
                    Some(code) => code,
                    None => {
                        if used == pool.len() {
                            runs.push(std::mem::replace(
                                &mut run,
                                Run {
                                    keys: Vec::new(),
                                    codes: Vec::new(),
                                },
                            ));
                            assigned.clear();
                            used = 0;
                        }
                        used += 1;
                        pool[used - 1]
                    }
                };
                assigned.insert(character, code);
                run.keys.push((character, code));
                code
            }
        };
        run.codes.push(code);
    }
    if !run.codes.is_empty() {
        runs.push(run);
    }
    Some(runs)
}

fn keymap_fd(keymap: &str) -> Result<OwnedFd, ()> {
    let fd = memfd_create("narrivy-keymap", MemfdFlags::CLOEXEC).map_err(|_| ())?;
    let mut bytes = keymap.as_bytes().to_vec();
    bytes.push(0);
    let mut written = 0;
    while written < bytes.len() {
        match rustix::io::write(&fd, &bytes[written..]) {
            Ok(0) | Err(_) => return Err(()),
            Ok(n) => written += n,
        }
    }
    Ok(fd)
}

#[derive(Default)]
struct State {
    seat: Option<(u32, u32)>,
    manager: Option<u32>,
    synced: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "wl_seat" && state.seat.is_none() {
                state.seat = Some((name, version.min(7)));
            } else if interface == "zwp_virtual_keyboard_manager_v1" {
                state.manager = Some(name);
            }
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced = true;
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ZwpVirtualKeyboardV1);

struct Session {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    keyboard: ZwpVirtualKeyboardV1,
}

impl Session {
    fn connect(deadline: Instant) -> Result<Self, &'static str> {
        let connection = Connection::connect_to_env().map_err(|_| "wayland_unavailable")?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        let registry = connection.display().get_registry(&handle, ());
        let mut state = State::default();
        sync(&connection, &mut queue, &mut state, deadline)?;
        let (Some((seat_name, seat_version)), Some(manager_name)) = (state.seat, state.manager)
        else {
            return Err(if state.seat.is_none() {
                "wayland_seat_unavailable"
            } else {
                "virtual_keyboard_unavailable"
            });
        };
        let seat: wl_seat::WlSeat = registry.bind(seat_name, seat_version, &handle, ());
        let manager: ZwpVirtualKeyboardManagerV1 = registry.bind(manager_name, 1, &handle, ());
        let keyboard = manager.create_virtual_keyboard(&seat, &handle, ());
        // A compositor that refuses the keyboard says so here, before any key.
        sync(&connection, &mut queue, &mut state, deadline)?;
        Ok(Self {
            connection,
            queue,
            state,
            keyboard,
        })
    }

    fn send(
        &mut self,
        runs: &[Run],
        keymaps: &[OwnedFd],
        deadline: Instant,
    ) -> Result<(), &'static str> {
        let mut time = 0_u32;
        let mut queued = 0_usize;
        for (run, keymap) in runs.iter().zip(keymaps) {
            let size = u32::try_from(run.keymap().len() + 1)
                .map_err(|_| "virtual_keyboard_keymap_failed")?;
            self.keyboard
                .keymap(KEYMAP_FORMAT_XKB_V1, keymap.as_fd(), size);
            // The protocol requires the keymap before any modifier state.
            self.keyboard.modifiers(0, 0, 0, 0);
            for code in &run.codes {
                self.keyboard.key(time, *code, 1);
                self.keyboard.key(time, *code, 0);
                time = time.wrapping_add(1);
                queued += 1;
                // Keep the outgoing buffer small; this never waits for replies.
                if queued.is_multiple_of(64) {
                    flush(&self.connection, deadline)?;
                }
            }
        }
        sync(&self.connection, &mut self.queue, &mut self.state, deadline)?;
        self.keyboard.destroy();
        let _ = self.connection.flush();
        Ok(())
    }
}

/// Writes queued requests, waiting while the socket is full.
fn flush(connection: &Connection, deadline: Instant) -> Result<(), &'static str> {
    loop {
        match connection.flush() {
            Ok(()) => return Ok(()),
            Err(WaylandError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                let backend = connection.backend();
                let fd = backend.poll_fd();
                wait(&fd, PollFlags::OUT, deadline)?;
            }
            Err(_) => return Err("wayland_flush_failed"),
        }
    }
}

/// Round trip with a deadline: everything sent before has been processed
/// once the compositor answers.
fn sync(
    connection: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), &'static str> {
    state.synced = false;
    let _callback = connection.display().sync(&queue.handle(), ());
    loop {
        queue
            .dispatch_pending(state)
            .map_err(|_| "wayland_dispatch_failed")?;
        if state.synced {
            return Ok(());
        }
        flush(connection, deadline)?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        wait(&guard.connection_fd(), PollFlags::IN, deadline)?;
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => return Err("wayland_read_failed"),
        }
    }
}

fn wait(fd: &impl AsFd, flags: PollFlags, deadline: Instant) -> Result<(), &'static str> {
    let now = Instant::now();
    if now >= deadline {
        return Err("virtual_keyboard_timeout");
    }
    let timeout = rustix::time::Timespec::try_from(deadline - now).ok();
    let mut fds = [PollFd::new(fd, flags)];
    match poll(&mut fds, timeout.as_ref()) {
        Ok(0) => Err("virtual_keyboard_timeout"),
        Ok(_) => Ok(()),
        Err(rustix::io::Errno::INTR) => Ok(()),
        Err(_) => Err("wayland_poll_failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFIER_AND_SPECIAL_CODES: [u32; 9] = [1, 14, 29, 42, 54, 56, 58, 69, 97];

    #[test]
    fn characters_never_land_on_modifier_or_special_keys() -> Result<(), &'static str> {
        let text: String = ('!'..='~').chain("éàüß—€".chars()).collect();
        let runs = plan(&text).ok_or("plan refused printable text")?;
        for run in &runs {
            for (_, code) in &run.keys {
                if MODIFIER_AND_SPECIAL_CODES.contains(code) {
                    return Err("a character landed on a modifier or special key");
                }
                if *code == RETURN_CODE || *code == TAB_CODE || *code == SPACE_CODE {
                    return Err("a character took the Return, Tab or Space key");
                }
                if !character_codes().any(|plain| plain == *code) {
                    return Err("a character landed outside the plain character keys");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn a_text_with_many_distinct_characters_is_split_and_kept_in_order() -> Result<(), &'static str>
    {
        let text: String = ('!'..='~').collect();
        let runs = plan(&text).ok_or("plan refused printable text")?;
        if runs.len() < 2 {
            return Err("94 distinct characters should need more than one keymap");
        }
        let mut typed = String::new();
        for run in &runs {
            let by_code: HashMap<u32, char> =
                run.keys.iter().map(|(c, code)| (*code, *c)).collect();
            for code in &run.codes {
                typed.push(*by_code.get(code).ok_or("a code missing from its keymap")?);
            }
        }
        if typed != text {
            return Err("the runs do not type the text back in order");
        }
        Ok(())
    }

    #[test]
    fn newline_tab_and_space_use_their_real_keys() -> Result<(), &'static str> {
        let runs = plan("a b\tc\n").ok_or("plan refused the text")?;
        let run = runs.first().ok_or("no run")?;
        if run.codes != vec![2, SPACE_CODE, 3, TAB_CODE, 4, RETURN_CODE] {
            return Err("unexpected key codes");
        }
        let keymap = run.keymap();
        if !keymap.contains("[ Return ]")
            || !keymap.contains("[ Tab ]")
            || !keymap.contains("[ U0020 ]")
        {
            return Err("keymap misses Return, Tab or space");
        }
        Ok(())
    }

    #[test]
    fn other_control_characters_are_left_to_another_typing_method() {
        assert_eq!(plan("one\rtwo"), None);
        assert_eq!(plan("bell\u{7}"), None);
    }

    #[test]
    fn an_empty_text_needs_no_keymap() {
        assert_eq!(plan(""), Some(Vec::new()));
    }

    #[test]
    fn non_ascii_characters_get_unicode_keysyms() -> Result<(), &'static str> {
        let runs = plan("café").ok_or("plan refused the text")?;
        let keymap = runs.first().ok_or("no run")?.keymap();
        if !keymap.contains("[ U00E9 ]") {
            return Err("é is not on a Unicode keysym");
        }
        Ok(())
    }
}
