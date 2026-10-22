//! The Wayland clipboard and primary selection, through the data-control
//! protocol (`ext-data-control-v1`), with no external clipboard helper.
//!
//! Used by the clipboard-paste delivery rung:
//! 1. capture the user's clipboard and primary selection, every format;
//! 2. put the dictated text on both;
//! 3. after the paste chord, wait until the target has read our text;
//! 4. put the user's values back, unless something newer replaced ours.
//!
//! A Wayland selection is served by the client that set it, so one worker
//! thread owns the connection for the adapter's lifetime: it answers paste
//! requests for whatever Narrivy put on the clipboard, including the user's
//! restored value, until another application takes the selection.
//!
//! # Privacy
//! Clipboard contents are routing data only. They are never logged, never
//! rendered in `Debug`, and never leave this module except to the requesting
//! application through the compositor.
//!
//! # Bounds
//! - Every request to the worker has a reply bound.
//! - A snapshot reads at most [`SNAPSHOT_BYTES_CAP`] bytes and
//!   [`SNAPSHOT_MIME_CAP`] formats within [`SNAPSHOT_READ_BOUND`]. A larger
//!   or slower clipboard is not captured, and the caller refuses to replace
//!   it.
//! - Each answer to a paste request is written by a short-lived thread with
//!   its own deadline ([`SERVE_WRITE_BOUND`]); at most [`MAX_SERVING_WRITERS`]
//!   run at once.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, poll};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::pipe::{PipeFlags, pipe_with};
use wayland_client::backend::WaylandError;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
    ext_data_control_source_v1::{self, ExtDataControlSourceV1},
};

/// Largest clipboard Narrivy captures and puts back, all formats together.
pub const SNAPSHOT_BYTES_CAP: usize = 32 * 1024 * 1024;
/// Most formats captured from one selection.
pub const SNAPSHOT_MIME_CAP: usize = 24;
/// Time allowed to read a snapshot.
pub const SNAPSHOT_READ_BOUND: Duration = Duration::from_secs(2);
/// Time allowed to answer one paste request.
pub const SERVE_WRITE_BOUND: Duration = Duration::from_secs(3);
/// Concurrent paste answers.
pub const MAX_SERVING_WRITERS: usize = 8;
/// Bound on one request to the worker (beyond the snapshot read itself).
const REPLY_BOUND: Duration = Duration::from_secs(5);
/// How often the worker wakes to check for shutdown when idle.
const IDLE_WAKE: Duration = Duration::from_millis(250);

/// Formats Narrivy offers for dictated text.
const TEXT_MIME_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];

type Items = Arc<Vec<(String, Vec<u8>)>>;

/// The user's clipboard and primary selection, captured whole. `None` for a
/// selection that was empty. Never rendered or logged.
#[derive(Clone, Default)]
pub struct SelectionSnapshot {
    clipboard: Option<Items>,
    primary: Option<Items>,
}

impl std::fmt::Debug for SelectionSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // PRIVACY: content is never rendered.
        formatter.write_str("SelectionSnapshot([redacted])")
    }
}

/// Which selections Narrivy still owns from one write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ownership {
    pub clipboard: bool,
    pub primary: bool,
}

/// Shared facts about one source Narrivy set.
#[derive(Default)]
struct SourceFacts {
    cancelled: AtomicBool,
    served: AtomicU32,
}

/// Wakes callers waiting for a source to be read or replaced.
#[derive(Default)]
struct Signal {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl Signal {
    fn notify(&self) {
        if let Ok(mut generation) = self.generation.lock() {
            *generation = generation.wrapping_add(1);
        }
        self.changed.notify_all();
    }

    /// Wait until `done` holds or `bound` passes. Returns `done`'s last value.
    fn wait_for(&self, bound: Duration, done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + bound;
        let Ok(mut generation) = self.generation.lock() else {
            return done();
        };
        loop {
            if done() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            match self.changed.wait_timeout(generation, deadline - now) {
                Ok((guard, _)) => generation = guard,
                Err(_) => return done(),
            }
        }
    }
}

/// One write's sources.
#[derive(Clone)]
struct Written {
    clipboard: Arc<SourceFacts>,
    primary: Arc<SourceFacts>,
}

enum Request {
    Snapshot(SyncSender<Result<SelectionSnapshot, &'static str>>),
    Write {
        text: Vec<u8>,
        reply: SyncSender<Result<u64, &'static str>>,
    },
    Restore {
        serial: u64,
        snapshot: SelectionSnapshot,
        reply: SyncSender<Result<(), &'static str>>,
    },
    Shutdown,
}

/// Narrivy's handle on the clipboard and primary selection.
pub struct WaylandSelection {
    requests: Sender<Request>,
    wake: OwnedFd,
    signal: Arc<Signal>,
    written: Arc<Mutex<HashMap<u64, Written>>>,
    /// Why the worker stopped, once it has (content-free).
    stopped: Arc<Mutex<Option<&'static str>>>,
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for WaylandSelection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WaylandSelection")
    }
}

impl WaylandSelection {
    /// Connect to the compositor and start the worker. Fails with a stable
    /// code when there is no Wayland session or no data-control support.
    pub fn connect() -> Result<Self, &'static str> {
        let connection = Connection::connect_to_env().map_err(|_| "wayland_unavailable")?;
        let (globals, mut queue) = registry_queue_init::<WorkerState>(&connection)
            .map_err(|_| "wayland_registry_failed")?;
        let handle = queue.handle();
        let manager: ExtDataControlManagerV1 = globals
            .bind(&handle, 1..=1, ())
            .map_err(|_| "data_control_unavailable")?;
        let seat: wl_seat::WlSeat = globals
            .bind(&handle, 1..=9, ())
            .map_err(|_| "wayland_seat_unavailable")?;
        let device = manager.get_data_device(&seat, &handle, ());
        let signal = Arc::new(Signal::default());
        let written = Arc::new(Mutex::new(HashMap::new()));
        let mut state = WorkerState::new(Arc::clone(&signal));
        // Learn the current selections before the first request.
        queue
            .roundtrip(&mut state)
            .map_err(|_| "wayland_roundtrip_failed")?;
        let (wake_read, wake) =
            pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).map_err(|_| "pipe_failed")?;
        let (requests, receiver) = mpsc::channel();
        let worker_written = Arc::clone(&written);
        let stopped = Arc::new(Mutex::new(None));
        let worker_stopped = Arc::clone(&stopped);
        let worker = std::thread::Builder::new()
            .name("narrivy-clipboard".into())
            .spawn(move || {
                let mut worker = Worker {
                    connection,
                    queue,
                    manager,
                    device,
                    state,
                    written: worker_written,
                    next_serial: 1,
                };
                let reason = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker.run(&receiver, &wake_read)
                }))
                .unwrap_or("clipboard_worker_panicked");
                if let Ok(mut slot) = worker_stopped.lock() {
                    *slot = Some(reason);
                }
            })
            .map_err(|_| "clipboard_worker_unavailable")?;
        Ok(Self {
            requests,
            wake,
            signal,
            written,
            stopped,
            worker: Some(worker),
        })
    }

    /// Why the worker stopped, or `None` while it serves requests. A stopped
    /// selection answers every request with an error; connect again.
    #[must_use]
    pub fn stopped(&self) -> Option<&'static str> {
        self.stopped
            .lock()
            .map_or(Some("clipboard_worker_poisoned"), |slot| *slot)
    }

    fn ask<T>(&self, request: Request, reply: &Receiver<T>, bound: Duration) -> Option<T> {
        self.requests.send(request).ok()?;
        let _ = rustix::io::write(&self.wake, &[1]);
        reply.recv_timeout(bound).ok()
    }

    /// Capture both selections whole, or refuse with a stable code.
    pub fn snapshot(&self) -> Result<SelectionSnapshot, &'static str> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.ask(
            Request::Snapshot(reply),
            &receiver,
            SNAPSHOT_READ_BOUND + REPLY_BOUND,
        )
        .unwrap_or(Err("clipboard_worker_timeout"))
    }

    /// Put `text` on the clipboard and the primary selection. Returns the
    /// write's serial.
    pub fn write_text(&self, text: &str) -> Result<u64, &'static str> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.ask(
            Request::Write {
                text: text.as_bytes().to_vec(),
                reply,
            },
            &receiver,
            REPLY_BOUND,
        )
        .unwrap_or(Err("clipboard_worker_timeout"))
    }

    /// How many reads of what write `serial` put on either selection have
    /// completed so far.
    pub fn reads(&self, serial: u64) -> u32 {
        self.written(serial).map_or(0, |written| {
            written
                .clipboard
                .served
                .load(Ordering::Acquire)
                .saturating_add(written.primary.served.load(Ordering::Acquire))
        })
    }

    /// Wait at most `bound` until an application has read what write
    /// `serial` put on either selection, or something replaced it. Returns
    /// whether a read was observed.
    pub fn wait_until_read(&self, serial: u64, bound: Duration) -> bool {
        self.wait_until_read_after(serial, 0, bound)
    }

    /// Like [`Self::wait_until_read`], counting only reads beyond `baseline`
    /// (from [`Self::reads`]). A clipboard manager reads every new value at
    /// once; only a read after the paste chord shows the target pasted.
    pub fn wait_until_read_after(&self, serial: u64, baseline: u32, bound: Duration) -> bool {
        let Some(written) = self.written(serial) else {
            return false;
        };
        let read = || {
            written
                .clipboard
                .served
                .load(Ordering::Acquire)
                .saturating_add(written.primary.served.load(Ordering::Acquire))
                > baseline
        };
        let settled = || {
            read()
                || (written.clipboard.cancelled.load(Ordering::Acquire)
                    && written.primary.cancelled.load(Ordering::Acquire))
        };
        self.signal.wait_for(bound, settled);
        read()
    }

    /// Which selections still hold what write `serial` put there.
    pub fn ownership(&self, serial: u64) -> Ownership {
        self.written(serial).map_or(
            Ownership {
                clipboard: false,
                primary: false,
            },
            |written| Ownership {
                clipboard: !written.clipboard.cancelled.load(Ordering::Acquire),
                primary: !written.primary.cancelled.load(Ordering::Acquire),
            },
        )
    }

    /// Put the user's values back on each selection Narrivy still owns from
    /// write `serial`. A selection something else replaced is left alone.
    pub fn restore(&self, serial: u64, snapshot: &SelectionSnapshot) -> Result<(), &'static str> {
        let (reply, receiver) = mpsc::sync_channel(1);
        let result = self
            .ask(
                Request::Restore {
                    serial,
                    snapshot: snapshot.clone(),
                    reply,
                },
                &receiver,
                REPLY_BOUND,
            )
            .unwrap_or(Err("clipboard_worker_timeout"));
        if let Ok(mut written) = self.written.lock() {
            written.remove(&serial);
        }
        result
    }

    fn written(&self, serial: u64) -> Option<Written> {
        self.written.lock().ok()?.get(&serial).cloned()
    }
}

impl Drop for WaylandSelection {
    fn drop(&mut self) {
        let _ = self.requests.send(Request::Shutdown);
        let _ = rustix::io::write(&self.wake, &[1]);
        if let Some(worker) = self.worker.take() {
            // The worker checks for shutdown at least every IDLE_WAKE.
            let _ = worker.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// Data attached to a source Narrivy set.
struct SourceData {
    items: Items,
    facts: Arc<SourceFacts>,
}

struct WorkerState {
    signal: Arc<Signal>,
    /// Formats each live offer announced.
    offers: HashMap<wayland_client::backend::ObjectId, Vec<String>>,
    clipboard: Option<ExtDataControlOfferV1>,
    primary: Option<ExtDataControlOfferV1>,
    /// What Narrivy is serving on each selection, while it owns it.
    own_clipboard: Option<(Items, Arc<SourceFacts>)>,
    own_primary: Option<(Items, Arc<SourceFacts>)>,
    writers: Arc<AtomicUsize>,
    finished: bool,
}

impl WorkerState {
    fn new(signal: Arc<Signal>) -> Self {
        Self {
            signal,
            offers: HashMap::new(),
            clipboard: None,
            primary: None,
            own_clipboard: None,
            own_primary: None,
            writers: Arc::new(AtomicUsize::new(0)),
            finished: false,
        }
    }

    fn replace_offer(&mut self, primary: bool, offer: Option<ExtDataControlOfferV1>) {
        let slot = if primary {
            &mut self.primary
        } else {
            &mut self.clipboard
        };
        if let Some(old) = std::mem::replace(slot, offer) {
            let still_used = self.clipboard.as_ref().is_some_and(|o| o.id() == old.id())
                || self.primary.as_ref().is_some_and(|o| o.id() == old.id());
            if !still_used {
                self.offers.remove(&old.id());
                old.destroy();
            }
        }
    }
}

struct Worker {
    connection: Connection,
    queue: EventQueue<WorkerState>,
    manager: ExtDataControlManagerV1,
    device: ExtDataControlDeviceV1,
    state: WorkerState,
    written: Arc<Mutex<HashMap<u64, Written>>>,
    next_serial: u64,
}

impl Worker {
    /// Serve until shut down or the connection fails; returns why it stopped.
    fn run(&mut self, requests: &Receiver<Request>, wake: &OwnedFd) -> &'static str {
        loop {
            if self.queue.dispatch_pending(&mut self.state).is_err() {
                return "wayland_dispatch_failed";
            }
            if self.state.finished {
                return "data_control_finished";
            }
            if self.connection.flush().is_err() {
                return "wayland_flush_failed";
            }
            loop {
                match requests.try_recv() {
                    Ok(Request::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                        return "shut_down";
                    }
                    Ok(request) => self.handle(request),
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            let _ = self.connection.flush();
            let Some(guard) = self.queue.prepare_read() else {
                continue;
            };
            let readable = {
                let wayland_fd = guard.connection_fd();
                let mut fds = [
                    PollFd::new(&wayland_fd, PollFlags::IN),
                    PollFd::new(wake, PollFlags::IN),
                ];
                let timeout = rustix::time::Timespec::try_from(IDLE_WAKE).ok();
                match poll(&mut fds, timeout.as_ref()) {
                    Ok(_) => fds[0].revents().contains(PollFlags::IN),
                    Err(_) => false,
                }
            };
            if readable {
                match guard.read() {
                    Ok(_) => {}
                    // Readable but nothing whole to read yet (poll can wake
                    // spuriously): not a broken connection.
                    Err(WaylandError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => return "wayland_read_failed",
                }
            } else {
                drop(guard);
            }
            let mut drain = [0_u8; 64];
            while matches!(rustix::io::read(wake, &mut drain), Ok(n) if n > 0) {}
        }
    }

    fn handle(&mut self, request: Request) {
        match request {
            Request::Snapshot(reply) => {
                let _ = reply.send(self.snapshot());
            }
            Request::Write { text, reply } => {
                let _ = reply.send(Ok(self.write(text)));
            }
            Request::Restore {
                serial,
                snapshot,
                reply,
            } => {
                let _ = reply.send(self.restore(serial, &snapshot));
            }
            Request::Shutdown => {}
        }
    }

    fn snapshot(&mut self) -> Result<SelectionSnapshot, &'static str> {
        // Bring the selection state up to date first.
        self.queue
            .roundtrip(&mut self.state)
            .map_err(|_| "wayland_roundtrip_failed")?;
        let deadline = Instant::now() + SNAPSHOT_READ_BOUND;
        let mut budget = SNAPSHOT_BYTES_CAP;
        let clipboard = self.capture(false, deadline, &mut budget)?;
        let primary = self.capture(true, deadline, &mut budget)?;
        Ok(SelectionSnapshot { clipboard, primary })
    }

    /// Capture one selection: Narrivy's own value when it owns it, otherwise
    /// every format of the current offer.
    fn capture(
        &mut self,
        primary: bool,
        deadline: Instant,
        budget: &mut usize,
    ) -> Result<Option<Items>, &'static str> {
        let own = if primary {
            &self.state.own_primary
        } else {
            &self.state.own_clipboard
        };
        if let Some((items, _)) = own {
            return Ok(Some(Arc::clone(items)));
        }
        let offer = if primary {
            self.state.primary.clone()
        } else {
            self.state.clipboard.clone()
        };
        let Some(offer) = offer else {
            return Ok(None);
        };
        let mimes: Vec<String> = self
            .state
            .offers
            .get(&offer.id())
            .map(|mimes| mimes.iter().filter(|m| capturable(m)).cloned().collect())
            .unwrap_or_default();
        if mimes.len() > SNAPSHOT_MIME_CAP {
            return Err("clipboard_too_many_formats");
        }
        let mut items = Vec::with_capacity(mimes.len());
        for mime in mimes {
            let (read, write) =
                pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).map_err(|_| "pipe_failed")?;
            offer.receive(mime.clone(), write.as_fd());
            self.connection
                .flush()
                .map_err(|_| "wayland_flush_failed")?;
            drop(write);
            let bytes = read_bounded(&read, deadline, budget)?;
            items.push((mime, bytes));
        }
        Ok(Some(Arc::new(items)))
    }

    fn new_source(&mut self, items: &Items, facts: &Arc<SourceFacts>) -> ExtDataControlSourceV1 {
        let source = self.manager.create_data_source(
            &self.queue.handle(),
            SourceData {
                items: Arc::clone(items),
                facts: Arc::clone(facts),
            },
        );
        for (mime, _) in items.iter() {
            source.offer(mime.clone());
        }
        source
    }

    fn write(&mut self, text: Vec<u8>) -> u64 {
        let serial = self.next_serial;
        self.next_serial += 1;
        let items: Items = Arc::new(
            TEXT_MIME_TYPES
                .iter()
                .map(|mime| ((*mime).to_owned(), text.clone()))
                .collect(),
        );
        let written = Written {
            clipboard: Arc::new(SourceFacts::default()),
            primary: Arc::new(SourceFacts::default()),
        };
        if let Ok(mut map) = self.written.lock() {
            // Only recent writes are tracked; older ones are finished.
            if map.len() > 16 {
                map.clear();
            }
            map.insert(serial, written.clone());
        }
        let clipboard = self.new_source(&items, &written.clipboard);
        let primary = self.new_source(&items, &written.primary);
        self.device.set_selection(Some(&clipboard));
        self.device.set_primary_selection(Some(&primary));
        self.state.own_clipboard = Some((Arc::clone(&items), written.clipboard));
        self.state.own_primary = Some((items, written.primary));
        // Make sure the compositor has applied the new selection before the
        // caller sends a paste chord from another client.
        let _ = self.queue.roundtrip(&mut self.state);
        serial
    }

    fn restore(&mut self, serial: u64, snapshot: &SelectionSnapshot) -> Result<(), &'static str> {
        let written = self
            .written
            .lock()
            .ok()
            .and_then(|map| map.get(&serial).cloned());
        let Some(written) = written else {
            return Err("clipboard_write_unknown");
        };
        if !written.clipboard.cancelled.load(Ordering::Acquire) {
            match &snapshot.clipboard {
                Some(items) => {
                    let facts = Arc::new(SourceFacts::default());
                    let source = self.new_source(items, &facts);
                    self.device.set_selection(Some(&source));
                    self.state.own_clipboard = Some((Arc::clone(items), facts));
                }
                None => {
                    self.device.set_selection(None);
                    self.state.own_clipboard = None;
                }
            }
        }
        if !written.primary.cancelled.load(Ordering::Acquire) {
            match &snapshot.primary {
                Some(items) => {
                    let facts = Arc::new(SourceFacts::default());
                    let source = self.new_source(items, &facts);
                    self.device.set_primary_selection(Some(&source));
                    self.state.own_primary = Some((Arc::clone(items), facts));
                }
                None => {
                    self.device.set_primary_selection(None);
                    self.state.own_primary = None;
                }
            }
        }
        self.connection
            .flush()
            .map_err(|_| "wayland_flush_failed")?;
        Ok(())
    }
}

/// Formats worth capturing: MIME types, and the X11 text atoms. Other X11
/// atoms (`TARGETS`, `TIMESTAMP`, `SAVE_TARGETS`, …) are protocol requests,
/// not content.
fn capturable(mime: &str) -> bool {
    mime.contains('/') || matches!(mime, "UTF8_STRING" | "STRING" | "TEXT" | "COMPOUND_TEXT")
}

/// Read a pipe to its end, within `deadline` and `budget`.
fn read_bounded(
    fd: &OwnedFd,
    deadline: Instant,
    budget: &mut usize,
) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        match rustix::io::read(fd, &mut chunk) {
            Ok(0) => return Ok(bytes),
            Ok(count) => {
                if count > *budget {
                    return Err("clipboard_too_large");
                }
                *budget -= count;
                bytes.extend_from_slice(&chunk[..count]);
            }
            Err(rustix::io::Errno::AGAIN) => {
                let now = Instant::now();
                if now >= deadline {
                    return Err("clipboard_read_timeout");
                }
                let timeout = rustix::time::Timespec::try_from(deadline - now).ok();
                let mut fds = [PollFd::new(fd, PollFlags::IN)];
                if poll(&mut fds, timeout.as_ref()).is_err() {
                    return Err("clipboard_read_failed");
                }
            }
            Err(rustix::io::Errno::INTR) => {}
            Err(_) => return Err("clipboard_read_failed"),
        }
    }
}

/// Answer one paste request on a short-lived thread, bounded by
/// [`SERVE_WRITE_BOUND`]. Records the read and wakes waiters when done.
fn serve(
    fd: OwnedFd,
    bytes: Items,
    mime: &str,
    facts: Arc<SourceFacts>,
    writers: &Arc<AtomicUsize>,
    signal: &Arc<Signal>,
) {
    let Some((_, data)) = bytes.iter().find(|(offered, _)| offered == mime) else {
        return;
    };
    if writers.fetch_add(1, Ordering::AcqRel) >= MAX_SERVING_WRITERS {
        writers.fetch_sub(1, Ordering::AcqRel);
        return;
    }
    let data = data.clone();
    let writers = Arc::clone(writers);
    let signal = Arc::clone(signal);
    let spawned = std::thread::Builder::new()
        .name("narrivy-clipboard-serve".into())
        .spawn(move || {
            let complete = write_bounded(&fd, &data, Instant::now() + SERVE_WRITE_BOUND).is_ok();
            drop(fd);
            if complete {
                facts.served.fetch_add(1, Ordering::AcqRel);
            }
            writers.fetch_sub(1, Ordering::AcqRel);
            signal.notify();
        });
    if spawned.is_err() {
        // The spawn closure (and its counter release) never ran.
        // `writers` was moved into it, so nothing more to undo here.
    }
}

fn write_bounded(fd: &OwnedFd, mut data: &[u8], deadline: Instant) -> io::Result<()> {
    let flags = fcntl_getfl(fd)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK)?;
    while !data.is_empty() {
        match rustix::io::write(fd, data) {
            Ok(count) => data = &data[count..],
            Err(rustix::io::Errno::AGAIN) => {
                let now = Instant::now();
                if now >= deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                let timeout = rustix::time::Timespec::try_from(deadline - now).ok();
                let mut fds = [PollFd::new(fd, PollFlags::OUT)];
                poll(&mut fds, timeout.as_ref())?;
            }
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Protocol dispatch
// ---------------------------------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WorkerState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for WorkerState {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlManagerV1, ()> for WorkerState {
    fn event(
        _: &mut Self,
        _: &ExtDataControlManagerV1,
        _: <ExtDataControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for WorkerState {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
            }
            ext_data_control_device_v1::Event::Selection { id } => state.replace_offer(false, id),
            ext_data_control_device_v1::Event::PrimarySelection { id } => {
                state.replace_offer(true, id);
            }
            ext_data_control_device_v1::Event::Finished => state.finished = true,
            _ => {}
        }
    }

    event_created_child!(WorkerState, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for WorkerState {
    fn event(
        state: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Some(mimes) = state.offers.get_mut(&offer.id())
            && mimes.len() < 256
        {
            mimes.push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, SourceData> for WorkerState {
    fn event(
        state: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        data: &SourceData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                serve(
                    fd,
                    Arc::clone(&data.items),
                    &mime_type,
                    Arc::clone(&data.facts),
                    &state.writers,
                    &state.signal,
                );
            }
            ext_data_control_source_v1::Event::Cancelled => {
                data.facts.cancelled.store(true, Ordering::Release);
                for own in [&mut state.own_clipboard, &mut state.own_primary] {
                    if own
                        .as_ref()
                        .is_some_and(|(_, facts)| Arc::ptr_eq(facts, &data.facts))
                    {
                        *own = None;
                    }
                }
                source.destroy();
                state.signal.notify();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_content_formats_are_captured() {
        assert!(capturable("text/plain;charset=utf-8"));
        assert!(capturable("image/png"));
        assert!(capturable("UTF8_STRING"));
        assert!(!capturable("TARGETS"));
        assert!(!capturable("SAVE_TARGETS"));
        assert!(!capturable("TIMESTAMP"));
    }

    #[test]
    fn a_pipe_is_read_within_its_budget_and_refused_beyond_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
        rustix::io::write(&write, b"hello")?;
        drop(write);
        let mut budget = 16;
        assert_eq!(
            read_bounded(&read, Instant::now() + Duration::from_secs(1), &mut budget)?,
            b"hello"
        );
        assert_eq!(budget, 11);

        let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
        rustix::io::write(&write, b"too long")?;
        drop(write);
        let mut budget = 3;
        assert_eq!(
            read_bounded(&read, Instant::now() + Duration::from_secs(1), &mut budget),
            Err("clipboard_too_large")
        );

        // A writer that never finishes is cut off at the deadline.
        let (read, _write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
        let mut budget = 16;
        assert_eq!(
            read_bounded(
                &read,
                Instant::now() + Duration::from_millis(50),
                &mut budget
            ),
            Err("clipboard_read_timeout")
        );
        Ok(())
    }

    #[test]
    fn snapshots_never_show_their_content() {
        let snapshot = SelectionSnapshot {
            clipboard: Some(Arc::new(vec![("text/plain".into(), b"secret".to_vec())])),
            primary: None,
        };
        assert_eq!(format!("{snapshot:?}"), "SelectionSnapshot([redacted])");
    }

    /// Writes to the real clipboard, so it is not part of the suite:
    /// `cargo test -p narrivy-platform-linux wayland_selection -- --ignored`.
    #[test]
    #[ignore = "needs a Wayland session with data-control"]
    fn a_long_lived_connection_still_offers_what_it_writes()
    -> Result<(), Box<dyn std::error::Error>> {
        let ours = WaylandSelection::connect()?;
        let before = ours.snapshot()?;
        for round in 0..3 {
            std::thread::sleep(Duration::from_secs(4));
            let text = format!("narrivy long-lived test {round}");
            let serial = ours.write_text(&text)?;
            std::thread::sleep(Duration::from_millis(300));
            // A clipboard manager, if one runs, has read it by now.
            let early_reads = ours.reads(serial);
            let reader = WaylandSelection::connect()?;
            let seen = reader.snapshot()?;
            let plain = seen.clipboard.as_ref().and_then(|items| {
                items
                    .iter()
                    .find(|(m, _)| m.starts_with("text/plain"))
                    .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
            });
            eprintln!(
                "round {round}: early reads={early_reads} other client sees ours={} read after={} stopped={:?}",
                plain.as_deref() == Some(text.as_str()),
                ours.wait_until_read_after(serial, early_reads, Duration::from_millis(500)),
                ours.stopped()
            );
            assert_eq!(plain.as_deref(), Some(text.as_str()));
            ours.restore(serial, &before)?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "uses the real Wayland clipboard"]
    fn round_trip_on_the_real_clipboard() -> Result<(), Box<dyn std::error::Error>> {
        let selection = WaylandSelection::connect()?;
        let before = selection.snapshot()?;
        let serial = selection.write_text("narrivy clipboard test")?;
        assert!(selection.ownership(serial).clipboard);
        selection.restore(serial, &before)?;
        let after = selection.snapshot()?;
        let text = |snapshot: &SelectionSnapshot| {
            snapshot.clipboard.as_ref().and_then(|items| {
                items
                    .iter()
                    .find(|(m, _)| m.starts_with("text/plain"))
                    .map(|(_, b)| b.clone())
            })
        };
        assert_eq!(text(&before), text(&after));
        Ok(())
    }
}
