//! Minimal, actor-owned EI sender for one `ei_text.utf8` device frame.
//!
//! This module deliberately bypasses Reis' convenience handshake and event
//! iterator. Narrivy advertises only the six interfaces required for a text
//! sender and owns readable/writable progress under the caller's absolute
//! deadline. `REIS_DEBUG` is rejected before a context is created because its
//! wire diagnostics include payload arguments.

use std::collections::HashMap;
use std::env;
use std::io;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use reis::ei;
use reis::handshake::HandshakeResp;
use reis::{Interface, PendingRequestResult};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
use rustix::time::{ClockId, clock_gettime};

use crate::portal_eis::{
    PortalCancellation, PortalEisTextFailure, PortalEisTextPayload, PortalEisTextSubmission,
    PortalSessionFailure,
};

const POLL_SLICE: Duration = Duration::from_millis(5);
const REQUIRED_INTERFACES: [(&str, u32); 6] = [
    ("ei_callback", 1),
    ("ei_connection", 1),
    ("ei_seat", 1),
    ("ei_device", 1),
    ("ei_pingpong", 1),
    ("ei_text", 1),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitFailure {
    Cancelled,
    TimedOut,
    Transport,
}

impl WaitFailure {
    const fn session_failure(self) -> PortalSessionFailure {
        match self {
            Self::Cancelled => PortalSessionFailure::Cancelled,
            Self::TimedOut => PortalSessionFailure::TimedOut,
            Self::Transport => PortalSessionFailure::TransportFailed,
        }
    }
}

/// A connected sender and its single currently-resumed text device.
///
/// The type intentionally has no `Debug` implementation: protocol objects and
/// buffered requests must never reach ordinary diagnostics.
pub(super) struct PortalEisSender {
    context: ei::Context,
    _connection: ei::Connection,
    last_serial: u32,
    seats: HashMap<ei::Seat, SeatState>,
    devices: HashMap<ei::Device, DeviceState>,
    text_device: Option<ei::Device>,
    text_device_resumed: bool,
    sequence: u32,
}

#[derive(Default)]
struct SeatState {
    text_mask: Option<u64>,
    setup_complete: bool,
}

#[derive(Default)]
struct DeviceState {
    text: Option<ei::Text>,
    device_type_seen: bool,
    setup_complete: bool,
}

impl PortalEisSender {
    pub(super) fn connect(
        stream: UnixStream,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<Self, PortalSessionFailure> {
        // Reis snapshots this variable in `Context::new` and prints every wire
        // argument when it is nonempty. Reject any declaration, including an
        // empty one, so configuration cannot silently weaken content privacy.
        Self::connect_with_debug_guard(
            stream,
            deadline,
            cancellation,
            env::var_os("REIS_DEBUG").is_some(),
        )
    }

    fn connect_with_debug_guard(
        stream: UnixStream,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
        debug_declared: bool,
    ) -> Result<Self, PortalSessionFailure> {
        if debug_declared {
            return Err(PortalSessionFailure::TransportFailed);
        }

        let context =
            ei::Context::new(stream).map_err(|_| PortalSessionFailure::TransportFailed)?;
        let handshake = minimal_handshake(&context, deadline, cancellation)
            .map_err(WaitFailure::session_failure)?;
        let mut sender = Self {
            context,
            _connection: handshake.connection,
            last_serial: handshake.serial,
            seats: HashMap::new(),
            devices: HashMap::new(),
            text_device: None,
            text_device_resumed: false,
            sequence: 1,
        };
        sender
            .wait_until_ready(deadline, cancellation)
            .map_err(WaitFailure::session_failure)?;
        Ok(sender)
    }

    /// Poll protocol lifecycle without blocking. `Ok(true)` means that the
    /// sender device or connection was revoked and must no longer be used.
    pub(super) fn poll_revoked(&mut self) -> Result<bool, PortalSessionFailure> {
        let flags = poll_now(&self.context).map_err(|_| PortalSessionFailure::TransportFailed)?;
        if flags.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) {
            return Ok(true);
        }
        if flags.contains(PollFlags::IN) {
            match self.context.read() {
                Ok(_) => self
                    .drain_events()
                    .map_err(|_| PortalSessionFailure::TransportFailed)?,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return Ok(true),
            }
            self.context
                .flush()
                .map_err(|_| PortalSessionFailure::TransportFailed)?;
        }
        Ok(!self.text_device_resumed || self.text_device.is_none())
    }

    /// Submit one frame. The caller retains the sender only after a complete
    /// local handoff; every uncertain result drops the context and its buffer.
    pub(super) fn submit(
        mut self,
        payload: PortalEisTextPayload,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> (Option<Self>, PortalEisTextSubmission) {
        if cancellation.is_cancelled() {
            return (Some(self), PortalEisTextSubmission::CancelledBeforeDispatch);
        }
        if Instant::now() >= deadline {
            return (
                Some(self),
                PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::TimedOut),
            );
        }

        match poll_now(&self.context) {
            Ok(flags) if flags.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) => {
                return (
                    None,
                    PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::Revoked),
                );
            }
            Ok(flags) if flags.contains(PollFlags::IN) => {
                if self.context.read().is_err() || self.drain_events().is_err() {
                    return (
                        None,
                        PortalEisTextSubmission::RejectedBeforeDispatch(
                            PortalEisTextFailure::Revoked,
                        ),
                    );
                }
            }
            Ok(_) => {}
            Err(_) => {
                return (
                    None,
                    PortalEisTextSubmission::RejectedBeforeDispatch(
                        PortalEisTextFailure::TransportFailed,
                    ),
                );
            }
        }

        let Some(device) = self
            .text_device
            .clone()
            .filter(|_| self.text_device_resumed)
        else {
            return (
                None,
                PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::Revoked),
            );
        };
        let Some(text) = self
            .devices
            .get(&device)
            .and_then(|state| state.text.clone())
        else {
            return (
                None,
                PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::Revoked),
            );
        };

        // This is the last clean-cancellation point. Once requests are queued,
        // the first flush is attempted immediately; any incomplete handoff is
        // conservatively dispatch-uncertain because Reis does not expose the
        // number of bytes accepted by its internal nonblocking flush.
        if cancellation.is_cancelled() {
            return (Some(self), PortalEisTextSubmission::CancelledBeforeDispatch);
        }
        if Instant::now() >= deadline {
            return (
                Some(self),
                PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::TimedOut),
            );
        }

        let serial = self.last_serial;
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        device.start_emulating(serial, sequence);
        text.utf8(payload.as_str());
        device.frame(serial, monotonic_micros());
        device.stop_emulating(serial);

        loop {
            match self.context.flush() {
                Ok(()) => {
                    return (Some(self), PortalEisTextSubmission::SubmittedUnverified);
                }
                Err(Errno::AGAIN) => {}
                Err(_) => {
                    return (
                        None,
                        PortalEisTextSubmission::DispatchUncertain(
                            PortalEisTextFailure::TransportFailed,
                        ),
                    );
                }
            }

            if cancellation.is_cancelled() {
                return (
                    None,
                    PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::Cancelled),
                );
            }
            if Instant::now() >= deadline {
                return (
                    None,
                    PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::TimedOut),
                );
            }
            if wait_for(&self.context, PollFlags::OUT, deadline, cancellation).is_err() {
                let failure = if cancellation.is_cancelled() {
                    PortalEisTextFailure::Cancelled
                } else if Instant::now() >= deadline {
                    PortalEisTextFailure::TimedOut
                } else {
                    PortalEisTextFailure::TransportFailed
                };
                return (None, PortalEisTextSubmission::DispatchUncertain(failure));
            }
        }
    }

    fn wait_until_ready(
        &mut self,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<(), WaitFailure> {
        loop {
            self.drain_events()?;
            flush_control(&self.context, deadline, cancellation)?;
            if self.text_device.is_some() && self.text_device_resumed {
                return Ok(());
            }
            wait_for(&self.context, PollFlags::IN, deadline, cancellation)?;
            self.context.read().map_err(|_| WaitFailure::Transport)?;
        }
    }

    #[allow(clippy::too_many_lines)]
    fn drain_events(&mut self) -> Result<(), WaitFailure> {
        while let Some(result) = self.context.pending_event() {
            let event = match result {
                PendingRequestResult::Request(event) => event,
                PendingRequestResult::ParseError(_) | PendingRequestResult::InvalidObject(_) => {
                    return Err(WaitFailure::Transport);
                }
            };
            match event {
                ei::Event::Handshake(_, _) => return Err(WaitFailure::Transport),
                ei::Event::Connection(_, event) => match event {
                    ei::connection::Event::Disconnected { last_serial, .. } => {
                        self.last_serial = last_serial;
                        return Err(WaitFailure::Transport);
                    }
                    ei::connection::Event::Seat { seat } => {
                        if self.seats.insert(seat, SeatState::default()).is_some() {
                            return Err(WaitFailure::Transport);
                        }
                    }
                    ei::connection::Event::InvalidObject { last_serial, .. } => {
                        self.last_serial = last_serial;
                        return Err(WaitFailure::Transport);
                    }
                    ei::connection::Event::Ping { ping } => ping.done(0),
                    _ => return Err(WaitFailure::Transport),
                },
                ei::Event::Seat(seat, event) => {
                    let state = self.seats.get_mut(&seat).ok_or(WaitFailure::Transport)?;
                    match event {
                        ei::seat::Event::Destroyed { serial } => {
                            self.last_serial = serial;
                            self.seats.remove(&seat);
                            self.devices.clear();
                            self.text_device = None;
                            self.text_device_resumed = false;
                        }
                        ei::seat::Event::Name { .. } if !state.setup_complete => {}
                        ei::seat::Event::Capability { mask, interface }
                            if !state.setup_complete && interface == ei::Text::NAME =>
                        {
                            if state.text_mask.replace(mask).is_some() {
                                return Err(WaitFailure::Transport);
                            }
                        }
                        ei::seat::Event::Capability { .. } => {
                            return Err(WaitFailure::Transport);
                        }
                        ei::seat::Event::Done if !state.setup_complete => {
                            state.setup_complete = true;
                            let mask = state.text_mask.ok_or(WaitFailure::Transport)?;
                            seat.bind(mask);
                        }
                        ei::seat::Event::Device { device } if state.setup_complete => {
                            if self
                                .devices
                                .insert(device, DeviceState::default())
                                .is_some()
                            {
                                return Err(WaitFailure::Transport);
                            }
                        }
                        _ => return Err(WaitFailure::Transport),
                    }
                }
                ei::Event::Device(device, event) => {
                    let state = self
                        .devices
                        .get_mut(&device)
                        .ok_or(WaitFailure::Transport)?;
                    match event {
                        ei::device::Event::Destroyed { serial } => {
                            self.last_serial = serial;
                            self.devices.remove(&device);
                            if self.text_device.as_ref() == Some(&device) {
                                self.text_device = None;
                                self.text_device_resumed = false;
                            }
                        }
                        ei::device::Event::Name { .. }
                        | ei::device::Event::Dimensions { .. }
                        | ei::device::Event::Region { .. }
                        | ei::device::Event::RegionMappingId { .. }
                            if !state.setup_complete => {}
                        ei::device::Event::DeviceType { .. } if !state.setup_complete => {
                            if state.device_type_seen {
                                return Err(WaitFailure::Transport);
                            }
                            state.device_type_seen = true;
                        }
                        ei::device::Event::Interface { object } if !state.setup_complete => {
                            if object.interface() != ei::Text::NAME || state.text.is_some() {
                                return Err(WaitFailure::Transport);
                            }
                            state.text = object.downcast::<ei::Text>();
                            if state.text.is_none() {
                                return Err(WaitFailure::Transport);
                            }
                        }
                        ei::device::Event::Done if !state.setup_complete => {
                            if !state.device_type_seen || state.text.is_none() {
                                return Err(WaitFailure::Transport);
                            }
                            state.setup_complete = true;
                            if self.text_device.is_none() {
                                self.text_device = Some(device.clone());
                                self.text_device_resumed = false;
                            }
                        }
                        ei::device::Event::Resumed { serial } if state.setup_complete => {
                            self.last_serial = serial;
                            if self.text_device.as_ref() == Some(&device) {
                                self.text_device_resumed = true;
                            }
                        }
                        ei::device::Event::Paused { serial } if state.setup_complete => {
                            self.last_serial = serial;
                            if self.text_device.as_ref() == Some(&device) {
                                self.text_device_resumed = false;
                            }
                        }
                        // These are receiver-only events and therefore protocol
                        // violations for Narrivy's sender context.
                        ei::device::Event::StartEmulating { .. }
                        | ei::device::Event::StopEmulating { .. }
                        | ei::device::Event::Frame { .. } => {
                            return Err(WaitFailure::Transport);
                        }
                        _ => return Err(WaitFailure::Transport),
                    }
                }
                // Narrivy creates no callback objects. Any callback event is
                // therefore a protocol violation, not something to log.
                ei::Event::Callback(_, _) | ei::Event::Pingpong(_, _) => {
                    return Err(WaitFailure::Transport);
                }
                // No non-text device interface was negotiated.
                ei::Event::Pointer(_, _)
                | ei::Event::PointerAbsolute(_, _)
                | ei::Event::Scroll(_, _)
                | ei::Event::Button(_, _)
                | ei::Event::Keyboard(_, _)
                | ei::Event::Touchscreen(_, _) => {
                    return Err(WaitFailure::Transport);
                }
                // A sender must never receive payload-bearing text events. Do
                // not retain or format a protocol-violating peer's content.
                ei::Event::Text(_, _) => {
                    return Err(WaitFailure::Transport);
                }
                _ => return Err(WaitFailure::Transport),
            }
        }
        Ok(())
    }
}

fn minimal_handshake(
    context: &ei::Context,
    deadline: Instant,
    cancellation: &dyn PortalCancellation,
) -> Result<HandshakeResp, WaitFailure> {
    let mut sent_client_hello = false;
    let mut negotiated = HashMap::new();
    loop {
        wait_for(context, PollFlags::IN, deadline, cancellation)?;
        context.read().map_err(|_| WaitFailure::Transport)?;
        while let Some(result) = context.pending_event() {
            let event = match result {
                PendingRequestResult::Request(event) => event,
                PendingRequestResult::ParseError(_) | PendingRequestResult::InvalidObject(_) => {
                    return Err(WaitFailure::Transport);
                }
            };
            let ei::Event::Handshake(handshake, event) = event else {
                return Err(WaitFailure::Transport);
            };
            match event {
                ei::handshake::Event::HandshakeVersion { version }
                    if version >= 1 && !sent_client_hello =>
                {
                    handshake.handshake_version(1);
                    handshake.name("narrivy-portal-eis-text");
                    handshake.context_type(ei::handshake::ContextType::Sender);
                    for (name, version) in REQUIRED_INTERFACES {
                        handshake.interface_version(name, version);
                    }
                    handshake.finish();
                    sent_client_hello = true;
                    flush_control(context, deadline, cancellation)?;
                }
                ei::handshake::Event::InterfaceVersion { name, version }
                    if sent_client_hello
                        && REQUIRED_INTERFACES
                            .iter()
                            .any(|(required, _)| *required == name) =>
                {
                    let advertised_maximum = REQUIRED_INTERFACES
                        .iter()
                        .find_map(|(required, maximum)| (*required == name).then_some(*maximum))
                        .ok_or(WaitFailure::Transport)?;
                    if version == 0
                        || version > advertised_maximum
                        || negotiated.insert(name, version).is_some()
                    {
                        return Err(WaitFailure::Transport);
                    }
                }
                ei::handshake::Event::Connection { connection, serial }
                    if sent_client_hello
                        && REQUIRED_INTERFACES
                            .iter()
                            .all(|(name, _)| negotiated.contains_key(*name)) =>
                {
                    return Ok(HandshakeResp {
                        connection,
                        serial,
                        negotiated_interfaces: negotiated,
                    });
                }
                _ => return Err(WaitFailure::Transport),
            }
        }
    }
}

fn flush_control(
    context: &ei::Context,
    deadline: Instant,
    cancellation: &dyn PortalCancellation,
) -> Result<(), WaitFailure> {
    loop {
        match context.flush() {
            Ok(()) => return Ok(()),
            Err(Errno::AGAIN) => {
                wait_for(context, PollFlags::OUT, deadline, cancellation)?;
            }
            Err(_) => return Err(WaitFailure::Transport),
        }
    }
}

fn wait_for(
    context: &ei::Context,
    interest: PollFlags,
    deadline: Instant,
    cancellation: &dyn PortalCancellation,
) -> Result<PollFlags, WaitFailure> {
    loop {
        if cancellation.is_cancelled() {
            return Err(WaitFailure::Cancelled);
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(WaitFailure::TimedOut)?;
        let slice = remaining.min(POLL_SLICE);
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: slice.as_nanos() as _,
        };
        let mut fds = [PollFd::new(
            context,
            interest | PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL,
        )];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => continue,
            Ok(_) => {
                let flags = fds[0].revents();
                if flags.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) {
                    return Err(WaitFailure::Transport);
                }
                if flags.intersects(interest) {
                    return Ok(flags);
                }
            }
            Err(Errno::INTR) => continue,
            Err(_) => return Err(WaitFailure::Transport),
        }
    }
}

fn poll_now(context: &ei::Context) -> Result<PollFlags, WaitFailure> {
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let mut fds = [PollFd::new(
        context,
        PollFlags::IN | PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL,
    )];
    poll(&mut fds, Some(&timeout)).map_err(|_| WaitFailure::Transport)?;
    Ok(fds[0].revents())
}

fn monotonic_micros() -> u64 {
    let time = clock_gettime(ClockId::Monotonic);
    u64::try_from(time.tv_sec)
        .unwrap_or_default()
        .saturating_mul(1_000_000)
        .saturating_add(u64::try_from(time.tv_nsec).unwrap_or_default() / 1_000)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::thread;

    use reis::eis;
    use reis::handshake::EisHandshaker;
    use reis::request::{DeviceCapability, EisRequest, EisRequestConverter};

    use super::*;
    use narrivy_types::SessionCancellation;

    const TEST_TIMEOUT: Duration = Duration::from_secs(2);
    const FIXTURE: &str = "controlled wire payload";

    #[derive(Debug)]
    struct PeerReport {
        interfaces: BTreeSet<String>,
        sender_context: bool,
        request_order: Vec<&'static str>,
        payload_matches: bool,
        payload_bytes: usize,
    }

    fn join_peer<T>(
        peer: thread::JoinHandle<Result<T, &'static str>>,
    ) -> Result<T, std::io::Error> {
        peer.join()
            .map_err(|_| std::io::Error::other("peer thread panicked"))?
            .map_err(std::io::Error::other)
    }

    fn test_io<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, std::io::Error> {
        result.map_err(|error| std::io::Error::other(format!("{error:?}")))
    }

    #[test]
    fn minimal_sender_writes_one_text_frame_to_real_protocol_peer()
    -> Result<(), Box<dyn std::error::Error>> {
        let (client, server) = UnixStream::pair()?;
        let peer = thread::spawn(move || run_peer(server, true));
        let cancellation = SessionCancellation::new();
        let sender = test_io(PortalEisSender::connect(
            client,
            Instant::now() + TEST_TIMEOUT,
            &cancellation,
        ))?;
        let (sender, outcome) = sender.submit(
            test_io(PortalEisTextPayload::parse(FIXTURE))?,
            Instant::now() + TEST_TIMEOUT,
            &cancellation,
        );
        assert_eq!(outcome, PortalEisTextSubmission::SubmittedUnverified);
        drop(sender);

        let report = join_peer(peer)?;
        assert_eq!(
            report.interfaces,
            REQUIRED_INTERFACES
                .into_iter()
                .map(|(name, _)| name.to_owned())
                .collect()
        );
        assert!(report.sender_context);
        assert_eq!(report.request_order, ["start", "text", "frame", "stop"]);
        assert!(report.payload_matches);
        assert_eq!(report.payload_bytes, FIXTURE.len());
        Ok(())
    }

    #[test]
    fn missing_text_capability_fails_closed_during_connect()
    -> Result<(), Box<dyn std::error::Error>> {
        let (client, server) = UnixStream::pair()?;
        let peer = thread::spawn(move || run_peer(server, false));
        let result = PortalEisSender::connect(
            client,
            Instant::now() + TEST_TIMEOUT,
            &SessionCancellation::new(),
        );
        assert_eq!(result.err(), Some(PortalSessionFailure::TransportFailed));
        let _ = join_peer(peer)?;
        Ok(())
    }

    #[test]
    fn peer_cannot_negotiate_above_the_advertised_interface_maximum()
    -> Result<(), Box<dyn std::error::Error>> {
        let (client, server) = UnixStream::pair()?;
        let peer = thread::spawn(move || run_overversioned_peer(server));
        let result = PortalEisSender::connect(
            client,
            Instant::now() + TEST_TIMEOUT,
            &SessionCancellation::new(),
        );
        assert_eq!(result.err(), Some(PortalSessionFailure::TransportFailed));
        join_peer(peer)?;
        Ok(())
    }

    #[test]
    fn cancellation_before_queueing_is_clean_and_emits_no_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let (client, server) = UnixStream::pair()?;
        let peer = thread::spawn(move || run_peer(server, true));
        let cancellation = SessionCancellation::new();
        let sender = test_io(PortalEisSender::connect(
            client,
            Instant::now() + TEST_TIMEOUT,
            &cancellation,
        ))?;
        cancellation.cancel();
        let (sender, outcome) = sender.submit(
            test_io(PortalEisTextPayload::parse(FIXTURE))?,
            Instant::now() + TEST_TIMEOUT,
            &cancellation,
        );
        assert_eq!(outcome, PortalEisTextSubmission::CancelledBeforeDispatch);
        drop(sender);
        let report = join_peer(peer)?;
        assert!(report.request_order.is_empty());
        assert!(!report.payload_matches);
        assert_eq!(report.payload_bytes, 0);
        Ok(())
    }

    #[test]
    fn reis_debug_declaration_is_rejected_before_context_creation()
    -> Result<(), Box<dyn std::error::Error>> {
        let (client, server) = UnixStream::pair()?;
        let result = PortalEisSender::connect_with_debug_guard(
            client,
            Instant::now() + TEST_TIMEOUT,
            &SessionCancellation::new(),
            true,
        );
        drop(server);
        assert_eq!(result.err(), Some(PortalSessionFailure::TransportFailed));
        Ok(())
    }

    fn run_peer(stream: UnixStream, offer_text: bool) -> Result<PeerReport, &'static str> {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let context = eis::Context::new(stream).map_err(|_| "peer context")?;
        let mut handshaker = EisHandshaker::new(&context, 40);
        flush_peer(&context, deadline)?;

        let handshake = loop {
            read_peer(&context, deadline)?;
            let mut completed = None;
            while let Some(request) = context.pending_request() {
                let request = match request {
                    PendingRequestResult::Request(request) => request,
                    PendingRequestResult::ParseError(_)
                    | PendingRequestResult::InvalidObject(_) => return Err("peer handshake"),
                };
                if let Some(response) = handshaker
                    .handle_request(request)
                    .map_err(|_| "peer handshake")?
                {
                    completed = Some(response);
                }
            }
            flush_peer(&context, deadline)?;
            if let Some(response) = completed {
                break response;
            }
        };

        let interfaces = handshake
            .negotiated_interfaces
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let sender_context = handshake.context_type == eis::handshake::ContextType::Sender;
        let mut converter = EisRequestConverter::new(&context, handshake, 40);
        let connection = converter.handle().clone();
        let capabilities = if offer_text {
            reis::enumflags2::BitFlags::from(DeviceCapability::Text)
        } else {
            reis::enumflags2::BitFlags::empty()
        };
        let _seat = connection.add_seat(Some("controlled-peer"), capabilities);
        flush_peer(&context, deadline)?;

        let mut request_order = Vec::new();
        let mut payload_matches = false;
        let mut payload_bytes = 0;
        let mut device = None;
        loop {
            match context.read() {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(PeerReport {
                        interfaces,
                        sender_context,
                        request_order,
                        payload_matches,
                        payload_bytes,
                    });
                }
                Err(_) => return Err("peer read"),
            }
            while let Some(request) = context.pending_request() {
                let request = match request {
                    PendingRequestResult::Request(request) => request,
                    PendingRequestResult::ParseError(_)
                    | PendingRequestResult::InvalidObject(_) => return Err("peer request"),
                };
                converter
                    .handle_request(request)
                    .map_err(|_| "peer conversion")?;
            }
            while let Some(request) = converter.next_request() {
                match request {
                    EisRequest::Bind(binding) if offer_text && device.is_none() => {
                        let added = binding.seat.add_device(
                            Some("controlled-text"),
                            eis::device::DeviceType::Virtual,
                            DeviceCapability::Text.into(),
                            |_| {},
                        );
                        added.resumed();
                        device = Some(added);
                    }
                    EisRequest::Bind(_) => {}
                    EisRequest::DeviceStartEmulating(_) => request_order.push("start"),
                    EisRequest::TextUtf8(text) => {
                        payload_bytes = text.text.len();
                        payload_matches = text.text == FIXTURE;
                        request_order.push("text");
                    }
                    EisRequest::Frame(_) => request_order.push("frame"),
                    EisRequest::DeviceStopEmulating(_) => request_order.push("stop"),
                    EisRequest::Disconnect | EisRequest::DeviceClosed(_) => {
                        return Ok(PeerReport {
                            interfaces,
                            sender_context,
                            request_order,
                            payload_matches,
                            payload_bytes,
                        });
                    }
                    _ => return Err("unexpected peer request"),
                }
            }
            flush_peer(&context, deadline)?;
            if request_order.as_slice() == ["start", "text", "frame", "stop"] {
                return Ok(PeerReport {
                    interfaces,
                    sender_context,
                    request_order,
                    payload_matches,
                    payload_bytes,
                });
            }
            if Instant::now() >= deadline {
                return Err("peer timeout");
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn run_overversioned_peer(stream: UnixStream) -> Result<(), &'static str> {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let context = eis::Context::new(stream).map_err(|_| "peer context")?;
        let handshake = context.handshake();
        handshake.handshake_version(1);
        flush_peer(&context, deadline)?;

        loop {
            read_peer(&context, deadline)?;
            while let Some(request) = context.pending_request() {
                let request = match request {
                    PendingRequestResult::Request(request) => request,
                    PendingRequestResult::ParseError(_)
                    | PendingRequestResult::InvalidObject(_) => return Err("peer handshake"),
                };
                match request {
                    eis::Request::Handshake(_, eis::handshake::Request::Finish) => {
                        for (name, version) in REQUIRED_INTERFACES {
                            let negotiated = if name == "ei_text" {
                                version + 1
                            } else {
                                version
                            };
                            handshake.interface_version(name, negotiated);
                        }
                        let _connection = handshake.connection(40, 1);
                        flush_peer(&context, deadline)?;
                        return Ok(());
                    }
                    eis::Request::Handshake(_, _) => {}
                    _ => return Err("non-handshake peer request"),
                }
            }
            if Instant::now() >= deadline {
                return Err("peer timeout");
            }
        }
    }

    fn read_peer(context: &eis::Context, deadline: Instant) -> Result<(), &'static str> {
        loop {
            match context.read() {
                Ok(0) if Instant::now() < deadline => thread::sleep(Duration::from_millis(1)),
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return Err("peer read"),
            }
            if Instant::now() >= deadline {
                return Err("peer timeout");
            }
        }
    }

    fn flush_peer(context: &eis::Context, deadline: Instant) -> Result<(), &'static str> {
        loop {
            match context.flush() {
                Ok(()) => return Ok(()),
                Err(Errno::AGAIN) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return Err("peer flush"),
            }
        }
    }
}
