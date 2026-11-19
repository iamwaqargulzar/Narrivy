//! What a same-user but hostile or broken peer can send the server: slow
//! frames, garbage frames and random commands. The server must answer with
//! a typed rejection or close, within its idle timeout, and keep serving.

use super::*;
use narrivy_ipc::{MAX_MESSAGE_BYTES, PROTOCOL_MAJOR, ResponsePayload};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use std::io::Write;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// How long a test waits for the server before calling it a hang.
const PATIENCE: Duration = Duration::from_secs(3);

struct Running {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    server: Arc<UnixIpcServer>,
    thread: Option<JoinHandle<Result<(), IpcError>>>,
}

impl Running {
    /// A server whose handler accepts every request.
    fn start() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("untrusted.sock");
        let server = Arc::new(UnixIpcServer::bind(&socket)?);
        let serving = server.clone();
        let thread = std::thread::spawn(move || {
            serving.serve(|_peer: PeerCredentials, envelope: Envelope| {
                Envelope::new(MessageKind::Response, envelope.request_id)
                    .with_payload(ResponsePayload::Accepted)
                    .ok()
            })
        });
        Ok(Self {
            _dir: dir,
            socket,
            server,
            thread: Some(thread),
        })
    }

    fn connect(&self) -> Result<UnixStream, Box<dyn std::error::Error + Send + Sync>> {
        let stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(PATIENCE))?;
        stream.set_write_timeout(Some(PATIENCE))?;
        Ok(stream)
    }

    fn handshake(&self) -> Result<UnixStream, Box<dyn std::error::Error + Send + Sync>> {
        let mut stream = self.connect()?;
        write_frame(
            &mut stream,
            &serde_json::to_vec(&HandshakeHello::new("test"))?,
        )?;
        let reply: HandshakeResponse = serde_json::from_slice(&read_frame(&mut stream)?)?;
        assert!(matches!(reply, HandshakeResponse::Accepted { .. }));
        Ok(stream)
    }

    /// A fresh client still gets a full request and reply.
    fn still_serves(&self) -> TestResult {
        let mut stream = self.handshake()?;
        let request = Envelope::new(MessageKind::GetStatus, RequestId(7))
            .with_payload(narrivy_ipc::Command::GetStatus)?;
        write_frame(&mut stream, &request.to_json_bytes()?)?;
        let reply = Envelope::from_json_bytes(&read_frame(&mut stream)?)?;
        assert_eq!(reply.request_id, RequestId(7));
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.server.request_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Sends a frame that declares `declared` bytes, one byte every 100 ms
/// (well inside the 400 ms idle timeout). Returns how long the server took
/// to give up on it, or `None` if it never did.
fn trickle(stream: &mut UnixStream, declared: u32) -> Option<Duration> {
    let started = Instant::now();
    if stream.write_all(&declared.to_le_bytes()).is_err() {
        return Some(started.elapsed());
    }
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let mut buf = [0_u8; 256];
    while started.elapsed() < PATIENCE {
        match stream.read(&mut buf) {
            // A rejection frame, or the close itself.
            Ok(_) => return Some(started.elapsed()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return Some(started.elapsed()),
        }
        if stream.write_all(b"x").is_err() {
            return Some(started.elapsed());
        }
    }
    None
}

/// Before the fix, every byte restarted the socket timeout, so a frame sent
/// slowly enough kept the connection (and its thread) forever. The whole
/// frame now has to arrive within the idle timeout, in the handshake and
/// after it.
#[test]
fn a_frame_sent_one_byte_at_a_time_is_cut_off() -> TestResult {
    let running = Running::start()?;

    let mut stream = running.connect()?;
    let took = trickle(&mut stream, 4_096).ok_or("handshake trickle never cut off")?;
    assert!(took < CONNECTION_IDLE_TIMEOUT * 3, "took {took:?}");

    let mut stream = running.handshake()?;
    let took = trickle(&mut stream, 4_096).ok_or("request trickle never cut off")?;
    assert!(took < CONNECTION_IDLE_TIMEOUT * 3, "took {took:?}");

    running.still_serves()
}

/// Oversized declared lengths are refused after the prefix, with a typed
/// rejection, before and after the handshake.
#[test]
fn oversized_declared_lengths_get_a_typed_rejection() -> TestResult {
    let running = Running::start()?;
    for declared in [MAX_MESSAGE_BYTES as u32 + 1, u32::MAX] {
        let mut stream = running.connect()?;
        stream.write_all(&declared.to_le_bytes())?;
        let rejection = Envelope::from_json_bytes(&read_frame(&mut stream)?)?;
        assert!(matches!(
            rejection.parse_payload::<ResponsePayload>()?,
            ResponsePayload::Rejected {
                code: IpcErrorCode::PayloadTooLarge,
                ..
            }
        ));

        let mut stream = running.handshake()?;
        stream.write_all(&declared.to_le_bytes())?;
        let rejection = Envelope::from_json_bytes(&read_frame(&mut stream)?)?;
        assert!(matches!(
            rejection.parse_payload::<ResponsePayload>()?,
            ResponsePayload::Rejected {
                code: IpcErrorCode::PayloadTooLarge,
                ..
            }
        ));
    }
    running.still_serves()
}

/// Random frames after the handshake: the server answers each with a reply
/// or a typed rejection (or closes) in good time, and never stops serving.
#[test]
fn random_frames_after_the_handshake_are_answered_or_closed() -> TestResult {
    let running = Running::start()?;
    let payload = prop_oneof![
        // Raw bytes, mostly not JSON and not UTF-8.
        prop::collection::vec(any::<u8>(), 0..128),
        // Envelopes with random kinds, minors and command objects.
        (
            "request\\.[a-z_]{0,16}|response|event|[a-z.]{0,8}",
            any::<u16>(),
            "[a-z_]{0,20}",
            prop::collection::vec(("[a-z_]{1,12}", "\"[a-z]{0,8}\"|[0-9]{1,4}|null|\\[\\]|\\{\\}"), 0..4),
        )
            .prop_map(|(kind, minor, command, fields)| {
                let fields: String = fields
                    .iter()
                    .map(|(name, value)| format!(r#","{name}":{value}"#))
                    .collect();
                format!(
                    r#"{{"version_major":{PROTOCOL_MAJOR},"version_minor":{minor},"kind":"{kind}","request_id":3,"payload":{{"command":"{command}"{fields}}}}}"#
                )
                .into_bytes()
            }),
    ];
    let mut config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 48;
    }
    let mut runner = TestRunner::new(config);
    runner
        .run(&payload, |bytes| {
            let mut stream = running
                .handshake()
                .map_err(|e| TestCaseError::fail(e.to_string()))?;
            write_frame(&mut stream, &bytes).map_err(|e| TestCaseError::fail(e.to_string()))?;
            match read_frame(&mut stream) {
                Ok(reply) => {
                    let reply = Envelope::from_json_bytes(&reply)
                        .map_err(|e| TestCaseError::fail(e.to_string()))?;
                    prop_assert!(reply.parse_payload::<ResponsePayload>().is_ok());
                }
                // Closed after a rejection, never a timeout.
                Err(err) => prop_assert_eq!(err.code, IpcErrorCode::Cancelled),
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    running.still_serves()
}
