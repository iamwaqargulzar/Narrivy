//! Blocking unix-socket IPC client.
//!
//! Salvage-adapted from `code/narrivy-v0.4/crates/narrivy-ipc/src/client.rs`
//! (framed handshake, strict request-id correlation) and extended per P1:
//! explicit timeouts on every request, event queuing while requests are in
//! flight, late-response buffering, typed reconnect.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use narrivy_ipc::error::{IpcError, IpcErrorCode};
use narrivy_ipc::framing::{encode_frame, read_frame};
use narrivy_ipc::protocol::{
    Command, Envelope, Event, HandshakeHello, HandshakeResponse, MessageKind, RequestId,
    ResponsePayload,
};

use crate::security::{current_uid, peer_credentials};

/// Timeout applied to the handshake exchange during connect/reconnect.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// One daemon event paired with its transport-wide monotonic sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequencedEvent {
    pub event_seq: u64,
    pub event: Event,
}

/// Blocking client connection to narrivyd over a unix socket.
pub struct IpcClient {
    stream: Option<UnixStream>,
    socket_path: PathBuf,
    client_role: String,
    next_request_id: u64,
    /// Events decoded while waiting inside [`IpcClient::request`].
    queued_events: VecDeque<SequencedEvent>,
    /// Responses buffered while their caller was not reading yet.
    pending_responses: HashMap<u64, Envelope>,
    protocol_minor: u16,
    daemon_build: String,
    last_event_seq: Option<u64>,
}

impl IpcClient {
    /// Connect to the daemon socket and perform the versioned handshake.
    ///
    /// Major skew fails with [`IpcErrorCode::IncompatibleProtocol`]; a
    /// daemon-side rejection surfaces its code and detail verbatim.
    pub fn connect(socket_path: &Path) -> Result<Self, IpcError> {
        Self::connect_as(socket_path, "client")
    }

    /// [`IpcClient::connect`] with an explicit client role advertised in the
    /// handshake (e.g. `"desktop"`, `"cli"`, `"overlay"`).
    pub fn connect_as(socket_path: &Path, role: &str) -> Result<Self, IpcError> {
        Self::connect_as_until(socket_path, role, Instant::now() + HANDSHAKE_TIMEOUT)
    }

    /// Share a caller-owned absolute budget across connection and handshake.
    pub fn connect_as_until(
        socket_path: &Path,
        role: &str,
        deadline: Instant,
    ) -> Result<Self, IpcError> {
        Self::connect_with_deadline(socket_path, role, deadline, validate_daemon_peer)
    }

    #[cfg(test)]
    fn connect_as_with_peer_validator<F>(
        socket_path: &Path,
        role: &str,
        validate_peer: F,
    ) -> Result<Self, IpcError>
    where
        F: FnOnce(&UnixStream) -> Result<(), IpcError>,
    {
        Self::connect_with_deadline(
            socket_path,
            role,
            Instant::now() + HANDSHAKE_TIMEOUT,
            validate_peer,
        )
    }

    fn connect_with_deadline<F>(
        socket_path: &Path,
        role: &str,
        deadline: Instant,
        validate_peer: F,
    ) -> Result<Self, IpcError>
    where
        F: FnOnce(&UnixStream) -> Result<(), IpcError>,
    {
        let mut client = Self {
            stream: None,
            socket_path: socket_path.to_path_buf(),
            client_role: role.to_owned(),
            next_request_id: 1,
            queued_events: VecDeque::new(),
            pending_responses: HashMap::new(),
            protocol_minor: 0,
            daemon_build: String::new(),
            last_event_seq: None,
        };
        client.open_and_handshake_with(deadline, validate_peer)?;
        Ok(client)
    }

    fn open_and_handshake(&mut self) -> Result<(), IpcError> {
        self.open_and_handshake_with(Instant::now() + HANDSHAKE_TIMEOUT, validate_daemon_peer)
    }

    fn open_and_handshake_with<F>(
        &mut self,
        deadline: Instant,
        validate_peer: F,
    ) -> Result<(), IpcError>
    where
        F: FnOnce(&UnixStream) -> Result<(), IpcError>,
    {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| IpcError::new(IpcErrorCode::Timeout, "IPC connect deadline exceeded"))?;
        let address = socket2::SockAddr::unix(&self.socket_path)
            .map_err(|_| IpcError::bad_request("invalid Unix socket path"))?;
        let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)
            .map_err(|_| IpcError::internal("could not create IPC socket"))?;
        socket
            .connect_timeout(&address, remaining)
            .map_err(|error| {
                let code = match error.kind() {
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                        IpcErrorCode::Timeout
                    }
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound => {
                        IpcErrorCode::DaemonUnavailable
                    }
                    _ => IpcErrorCode::Internal,
                };
                IpcError::new(code, "IPC connection failed")
            })?;
        let descriptor: std::os::fd::OwnedFd = socket.into();
        let stream = UnixStream::from(descriptor);

        // Authenticate the daemon using kernel-reported credentials before
        // sending even the content-free protocol hello. This is the client
        // half of ADR-0011's bidirectional same-user transport boundary.
        validate_peer(&stream)?;

        let handshake_deadline = deadline;
        let hello = HandshakeHello::new(self.client_role.clone());
        hello.validate()?;
        let hello_bytes = serde_json::to_vec(&hello)
            .map_err(|e| IpcError::bad_request(format!("handshake encode failed: {e}")))?;
        let mut wire = stream
            .try_clone()
            .map_err(|e| IpcError::internal(format!("stream clone failed: {e}")))?;
        let mut wire = DeadlineWire {
            stream: &mut wire,
            deadline: handshake_deadline,
        };
        wire.write_frame(&hello_bytes)?;

        let response_bytes = read_frame(&mut wire)?;
        let response: HandshakeResponse = serde_json::from_slice(&response_bytes)
            .map_err(|e| IpcError::bad_request(format!("handshake decode failed: {e}")))?;
        match response {
            HandshakeResponse::Accepted {
                protocol_minor,
                daemon_build,
            } => {
                self.protocol_minor = protocol_minor;
                self.daemon_build = daemon_build;
                self.stream = Some(stream);
                Ok(())
            }
            HandshakeResponse::Rejected { code, detail } => Err(IpcError::new(code, detail)),
        }
    }

    /// Protocol minor version negotiated at handshake time.
    pub fn negotiated_minor(&self) -> u16 {
        self.protocol_minor
    }

    /// Daemon build identity reported by the handshake.
    pub fn daemon_build(&self) -> &str {
        &self.daemon_build
    }

    /// True while the transport has a live socket.
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Send one command and await its correlated reply within `timeout`.
    ///
    /// Events pushed while waiting are queued for [`IpcClient::next_event`];
    /// replies for other request ids are buffered the same way. A
    /// `ResponsePayload::Rejected` reply maps to an [`IpcError`] carrying
    /// the server's code and detail.
    pub fn request(
        &mut self,
        command: &Command,
        timeout: Duration,
    ) -> Result<ResponsePayload, IpcError> {
        let deadline = Instant::now() + timeout;
        let mut received_response = false;
        let result = self.request_until(command, deadline, &mut received_response);
        if result.is_err() && !received_response {
            // A timeout may leave a partial frame. Never interpret its remaining
            // bytes as a new envelope or replay a potentially executed command.
            self.stream = None;
            self.pending_responses.clear();
            self.queued_events.clear();
            self.last_event_seq = None;
        }
        result
    }

    fn request_until(
        &mut self,
        command: &Command,
        deadline: Instant,
        received_response: &mut bool,
    ) -> Result<ResponsePayload, IpcError> {
        let request_id = self.allocate_request_id();

        // A reply may already sit in the buffer (arrived during earlier traffic).
        if let Some(envelope) = self.pending_responses.remove(&request_id.0) {
            *received_response = true;
            return decode_response(envelope);
        }

        let envelope = Envelope::new(command.kind(), request_id).with_payload(command)?;
        DeadlineWire {
            stream: self.connected_stream()?,
            deadline,
        }
        .write_frame(&envelope.to_json_bytes()?)?;

        loop {
            let bytes = read_frame(&mut DeadlineWire {
                stream: self.connected_stream()?,
                deadline,
            })?;
            let envelope = Envelope::from_json_bytes(&bytes)?;
            match envelope.kind {
                MessageKind::Response => {
                    if envelope.request_id == request_id {
                        *received_response = true;
                        return decode_response(envelope);
                    }
                    // Reply for a different id: keep it for its own caller.
                    self.pending_responses
                        .insert(envelope.request_id.0, envelope);
                }
                MessageKind::Event => {
                    let event = decode_event(envelope, &mut self.last_event_seq)?;
                    self.queued_events.push_back(event);
                }
                other => {
                    return Err(IpcError::bad_request(format!(
                        "unexpected kind {:?} while awaiting response",
                        other.wire_tag()
                    )));
                }
            }
        }
    }

    /// Receive the next daemon-pushed event, waiting up to `timeout`.
    ///
    /// Events queued during earlier requests are delivered first, in order.
    /// Response frames encountered meanwhile are buffered for correlation.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Event, IpcError> {
        self.next_event_with_sequence(timeout)
            .map(|event| event.event)
    }

    /// Receive the next daemon event with the envelope sequence preserved.
    /// Desktop consumers use this to reject stale cross-session updates.
    pub fn next_event_with_sequence(
        &mut self,
        timeout: Duration,
    ) -> Result<SequencedEvent, IpcError> {
        if let Some(event) = self.queued_events.pop_front() {
            return Ok(event);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let mut reader = FrameRead {
                wire: DeadlineWire {
                    stream: self.connected_stream()?,
                    deadline,
                },
                received: 0,
            };
            let result = read_frame(&mut reader);
            let received = reader.received;
            let bytes = match result {
                Ok(bytes) => bytes,
                Err(error) => {
                    // An idle timeout consumed no prefix and preserves framing.
                    // Any partial frame or terminal transport error requires a
                    // reconnect; discarded prefix bytes cannot be recovered.
                    if received != 0 || error.code != IpcErrorCode::Timeout {
                        self.stream = None;
                        self.pending_responses.clear();
                        self.queued_events.clear();
                        self.last_event_seq = None;
                    }
                    return Err(error);
                }
            };
            let envelope = Envelope::from_json_bytes(&bytes)?;
            match envelope.kind {
                MessageKind::Event => return decode_event(envelope, &mut self.last_event_seq),
                MessageKind::Response => {
                    self.pending_responses
                        .insert(envelope.request_id.0, envelope);
                }
                other => {
                    return Err(IpcError::bad_request(format!(
                        "unexpected kind {:?} while awaiting event",
                        other.wire_tag()
                    )));
                }
            }
        }
    }

    /// Drop the current transport and reconnect + re-handshake.
    ///
    /// Buffered responses and events are cleared because they belong to the
    /// dead transport epoch. The next daemon connection starts a fresh event
    /// sequence.
    pub fn reconnect(&mut self) -> Result<(), IpcError> {
        self.stream = None;
        self.pending_responses.clear();
        self.queued_events.clear();
        self.last_event_seq = None;
        self.open_and_handshake()
    }

    fn allocate_request_id(&mut self) -> RequestId {
        let id = RequestId(self.next_request_id);
        // Wrap-safe monotonic allocation; ids need uniqueness per epoch only.
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        id
    }

    fn connected_stream(&mut self) -> Result<&mut UnixStream, IpcError> {
        self.stream.as_mut().ok_or_else(|| {
            IpcError::new(
                IpcErrorCode::DaemonUnavailable,
                "not connected; call reconnect()",
            )
        })
    }
}

fn decode_event(
    envelope: Envelope,
    last_event_seq: &mut Option<u64>,
) -> Result<SequencedEvent, IpcError> {
    let event_seq = envelope
        .event_seq
        .ok_or_else(|| IpcError::bad_request("event envelope omitted event_seq"))?;
    if last_event_seq.is_some_and(|last| event_seq <= last) {
        return Err(IpcError::bad_request(
            "event envelope sequence was stale or out of order",
        ));
    }
    let event = envelope.parse_payload()?;
    *last_event_seq = Some(event_seq);
    Ok(SequencedEvent { event_seq, event })
}

fn validate_daemon_peer(stream: &UnixStream) -> Result<(), IpcError> {
    let credentials = peer_credentials(stream).map_err(|err| {
        IpcError::permission_denied(format!("daemon peer credentials unavailable: {err}"))
    })?;
    credentials
        .validate(current_uid())
        .map_err(|err| IpcError::permission_denied(format!("daemon peer identity rejected: {err}")))
}

/// Turn a response envelope into a payload or a typed error.
fn decode_response(envelope: Envelope) -> Result<ResponsePayload, IpcError> {
    match envelope.parse_payload::<ResponsePayload>()? {
        ResponsePayload::Rejected { code, detail } => Err(IpcError::new(code, detail)),
        payload => Ok(payload),
    }
}

/// Recompute the remaining budget for every underlying read/write, including
/// the framing library's short-I/O retries. Socket timeouts alone reset after
/// each successful short operation and do not establish a frame deadline.
struct DeadlineWire<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
}

struct FrameRead<'a> {
    wire: DeadlineWire<'a>,
    received: usize,
}

impl Read for FrameRead<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.wire.read(buffer)?;
        self.received += count;
        Ok(count)
    }
}

impl DeadlineWire<'_> {
    fn write_frame(&mut self, payload: &[u8]) -> Result<(), IpcError> {
        let frame = encode_frame(payload)?;
        self.write_all(&frame)
            .and_then(|()| self.flush())
            .map_err(|error| {
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) {
                    IpcError::new(IpcErrorCode::Timeout, "IPC frame write deadline exceeded")
                } else {
                    IpcError::internal("IPC frame write failed")
                }
            })
    }

    fn remaining(&self) -> std::io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "IPC operation deadline")
            })
    }
}

impl Read for DeadlineWire<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(buffer)
    }
}

impl Write for DeadlineWire<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_budget_includes_unresponsive_handshake() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("silent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        let peer = std::thread::spawn(move || -> Result<usize, String> {
            let (mut socket, _) = listener.accept().map_err(|error| error.to_string())?;
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .map_err(|error| error.to_string())?;
            read_frame(&mut socket).map_err(|error| error.to_string())?;
            socket.read(&mut [0; 1]).map_err(|error| error.to_string())
        });
        let result = IpcClient::connect_as_until(
            &path,
            "fixture",
            Instant::now() + Duration::from_millis(100),
        );
        assert_eq!(
            result.err().map(|error| error.code),
            Some(IpcErrorCode::Timeout)
        );
        assert_eq!(peer.join().map_err(|_| "fixture panicked")??, 0);
        Ok(())
    }

    #[test]
    fn expired_connect_budget_fails_before_connecting() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("unused.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let result = IpcClient::connect_as_until(&path, "fixture", Instant::now());
        assert_eq!(
            result.err().map(|error| error.code),
            Some(IpcErrorCode::Timeout)
        );
        assert_eq!(
            listener.accept().err().map(|error| error.kind()),
            Some(std::io::ErrorKind::WouldBlock)
        );
        Ok(())
    }

    fn client_for_stream(stream: UnixStream) -> IpcClient {
        IpcClient {
            stream: Some(stream),
            socket_path: PathBuf::new(),
            client_role: "fixture".into(),
            next_request_id: 1,
            queued_events: VecDeque::new(),
            pending_responses: HashMap::new(),
            protocol_minor: 0,
            daemon_build: String::new(),
            last_event_seq: None,
        }
    }

    #[test]
    fn event_idle_timeout_preserves_connection_but_partial_frame_discards_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let (stream, mut peer) = UnixStream::pair()?;
        let mut client = client_for_stream(stream);
        assert_eq!(
            client
                .next_event(Duration::from_millis(20))
                .err()
                .map(|error| error.code),
            Some(IpcErrorCode::Timeout)
        );
        assert!(client.is_connected());
        peer.write_all(&[10, 0])?; // Incomplete length prefix.
        assert_eq!(
            client
                .next_event(Duration::from_millis(20))
                .err()
                .map(|error| error.code),
            Some(IpcErrorCode::Timeout)
        );
        assert!(!client.is_connected());
        assert_eq!(
            client
                .next_event(Duration::from_millis(20))
                .err()
                .map(|error| error.code),
            Some(IpcErrorCode::DaemonUnavailable)
        );
        Ok(())
    }

    #[test]
    fn request_timeout_invalidates_stream_without_replay() -> Result<(), Box<dyn std::error::Error>>
    {
        let (stream, mut peer) = UnixStream::pair()?;
        let mut client = IpcClient {
            stream: Some(stream),
            socket_path: PathBuf::new(),
            client_role: "fixture".into(),
            next_request_id: 1,
            queued_events: VecDeque::new(),
            pending_responses: HashMap::new(),
            protocol_minor: 0,
            daemon_build: String::new(),
            last_event_seq: None,
        };
        assert_eq!(
            client
                .request(&Command::GetStatus, Duration::ZERO)
                .err()
                .map(|error| error.code),
            Some(IpcErrorCode::Timeout)
        );
        assert!(!client.is_connected());
        assert_eq!(
            client
                .request(&Command::GetStatus, Duration::from_secs(1))
                .err()
                .map(|error| error.code),
            Some(IpcErrorCode::DaemonUnavailable)
        );
        assert_eq!(peer.read(&mut [0; 8])?, 0);
        Ok(())
    }

    #[test]
    fn expired_deadline_sends_no_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let (mut client, mut peer) = UnixStream::pair()?;
        peer.set_nonblocking(true)?;
        let mut wire = DeadlineWire {
            stream: &mut client,
            deadline: Instant::now(),
        };
        assert_eq!(
            wire.write(b"fixture").err().map(|error| error.kind()),
            Some(std::io::ErrorKind::TimedOut)
        );
        assert_eq!(
            peer.read(&mut [0; 8]).err().map(|error| error.kind()),
            Some(std::io::ErrorKind::WouldBlock)
        );
        Ok(())
    }

    #[test]
    fn trickled_frame_cannot_restart_absolute_budget() -> Result<(), Box<dyn std::error::Error>> {
        let (mut client, mut peer) = UnixStream::pair()?;
        let writer = std::thread::spawn(move || {
            // Each byte arrives faster than the overall timeout. A timeout
            // reset per read would accept this frame after more than 300 ms.
            for byte in [4, 0, 0, 0, b't', b'e', b's', b't'] {
                if peer.write_all(&[byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(45));
            }
        });
        let result = read_frame(&mut DeadlineWire {
            stream: &mut client,
            deadline: Instant::now() + Duration::from_millis(100),
        });
        drop(client);
        writer.join().map_err(|_| "fixture writer panicked")?;
        assert_eq!(
            result.err().map(|error| error.code),
            Some(IpcErrorCode::Timeout)
        );
        Ok(())
    }

    #[test]
    fn ipc_client_connect_refused_is_daemon_unavailable() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let missing = dir.path().join("no-daemon-here.sock");
        let err = match IpcClient::connect(&missing) {
            Err(err) => err,
            Ok(_) => return Err("connect to missing socket must fail".into()),
        };
        assert_eq!(err.code, IpcErrorCode::DaemonUnavailable);
        Ok(())
    }

    #[test]
    fn ipc_client_authenticates_daemon_before_sending_hello()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("fake-daemon.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket)?;
        let fake_daemon = std::thread::spawn(move || -> std::io::Result<usize> {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(1)))?;
            let mut observed = [0_u8; 64];
            stream.read(&mut observed)
        });

        let err =
            match IpcClient::connect_as_with_peer_validator(&socket, "auth-order-test", |_stream| {
                Err(IpcError::permission_denied("injected daemon uid mismatch"))
            }) {
                Err(err) => err,
                Ok(_) => return Err("injected daemon identity mismatch must fail closed".into()),
            };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        assert!(err.message.contains("injected daemon uid mismatch"));
        assert_eq!(
            fake_daemon.join().map_err(|_| "fake daemon panicked")??,
            0,
            "the fake daemon must observe EOF without any handshake bytes"
        );
        Ok(())
    }

    #[test]
    fn sequenced_event_decode_rejects_missing_duplicate_and_reordered_envelopes()
    -> Result<(), Box<dyn std::error::Error>> {
        let event = Event::HealthChanged {
            module_id: "controlled".to_owned(),
            healthy: true,
        };
        let make = |seq| {
            let envelope = Envelope::new(MessageKind::Event, RequestId(0)).with_payload(&event)?;
            Ok::<_, IpcError>(match seq {
                Some(seq) => envelope.with_event_seq(seq),
                None => envelope,
            })
        };
        let mut last = None;
        assert!(decode_event(make(None)?, &mut last).is_err());
        assert_eq!(decode_event(make(Some(4))?, &mut last)?.event_seq, 4);
        assert!(decode_event(make(Some(4))?, &mut last).is_err());
        assert!(decode_event(make(Some(3))?, &mut last).is_err());
        assert_eq!(last, Some(4));
        Ok(())
    }
}
