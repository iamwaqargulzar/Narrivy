//! Blocking named-pipe IPC server (`WindowsIpcServer`) for narrivyd.
//!
//! The Windows counterpart of `narrivy-ipc-unix`'s `UnixIpcServer`, with the
//! same public API, protocol loop, admission and shutdown behaviour. The
//! endpoint lifecycle (ADR-0011/ADR-0012) maps onto named pipes as follows:
//!
//! - the endpoint is a per-user pipe name; every instance is created with a
//!   protected DACL granting only the current user's SID ([`endpoint_sddl`])
//!   and rejects remote clients;
//! - the first instance uses `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a running
//!   daemon, or a pipe squatted by anyone else, makes `bind` fail instead of
//!   joining someone else's endpoint. That replaces the Unix `flock`;
//! - a listening instance always exists while the server runs (the next one
//!   is created before a connected one is handed out), and admitted
//!   connections keep their instances, so a new server generation cannot
//!   bind until the last handler of the old one has exited;
//! - peer identity (client process id → token user SID) is read and
//!   validated **before** any frame is read or any handler runs;
//! - bounded threads (one per connection, hard cap), idle timeouts, and a
//!   typed rejection envelope for every policy violation.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::pipe::PipeStream;
use crate::security::{PeerCredentials, UserSid, endpoint_sddl};
use crate::sys::{self, Operation, SecurityDescriptor};
use narrivy_ipc::error::{IpcError, IpcErrorCode};
use narrivy_ipc::framing::{read_frame, write_frame};
use narrivy_ipc::protocol::{HandshakeHello, HandshakeResponse, negotiate_handshake};
use narrivy_ipc::{
    Command, Envelope, Event, MessageKind, RequestId, ResponsePayload,
    TRANSCRIPT_PARTIAL_PROTOCOL_MINOR,
};
use narrivy_types::ShutdownBudget;

/// Hard cap on concurrently handled connections; excess peers get a typed
/// [`IpcErrorCode::DaemonUnavailable`] rejection instead of unbounded growth.
pub const MAX_CONCURRENT_CONNECTIONS: usize = 64;

/// Idle timeout applied to each connection; stuck peers cannot pin threads.
#[cfg(not(test))]
pub const CONNECTION_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Short in tests, so idle behaviour is checked without waiting 30 s.
#[cfg(test)]
pub const CONNECTION_IDLE_TIMEOUT: Duration = Duration::from_millis(400);

/// Per-client bound for a daemon-pushed event. A subscribed but stalled UI
/// must not block the daemon event pump for the normal request timeout.
pub const EVENT_WRITE_TIMEOUT: Duration = Duration::from_millis(50);

/// Longest single wait for a client before the accept loop re-checks its
/// shutdown state. Shutdown also wakes the wait immediately.
pub const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum time a normal server shutdown waits for admitted connections.
///
/// This is the default total only; a composing service may install one shared
/// [`ShutdownBudget`] so connection cleanup spends the same remaining time as
/// every other shutdown owner.
pub const CONNECTION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Handler invoked for every authorized request envelope. Returning `None`
/// means "no reply" (fire-and-forget command).
pub trait IpcHandler: Send + Sync {
    fn handle(&self, credentials: PeerCredentials, envelope: Envelope) -> Option<Envelope>;
}

impl<F> IpcHandler for F
where
    F: Fn(PeerCredentials, Envelope) -> Option<Envelope> + Send + Sync,
{
    fn handle(&self, credentials: PeerCredentials, envelope: Envelope) -> Option<Envelope> {
        self(credentials, envelope)
    }
}

type SharedWriter = Arc<Mutex<PipeStream>>;

struct ConnectionEntry {
    writer: SharedWriter,
    control: PipeStream,
    event_eligible: AtomicBool,
    protocol_minor: AtomicU16,
}

type ConnectionRegistry = Arc<Mutex<HashMap<u64, Arc<ConnectionEntry>>>>;
type WorkerRegistry = Arc<Mutex<HashMap<u64, JoinHandle<()>>>>;

struct AdmittedConnection {
    reader: PipeStream,
    writer: SharedWriter,
    entry: Arc<ConnectionEntry>,
    guard: ConnectionGuard,
}

/// The listening side of the endpoint: the one unconnected instance plus the
/// event its overlapped `ConnectNamedPipe` completes on.
struct Listener {
    instance: Option<OwnedHandle>,
    connect_event: OwnedHandle,
}

/// Bound named-pipe server owning one IPC endpoint.
///
/// The pipe name stays reserved (no other process can create its first
/// instance) while this value or any connection it admitted is alive.
pub struct WindowsIpcServer {
    pipe_path: PathBuf,
    wide_name: Vec<u16>,
    descriptor: SecurityDescriptor,
    listener: Mutex<Listener>,
    /// Manual-reset; signalled once by shutdown to wake a waiting accept.
    stop_event: OwnedHandle,
    expected_sid: UserSid,
    stop: Arc<AtomicBool>,
    lifecycle: Mutex<bool>,
    event_seq: AtomicU64,
    next_client_id: Arc<AtomicU64>,
    active_connections: Arc<AtomicUsize>,
    clients: ConnectionRegistry,
    workers: WorkerRegistry,
    /// Shared total budget armed by the first shutdown request, so connection
    /// cleanup spends the same remaining time as the rest of the service.
    shutdown_budget: ShutdownBudget,
}

impl WindowsIpcServer {
    /// Bind the canonical daemon endpoint at `pipe_path` (`\\.\pipe\...`).
    ///
    /// Connection cleanup uses [`CONNECTION_SHUTDOWN_TIMEOUT`] as its total
    /// unless the composing service installs a shared
    /// [`WindowsIpcServer::bind_with_shutdown_budget`].
    pub fn bind(pipe_path: &Path) -> Result<Self, IpcError> {
        Self::bind_with_shutdown_budget(pipe_path, ShutdownBudget::new(CONNECTION_SHUTDOWN_TIMEOUT))
    }

    /// Bind the endpoint with one shared shutdown budget.
    ///
    /// The budget is armed by the first [`WindowsIpcServer::request_shutdown`],
    /// so the connection cleanup performed inside [`WindowsIpcServer::serve`]
    /// is charged to the same total as every later daemon cleanup owner
    /// instead of receiving a fresh window.
    pub fn bind_with_shutdown_budget(
        pipe_path: &Path,
        shutdown_budget: ShutdownBudget,
    ) -> Result<Self, IpcError> {
        let name = crate::validate_pipe_path(pipe_path)?;
        let wide_name =
            sys::wide_nul(name).map_err(|_| IpcError::bad_request("invalid pipe name"))?;
        let expected_sid = sys::current_user_sid()?;
        let descriptor =
            SecurityDescriptor::from_sddl(&endpoint_sddl(&expected_sid)).map_err(|e| {
                IpcError::internal(format!("build pipe security descriptor failed: {e}"))
            })?;

        let first = sys::create_pipe_instance(&wide_name, &descriptor, true).map_err(|e| {
            match e.raw_os_error() {
                // ACCESS_DENIED: an instance exists, whoever made it.
                // PIPE_BUSY: the existing pipe has no free instance.
                Some(5) | Some(231) => IpcError::new(
                    IpcErrorCode::DaemonUnavailable,
                    format!(
                        "another narrivyd instance (or another process) holds {}",
                        pipe_path.display()
                    ),
                ),
                _ => IpcError::internal(format!("create pipe {} failed: {e}", pipe_path.display())),
            }
        })?;
        let connect_event = sys::create_event()
            .map_err(|e| IpcError::internal(format!("create event failed: {e}")))?;
        let stop_event = sys::create_event()
            .map_err(|e| IpcError::internal(format!("create event failed: {e}")))?;

        Ok(Self {
            pipe_path: pipe_path.to_path_buf(),
            wide_name,
            descriptor,
            listener: Mutex::new(Listener {
                instance: Some(first),
                connect_event,
            }),
            stop_event,
            expected_sid,
            stop: Arc::new(AtomicBool::new(false)),
            lifecycle: Mutex::new(true),
            event_seq: AtomicU64::new(0),
            next_client_id: Arc::new(AtomicU64::new(1)),
            active_connections: Arc::new(AtomicUsize::new(0)),
            clients: Arc::new(Mutex::new(HashMap::new())),
            workers: Arc::new(Mutex::new(HashMap::new())),
            shutdown_budget,
        })
    }

    /// Pipe path this server owns. Named like the Unix transport's accessor
    /// so callers switch transports with `cfg` alone.
    pub fn socket_path(&self) -> &Path {
        &self.pipe_path
    }

    /// User SID the endpoint is bound to.
    pub fn expected_sid(&self) -> UserSid {
        self.expected_sid
    }

    /// This server's shared shutdown budget, armed by the first shutdown
    /// request. The composing service reads the same instance to bound every
    /// later cleanup owner by one monotonic deadline.
    #[must_use]
    pub fn shutdown_budget(&self) -> ShutdownBudget {
        self.shutdown_budget.clone()
    }

    /// Ask the serve loop to stop; a waiting accept wakes immediately.
    ///
    /// The first request establishes the shared shutdown epoch; later requests
    /// cannot move it.
    pub fn request_shutdown(&self) {
        self.shutdown_budget.arm();
        *lock_or_recover(&self.lifecycle) = false;
        self.stop.store(true, Ordering::SeqCst);
        let _ = sys::set_event(&self.stop_event);
        self.interrupt_connections();
        // Stop new connects: close the listening instance. Waiting for the
        // listener lock is short because the stop event above ends any
        // accept wait. Admitted connections keep their own instances, so the
        // name stays reserved until the last handler exits.
        lock_or_recover(&self.listener).instance = None;
    }

    /// Close admission, interrupt every admitted connection and reap all
    /// owned connection threads within `timeout` and never past the shared
    /// shutdown deadline.
    ///
    /// A timeout leaves unfinished handles retained by this server; their
    /// pipe instances keep the endpoint name reserved until they exit.
    pub fn shutdown_and_join(&self, timeout: Duration) -> Result<(), IpcError> {
        self.request_shutdown();
        let timeout = timeout.min(self.shutdown_budget.remaining());
        let deadline = Instant::now() + timeout;
        let mut first_failure = None;
        loop {
            if let Err(err) = self.reap_finished_workers() {
                first_failure.get_or_insert(err);
            }
            if lock_or_recover(&self.workers).is_empty()
                && self.active_connections.load(Ordering::Acquire) == 0
            {
                return first_failure.map_or(Ok(()), Err);
            }
            if Instant::now() >= deadline {
                return Err(IpcError::new(
                    IpcErrorCode::Timeout,
                    format!(
                        "{} IPC connection worker(s) did not stop before deadline",
                        self.active_connections.load(Ordering::Acquire)
                    ),
                ));
            }
            self.interrupt_connections();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Number of currently admitted connection handlers. This content-free
    /// diagnostic is also used to coordinate deterministic lifecycle tests.
    pub fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn registered_connection_count(&self) -> usize {
        lock_or_recover(&self.clients).len()
    }

    #[cfg(test)]
    fn owned_worker_count(&self) -> usize {
        let _ = self.reap_finished_workers();
        lock_or_recover(&self.workers).len()
    }

    /// Accept-and-handle loop: one bounded thread per connection, running
    /// until [`WindowsIpcServer::request_shutdown`] or a fatal accept error.
    pub fn serve<H>(&self, handler: H) -> Result<(), IpcError>
    where
        H: IpcHandler + 'static,
    {
        let handler = Arc::new(handler);
        while !self.stop.load(Ordering::SeqCst) {
            if let Err(err) = self.reap_finished_workers() {
                self.request_shutdown();
                let _ = self.shutdown_and_join(CONNECTION_SHUTDOWN_TIMEOUT);
                return Err(err);
            }
            match self.accept_connection() {
                Ok(Some(stream)) => {
                    if let Err(err) = self.dispatch(stream, handler.clone()) {
                        self.request_shutdown();
                        let _ = self.shutdown_and_join(CONNECTION_SHUTDOWN_TIMEOUT);
                        return Err(err);
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    self.request_shutdown();
                    let _ = self.shutdown_and_join(CONNECTION_SHUTDOWN_TIMEOUT);
                    return Err(err);
                }
            }
        }
        self.shutdown_and_join(CONNECTION_SHUTDOWN_TIMEOUT)
    }

    /// Handle at most one incoming connection synchronously. Returns
    /// `Ok(false)` when no peer arrived within [`ACCEPT_POLL_INTERVAL`].
    /// Useful for tests and for embedding in external event loops. This path
    /// creates no detached worker: the caller's borrow owns the handler until
    /// the peer closes, while the connection remains visible to shutdown.
    pub fn accept_once<H>(&self, handler: &H) -> Result<bool, IpcError>
    where
        H: IpcHandler + ?Sized,
    {
        let Some(stream) = self.accept_connection()? else {
            return Ok(false);
        };
        let admission = lock_or_recover(&self.lifecycle);
        if !*admission {
            let mut stream = stream;
            send_rejection_direct(
                &mut stream,
                RequestId(0),
                IpcErrorCode::DaemonUnavailable,
                "server is shutting down",
            );
            return Ok(true);
        }
        let Some(admitted) = self.admit_open_connection(stream)? else {
            return Ok(true);
        };
        drop(admission);
        handle_connection(
            admitted.reader,
            admitted.writer,
            admitted.entry,
            &self.expected_sid,
            handler,
        );
        drop(admitted.guard);
        Ok(true)
    }

    /// Publish one event to every currently-connected client.
    ///
    /// The envelope carries kind `event`, `request_id 0` and a daemon-wide
    /// monotonic `event_seq`. Returns how many clients received it; a failed
    /// writer is made event-ineligible and its connection is interrupted so
    /// the owning worker performs terminal RAII cleanup.
    pub fn publish_event(&self, event: &Event) -> Result<usize, IpcError> {
        let seq = self.event_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let envelope = Envelope::new(MessageKind::Event, RequestId(0))
            .with_payload(event)?
            .with_event_seq(seq);
        let bytes = envelope.to_json_bytes()?;

        let snapshot: Vec<Arc<ConnectionEntry>> = lock_or_recover(&self.clients)
            .iter()
            .filter(|(_, entry)| {
                entry.event_eligible.load(Ordering::Acquire)
                    && (!matches!(event, Event::TranscriptPartial { .. })
                        || entry.protocol_minor.load(Ordering::Acquire)
                            >= TRANSCRIPT_PARTIAL_PROTOCOL_MINOR)
            })
            .map(|(_, entry)| entry.clone())
            .collect();
        let mut delivered = 0;
        for entry in snapshot {
            let mut guard = lock_or_recover(&entry.writer);
            // Eligibility can change after the registry snapshot but before
            // this writer is acquired. Re-check while serialized with the
            // subscription acknowledgement so an accepted Unsubscribe is a
            // strict boundary: no older snapshot can emit behind its reply.
            if !entry.event_eligible.load(Ordering::Acquire) {
                continue;
            }
            let write_result = guard
                .set_write_timeout(Some(EVENT_WRITE_TIMEOUT))
                .map_err(|error| {
                    IpcError::internal(format!("set event write timeout failed: {error}"))
                })
                .and_then(|()| write_frame(&mut *guard, &bytes));
            let restore_result = guard.set_write_timeout(Some(CONNECTION_IDLE_TIMEOUT));
            match (write_result, restore_result) {
                (Ok(()), Ok(())) => delivered += 1,
                _ => {
                    entry.event_eligible.store(false, Ordering::Release);
                    entry.control.shutdown();
                }
            }
        }
        Ok(delivered)
    }

    /// Wait up to [`ACCEPT_POLL_INTERVAL`] for one client. `Ok(None)` means
    /// no client arrived, or the server is shutting down.
    fn accept_connection(&self) -> Result<Option<PipeStream>, IpcError> {
        let mut listener = lock_or_recover(&self.listener);
        let Listener {
            instance,
            connect_event,
        } = &mut *listener;
        let Some(pending) = instance.as_ref() else {
            return Ok(None);
        };
        match sys::overlapped(
            pending,
            connect_event,
            Some(&self.stop_event),
            Some(ACCEPT_POLL_INTERVAL),
            Operation::Connect,
        ) {
            Ok(_) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == sys::CANCELLED_KIND =>
            {
                return Ok(None);
            }
            // A client opened this instance and closed it again before it was
            // accepted (it gave up waiting while the daemon was still
            // starting). Nothing was exchanged: listen on a fresh instance,
            // created before the old one is closed so the name never has none.
            Err(e) if sys::is_peer_gone(&e) => {
                let next = sys::create_pipe_instance(&self.wide_name, &self.descriptor, false)
                    .map_err(|e| IpcError::internal(format!("create pipe instance failed: {e}")))?;
                *instance = Some(next);
                return Ok(None);
            }
            Err(e) => return Err(IpcError::internal(format!("accept failed: {e}"))),
        }
        // Create the next listening instance before handing this one out, so
        // the name never has zero instances while the server runs.
        let next = sys::create_pipe_instance(&self.wide_name, &self.descriptor, false);
        let connected = instance.take();
        let next =
            next.map_err(|e| IpcError::internal(format!("create pipe instance failed: {e}")))?;
        *instance = Some(next);
        let Some(connected) = connected else {
            return Ok(None);
        };
        PipeStream::from_handle(connected)
            .map(Some)
            .map_err(|e| IpcError::internal(format!("create pipe events failed: {e}")))
    }

    fn dispatch<H>(&self, stream: PipeStream, handler: Arc<H>) -> Result<(), IpcError>
    where
        H: IpcHandler + 'static,
    {
        let admission = lock_or_recover(&self.lifecycle);
        if !*admission {
            let mut stream = stream;
            send_rejection_direct(
                &mut stream,
                RequestId(0),
                IpcErrorCode::DaemonUnavailable,
                "server is shutting down",
            );
            return Ok(());
        }
        let Some(admitted) = self.admit_open_connection(stream)? else {
            return Ok(());
        };
        let expected_sid = self.expected_sid;
        let client_id = admitted.guard.client_id;
        // A single buffered token preserves register-before-start ordering
        // without the unbounded rendezvous wait of `sync_channel(0)`.
        let (start_tx, start_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name(format!("narrivy-ipc-{client_id}"))
            .spawn(move || {
                if start_rx.recv().is_err() {
                    return;
                }
                handle_connection(
                    admitted.reader,
                    admitted.writer,
                    admitted.entry,
                    &expected_sid,
                    handler.as_ref(),
                );
                drop(admitted.guard);
            })
            .map_err(|err| IpcError::internal(format!("spawn connection worker failed: {err}")))?;
        lock_or_recover(&self.workers).insert(client_id, worker);
        // Admission remains serialized until the worker is registered. A
        // concurrent shutdown therefore cannot miss this worker or pipe.
        start_tx
            .try_send(())
            .map_err(|_| IpcError::internal("connection worker start gate failed"))?;
        drop(admission);
        self.reap_finished_workers()?;
        Ok(())
    }

    /// Admit a connection while the caller holds `lifecycle` open.
    fn admit_open_connection(
        &self,
        stream: PipeStream,
    ) -> Result<Option<AdmittedConnection>, IpcError> {
        if self.active_connections.fetch_add(1, Ordering::AcqRel) >= MAX_CONCURRENT_CONNECTIONS {
            self.active_connections.fetch_sub(1, Ordering::AcqRel);
            let mut stream = stream;
            send_rejection_direct(
                &mut stream,
                RequestId(0),
                IpcErrorCode::DaemonUnavailable,
                "connection limit reached",
            );
            return Ok(None);
        }
        let clones = stream
            .try_clone()
            .and_then(|writer| Ok((writer.try_clone()?, writer)));
        let (control, writer_stream) = match clones {
            Ok(clones) => clones,
            Err(err) => {
                self.active_connections.fetch_sub(1, Ordering::AcqRel);
                return Err(IpcError::internal(format!("stream clone failed: {err}")));
            }
        };
        let client_id = self.next_client_id.fetch_add(1, Ordering::SeqCst);
        let writer = Arc::new(Mutex::new(writer_stream));
        let entry = Arc::new(ConnectionEntry {
            writer: writer.clone(),
            control,
            event_eligible: AtomicBool::new(false),
            protocol_minor: AtomicU16::new(0),
        });
        lock_or_recover(&self.clients).insert(client_id, entry.clone());
        let guard = ConnectionGuard {
            client_id,
            clients: self.clients.clone(),
            active_connections: self.active_connections.clone(),
        };
        Ok(Some(AdmittedConnection {
            reader: stream,
            writer,
            entry,
            guard,
        }))
    }

    fn interrupt_connections(&self) {
        let clients = lock_or_recover(&self.clients);
        for entry in clients.values() {
            entry.control.shutdown();
        }
    }

    fn reap_finished_workers(&self) -> Result<(), IpcError> {
        let finished = {
            let workers = lock_or_recover(&self.workers);
            workers
                .iter()
                .filter_map(|(id, worker)| worker.is_finished().then_some(*id))
                .collect::<Vec<_>>()
        };
        let mut first_failure = None;
        for id in finished {
            let worker = lock_or_recover(&self.workers).remove(&id);
            if worker.is_some_and(|worker| worker.join().is_err()) {
                first_failure.get_or_insert_with(|| {
                    IpcError::internal(format!("IPC connection worker {id} panicked"))
                });
            }
        }
        first_failure.map_or(Ok(()), Err)
    }
}

impl Drop for WindowsIpcServer {
    fn drop(&mut self) {
        self.request_shutdown();
        let _ = self.reap_finished_workers();
        // Unfinished workers keep their pipe instances, so the endpoint name
        // stays reserved and no overlapping server generation can bind.
    }
}

/// Kernel-reported identity of the client process behind a server pipe end.
fn peer_credentials(stream: &PipeStream) -> Result<PeerCredentials, IpcError> {
    let pid = sys::pipe_client_process_id(stream.handle()).map_err(|err| {
        IpcError::permission_denied(format!("client process id unavailable: {err}"))
    })?;
    Ok(PeerCredentials {
        sid: sys::process_user_sid(pid)?,
        pid: Some(pid),
    })
}

/// Write one typed handshake rejection frame and end the connection.
fn write_handshake_rejection(writer: &SharedWriter, code: IpcErrorCode, detail: String) {
    let response = HandshakeResponse::Rejected { code, detail };
    let bytes = serde_json::to_vec(&response).unwrap_or_default();
    let _ = write_shared(writer, &bytes);
}

/// Write one typed rejection envelope; failures are ignored because the
/// peer may already be gone.
fn send_rejection(
    writer: &SharedWriter,
    request_id: RequestId,
    code: IpcErrorCode,
    detail: impl Into<String>,
) {
    let envelope =
        Envelope::new(MessageKind::Response, request_id).with_payload(ResponsePayload::Rejected {
            code,
            detail: detail.into(),
        });
    let bytes = match envelope.and_then(|envelope| envelope.to_json_bytes()) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    let _ = write_shared(writer, &bytes);
}

fn send_rejection_direct(
    stream: &mut PipeStream,
    request_id: RequestId,
    code: IpcErrorCode,
    detail: impl Into<String>,
) {
    let envelope =
        Envelope::new(MessageKind::Response, request_id).with_payload(ResponsePayload::Rejected {
            code,
            detail: detail.into(),
        });
    let bytes = match envelope.and_then(|envelope| envelope.to_json_bytes()) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    // A peer that never reads cannot hold the accept loop.
    if stream.set_write_timeout(Some(EVENT_WRITE_TIMEOUT)).is_ok() {
        let _ = write_frame(stream, &bytes);
    }
}

fn write_shared(writer: &SharedWriter, bytes: &[u8]) -> Result<(), IpcError> {
    write_frame(&mut *lock_or_recover(writer), bytes)
}

/// Counts the bytes a frame read consumed, so a quiet timeout can be told
/// apart from one that stopped mid-frame.
struct CountingRead<'a> {
    inner: &'a mut PipeStream,
    received: usize,
}

impl std::io::Read for CountingRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.received += read;
        Ok(read)
    }
}

/// Per-connection protocol loop.
///
/// Order is security-critical: transport-level peer authorization happens
/// BEFORE any frame is read or any handler runs (ADR-0011). Unauthorized or
/// unverifiable peers get exactly one PermissionDenied envelope and a close.
fn handle_connection<H>(
    mut stream: PipeStream,
    writer: SharedWriter,
    entry: Arc<ConnectionEntry>,
    expected_sid: &UserSid,
    handler: &H,
) where
    H: IpcHandler + ?Sized,
{
    let _ = stream.set_read_timeout(Some(CONNECTION_IDLE_TIMEOUT));
    let _ = lock_or_recover(&writer).set_write_timeout(Some(CONNECTION_IDLE_TIMEOUT));

    let credentials = match peer_credentials(&stream) {
        Ok(credentials) => credentials,
        Err(err) => {
            send_rejection(
                &writer,
                RequestId(0),
                IpcErrorCode::PermissionDenied,
                format!("peer credentials unavailable: {err}"),
            );
            return;
        }
    };
    if let Err(err) = credentials.validate(expected_sid) {
        send_rejection(
            &writer,
            RequestId(0),
            IpcErrorCode::PermissionDenied,
            format!("cross-user connection denied: {err}"),
        );
        return;
    }

    // Handshake phase (ipc.md "Handshake"): the FIRST frame must be a
    // versioned hello; anything else ends the connection with a typed
    // rejection. Major skew rejects before any command can be issued.
    let hello_bytes = match read_frame(&mut stream) {
        Ok(bytes) => bytes,
        Err(err) => {
            if err.code != IpcErrorCode::Cancelled {
                send_rejection(
                    &writer,
                    RequestId(0),
                    err.code.clone(),
                    format!("transport error: {err}"),
                );
            }
            return;
        }
    };
    match serde_json::from_slice::<HandshakeHello>(&hello_bytes) {
        Ok(hello) => {
            if let Err(err) = hello.validate() {
                write_handshake_rejection(&writer, err.code, err.message);
                return;
            }
            let negotiated = negotiate_handshake(&hello);
            let client_protocol_minor = hello.protocol_minor;
            match negotiated {
                HandshakeResponse::Accepted { .. } => {
                    let response = negotiated;
                    let bytes = match serde_json::to_vec(&response) {
                        Ok(bytes) => bytes,
                        Err(err) => {
                            write_handshake_rejection(
                                &writer,
                                IpcErrorCode::Internal,
                                format!("handshake encode failed: {err}"),
                            );
                            return;
                        }
                    };
                    if write_shared(&writer, &bytes).is_err() {
                        return;
                    }
                    entry
                        .protocol_minor
                        .store(client_protocol_minor, Ordering::Release);
                }
                HandshakeResponse::Rejected { code, detail } => {
                    write_handshake_rejection(&writer, code, detail);
                    return;
                }
            }
        }
        Err(err) => {
            write_handshake_rejection(
                &writer,
                IpcErrorCode::BadRequest,
                format!("handshake decode failed: {err}"),
            );
            return;
        }
    }

    loop {
        let mut reader = CountingRead {
            inner: &mut stream,
            received: 0,
        };
        let result = read_frame(&mut reader);
        let received = reader.received;
        let bytes = match result {
            Ok(bytes) => bytes,
            // A subscriber only listens, so a quiet read is normal for it.
            // Closing it here would drop every event published before it
            // reconnects. A timeout that consumed part of a frame still
            // ends the connection, because framing cannot be recovered.
            Err(err)
                if err.code == IpcErrorCode::Timeout
                    && received == 0
                    && entry.event_eligible.load(Ordering::Acquire) =>
            {
                continue;
            }
            // Polite EOF: done. Anything else gets one typed rejection.
            Err(err) => {
                if err.code != IpcErrorCode::Cancelled {
                    send_rejection(
                        &writer,
                        RequestId(0),
                        err.code.clone(),
                        format!("transport error: {err}"),
                    );
                }
                break;
            }
        };
        let envelope = match Envelope::from_json_bytes(&bytes) {
            Ok(envelope) => envelope,
            Err(err) => {
                send_rejection(
                    &writer,
                    RequestId(0),
                    err.code,
                    format!("malformed message: {}", err.message),
                );
                break;
            }
        };
        if !envelope.kind.is_request() {
            send_rejection(
                &writer,
                envelope.request_id,
                IpcErrorCode::BadRequest,
                format!("kind {:?} is not client-sendable", envelope.kind.wire_tag()),
            );
            break;
        }
        let subscription_change = match envelope.parse_payload::<Command>() {
            Ok(Command::Subscribe) if envelope.kind == MessageKind::Subscribe => Some(true),
            Ok(Command::Unsubscribe) if envelope.kind == MessageKind::Unsubscribe => Some(false),
            _ => None,
        };
        if let Some(reply) = handler.handle(credentials, envelope) {
            // Re-validate daemon-generated replies against the same bounds.
            if let Err(err) = reply.validate() {
                send_rejection(
                    &writer,
                    RequestId(0),
                    err.code,
                    format!("handler reply rejected: {}", err.message),
                );
                continue;
            }
            let accepted = matches!(
                reply.parse_payload::<ResponsePayload>(),
                Ok(ResponsePayload::Accepted)
            );
            match reply.to_json_bytes() {
                Ok(bytes) => {
                    let mut writer_guard = lock_or_recover(&writer);
                    // Publish the transition before its response while the
                    // shared writer is held. A concurrent event either wins
                    // before an Unsubscribe acknowledgement, or re-checks
                    // eligibility and stays silent afterward. Subscribe uses
                    // the same ordering so any newly eligible event follows
                    // its acknowledgement on the byte stream.
                    if accepted {
                        if let Some(eligible) = subscription_change {
                            entry.event_eligible.store(eligible, Ordering::Release);
                        }
                    }
                    if write_frame(&mut *writer_guard, &bytes).is_err() {
                        entry.event_eligible.store(false, Ordering::Release);
                        break;
                    }
                }
                Err(err) => send_rejection(
                    &writer,
                    RequestId(0),
                    err.code,
                    format!("handler reply rejected: {}", err.message),
                ),
            }
        }
    }
}

struct ConnectionGuard {
    client_id: u64,
    clients: ConnectionRegistry,
    active_connections: Arc<AtomicUsize>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        lock_or_recover(&self.clients).remove(&self.client_id);
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Poison recovery is deliberate: a panicked peer must not disable endpoint
/// shutdown or event publication for every other connection.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod ipc_tests {
    use super::*;
    use crate::client::IpcClient;
    use crate::pipe::test_support::{connect_raw, pair, unique_pipe_path};
    use narrivy_ipc::protocol::{
        ActiveSessionDto, PROTOCOL_MAJOR, PROTOCOL_MINOR, PipelineSummaryDto, StatusDto,
    };
    use narrivy_ipc::{BUILD_IDENTITY, Command};
    use std::error::Error;

    /// Thread-safe test error: spawned helper threads return this type.
    type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn reply_envelope(request_id: RequestId, payload: &ResponsePayload) -> Option<Envelope> {
        Envelope::new(MessageKind::Response, request_id)
            .with_payload(payload)
            .ok()
    }

    fn join_serve_thread(handle: std::thread::JoinHandle<Result<(), IpcError>>) -> TestResult {
        let inner = handle.join().map_err(|_| "server thread panicked")?;
        inner.map_err(|e| format!("serve failed: {e}").into())
    }

    /// Publish with retries until at least one connected client accepts the
    /// event; tolerates accept-lag between client connect and registration.
    fn publish_until_accepted(server: &WindowsIpcServer, event: &Event) -> TestResult {
        for _ in 0..300 {
            if server.publish_event(event)? > 0 {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err("event was never accepted by a client".into())
    }

    fn status(daemon_build: impl Into<String>) -> ResponsePayload {
        ResponsePayload::Status(StatusDto {
            daemon_build: daemon_build.into(),
            protocol_minor: PROTOCOL_MINOR,
            active_session: None,
            last_session_timings_us: None,
            capabilities: vec![],
            installed_model: None,
            model_provisioning: None,
            portal_keyboard: None,
        })
    }

    fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + TEST_TIMEOUT;
        while !condition() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    /// A client that opens the pipe while the daemon is still starting and
    /// gives up before it is accepted must not stop the server.
    #[test]
    fn a_client_that_left_before_accept_does_not_stop_the_server() -> TestResult {
        let path = unique_pipe_path("left-early");
        let server = Arc::new(WindowsIpcServer::bind(&path)?);
        // Opened and closed before `serve` ever accepts.
        drop(connect_raw(&path)?);

        let srv = server.clone();
        let serve_thread = std::thread::spawn(move || {
            srv.serve(move |_credentials: PeerCredentials, envelope: Envelope| {
                match envelope.kind {
                    MessageKind::GetStatus => {
                        reply_envelope(envelope.request_id, &status("narrivyd-test-0.0.0"))
                    }
                    _ => None,
                }
            })
        });

        let mut client = IpcClient::connect(&path)?;
        assert!(matches!(
            client.request(&Command::GetStatus, TEST_TIMEOUT)?,
            ResponsePayload::Status(_)
        ));
        drop(client);
        server.request_shutdown();
        join_serve_thread(serve_thread)
    }

    #[test]
    fn ipc_end_to_end_status_and_event_over_named_pipe() -> TestResult {
        let path = unique_pipe_path("e2e");
        let server = Arc::new(WindowsIpcServer::bind(&path)?);
        let own_sid = sys::current_user_sid()?;
        assert_eq!(server.expected_sid(), own_sid);
        assert_eq!(server.socket_path(), path.as_path());

        let srv = server.clone();
        let serve_thread = std::thread::spawn(move || {
            srv.serve(move |credentials: PeerCredentials, envelope: Envelope| {
                // Same-user policy is already enforced before this runs.
                if !credentials.is_same_user(&own_sid) || credentials.pid.is_none() {
                    return None;
                }
                let request_id = envelope.request_id;
                match envelope.kind {
                    MessageKind::GetStatus => reply_envelope(
                        request_id,
                        &ResponsePayload::Status(StatusDto {
                            daemon_build: "narrivyd-test-0.0.0".into(),
                            protocol_minor: PROTOCOL_MINOR,
                            active_session: Some(ActiveSessionDto {
                                session_id: 42,
                                transaction_id: Some("tx-abc".into()),
                                pipeline_id: Some("default".into()),
                                state: "listening".into(),
                                stage_timings_us: None,
                            }),
                            last_session_timings_us: None,
                            capabilities: vec!["asr.local".into(), "pipelines".into()],
                            installed_model: None,
                            model_provisioning: None,
                            portal_keyboard: None,
                        }),
                    ),
                    MessageKind::ListPipelines => reply_envelope(
                        request_id,
                        &ResponsePayload::Pipelines(vec![PipelineSummaryDto {
                            id: "default".into(),
                            name: "Default dictation pipeline".into(),
                            version: 1,
                            enabled: true,
                            model_id: None,
                            compute: None,
                            refine: None,
                        }]),
                    ),
                    MessageKind::Subscribe | MessageKind::Unsubscribe => {
                        reply_envelope(request_id, &ResponsePayload::Accepted)
                    }
                    _ => reply_envelope(
                        request_id,
                        &ResponsePayload::Rejected {
                            code: IpcErrorCode::UnknownCommand,
                            detail: "unsupported in test handler".into(),
                        },
                    ),
                }
            })
        });

        let mut client = IpcClient::connect(&path)?;
        assert_eq!(client.daemon_build(), BUILD_IDENTITY);
        assert_eq!(client.negotiated_minor(), PROTOCOL_MINOR);

        let ResponsePayload::Status(dto) = client.request(&Command::GetStatus, TEST_TIMEOUT)?
        else {
            return Err("expected Status payload".into());
        };
        assert_eq!(dto.daemon_build, "narrivyd-test-0.0.0");
        let active = dto.active_session.ok_or("active session must be set")?;
        assert_eq!(active.session_id, 42u128);
        assert_eq!(active.transaction_id.as_deref(), Some("tx-abc"));

        assert_eq!(
            client.request(&Command::Subscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
        let event = Event::SessionStateChanged {
            session_id: 42,
            transaction_id: Some("tx-abc".into()),
            pipeline_id: Some("default".into()),
            state: "listening".into(),
            seq: 1,
            delivery: None,
        };
        publish_until_accepted(&server, &event)?;
        let received = client.next_event_with_sequence(TEST_TIMEOUT)?;
        assert_eq!(received.event, event);
        assert!(received.event_seq >= 1);

        let ResponsePayload::Pipelines(list) =
            client.request(&Command::ListPipelines, TEST_TIMEOUT)?
        else {
            return Err("expected Pipelines payload".into());
        };
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "default");

        assert_eq!(
            client.request(&Command::Unsubscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
        assert_eq!(server.publish_event(&event)?, 0);

        // A dropped transport reconnects to the same endpoint.
        client.reconnect()?;
        assert!(matches!(
            client.request(&Command::GetStatus, TEST_TIMEOUT)?,
            ResponsePayload::Status(_)
        ));

        drop(client);
        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        assert_eq!(server.active_connection_count(), 0);
        assert_eq!(server.owned_worker_count(), 0);
        Ok(())
    }

    #[test]
    fn a_quiet_subscriber_stays_connected_and_a_quiet_requester_does_not() -> TestResult {
        let path = unique_pipe_path("quiet");
        let server = Arc::new(WindowsIpcServer::bind(&path)?);
        let srv = server.clone();
        let serve_thread = std::thread::spawn(move || {
            srv.serve(|_credentials: PeerCredentials, envelope: Envelope| {
                reply_envelope(envelope.request_id, &ResponsePayload::Accepted)
            })
        });
        let mut subscriber = IpcClient::connect(&path)?;
        assert_eq!(
            subscriber.request(&Command::Subscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
        let mut requester = IpcClient::connect(&path)?;
        assert_eq!(
            requester.request(&Command::GetStatus, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );

        std::thread::sleep(CONNECTION_IDLE_TIMEOUT * 4);
        let event = Event::HealthChanged {
            module_id: "quiet".into(),
            healthy: true,
        };
        assert_eq!(server.publish_event(&event)?, 1);
        assert_eq!(subscriber.next_event(TEST_TIMEOUT)?, event);
        assert!(
            requester
                .request(&Command::GetStatus, TEST_TIMEOUT)
                .is_err()
        );

        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        Ok(())
    }

    #[test]
    fn ipc_request_ids_correlate_across_interleaved_events() -> TestResult {
        let path = unique_pipe_path("corr");
        let server = Arc::new(WindowsIpcServer::bind(&path)?);
        let srv = server.clone();
        let serve_thread = std::thread::spawn(move || {
            srv.serve(|_credentials: PeerCredentials, envelope: Envelope| {
                if envelope.kind == MessageKind::Subscribe {
                    return reply_envelope(envelope.request_id, &ResponsePayload::Accepted);
                }
                std::thread::sleep(Duration::from_millis(120));
                let request_id = envelope.request_id;
                reply_envelope(request_id, &status(format!("reply-for-{request_id}")))
            })
        });

        let mut client = IpcClient::connect(&path)?;
        assert_eq!(
            client.request(&Command::Subscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
        let srv2 = server.clone();
        let publisher = std::thread::spawn(move || -> TestResult {
            for seq in [1u64, 2] {
                let event = Event::HealthChanged {
                    module_id: format!("module-{seq}"),
                    healthy: true,
                };
                publish_until_accepted(&srv2, &event)?;
                std::thread::sleep(Duration::from_millis(40));
            }
            Ok(())
        });

        let first = client.request(&Command::GetStatus, TEST_TIMEOUT)?;
        let second = client.request(&Command::GetStatus, TEST_TIMEOUT)?;
        publisher.join().map_err(|_| "publisher panicked")??;
        assert_eq!(first, status("reply-for-2"));
        assert_eq!(second, status("reply-for-3"));
        for seq in [1u64, 2] {
            let event = client.next_event(TEST_TIMEOUT)?;
            let Event::HealthChanged { module_id, .. } = event else {
                return Err(format!("unexpected event {event:?}").into());
            };
            assert_eq!(module_id, format!("module-{seq}"));
        }
        let err = match client.next_event(Duration::from_millis(250)) {
            Err(err) => err,
            Ok(event) => return Err(format!("unexpected extra event {event:?}").into()),
        };
        assert_eq!(err.code, IpcErrorCode::Timeout);

        drop(client);
        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        Ok(())
    }

    #[test]
    fn ipc_concurrent_event_and_response_frames_remain_intact() -> TestResult {
        const ROUNDS: usize = 16;
        // Larger than the pipe buffer, so every event needs several writes.
        const EVENT_PAD_BYTES: usize = 96 * 1024;
        let path = unique_pipe_path("serialized-writes");
        let server = Arc::new(WindowsIpcServer::bind(&path)?);
        let write_race = Arc::new(std::sync::Barrier::new(2));
        let handler_race = write_race.clone();
        let serving = server.clone();
        let serve_thread = std::thread::spawn(move || {
            serving.serve(move |_peer: PeerCredentials, envelope: Envelope| {
                if envelope.kind == MessageKind::Subscribe {
                    return reply_envelope(envelope.request_id, &ResponsePayload::Accepted);
                }
                handler_race.wait();
                reply_envelope(envelope.request_id, &ResponsePayload::Accepted)
            })
        });

        let mut client = IpcClient::connect_as(&path, "write-race-test")?;
        assert_eq!(
            client.request(&Command::Subscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
        // Event writes are bounded by EVENT_WRITE_TIMEOUT; the client reads
        // them while it waits for each response, so none may be dropped.
        let publishing = server.clone();
        let publisher = std::thread::spawn(move || -> Result<usize, IpcError> {
            let mut delivered = 0;
            for round in 0..ROUNDS {
                write_race.wait();
                delivered += publishing.publish_event(&Event::HealthChanged {
                    module_id: format!("serialized-writes-{round}-{}", "x".repeat(EVENT_PAD_BYTES)),
                    healthy: true,
                })?;
            }
            Ok(delivered)
        });
        for _ in 0..ROUNDS {
            assert_eq!(
                client.request(&Command::GetStatus, TEST_TIMEOUT)?,
                ResponsePayload::Accepted
            );
        }
        let delivered = publisher.join().map_err(|_| "publisher panicked")??;
        for round in 0..delivered {
            let expected_id = format!("serialized-writes-{round}-{}", "x".repeat(EVENT_PAD_BYTES));
            assert!(matches!(
                client.next_event(TEST_TIMEOUT)?,
                Event::HealthChanged { module_id, healthy }
                    if module_id == expected_id && healthy
            ));
        }

        drop(client);
        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        Ok(())
    }

    #[test]
    fn ipc_server_second_instance_rejected_and_name_reusable_after_drop() -> TestResult {
        let path = unique_pipe_path("single-instance");
        let first = WindowsIpcServer::bind(&path)?;
        let second = match WindowsIpcServer::bind(&path) {
            Err(second) => second,
            Ok(_) => return Err("second bind must fail while first instance lives".into()),
        };
        assert_eq!(second.code, IpcErrorCode::DaemonUnavailable);
        assert!(second.to_string().contains("another narrivyd instance"));

        drop(first);
        WindowsIpcServer::bind(&path)?;
        Ok(())
    }

    #[test]
    fn ipc_server_refuses_a_name_that_already_has_an_instance() -> TestResult {
        // Stands in for a squatter: someone else created the name first.
        let (path, _squatter) = crate::pipe::test_support::listening_instance("squatted")
            .map_err(|error| error.to_string())?;
        let err = WindowsIpcServer::bind(&path).err().map(|error| error.code);
        assert_eq!(err, Some(IpcErrorCode::DaemonUnavailable));
        Ok(())
    }

    #[test]
    fn ipc_server_rejects_non_pipe_paths() {
        for path in [r"C:\narrivy\narrivyd.sock", r"\\server\pipe\narrivy"] {
            let err = WindowsIpcServer::bind(Path::new(path))
                .err()
                .map(|error| error.code);
            assert_eq!(err, Some(IpcErrorCode::BadRequest), "{path}");
        }
    }

    #[test]
    fn ipc_shutdown_closes_admission_and_keeps_name_until_handlers_exit() -> TestResult {
        let path = unique_pipe_path("shutdown");
        let server = Arc::new(WindowsIpcServer::bind(&path)?);
        let serving = server.clone();
        let serve_thread = std::thread::spawn(move || {
            serving.serve(|_peer: PeerCredentials, envelope: Envelope| {
                reply_envelope(envelope.request_id, &ResponsePayload::Accepted)
            })
        });
        let mut client = IpcClient::connect(&path)?;
        assert_eq!(
            client.request(&Command::GetStatus, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );

        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        // The admitted connection was interrupted, not left half-open.
        assert!(client.request(&Command::GetStatus, TEST_TIMEOUT).is_err());
        // No listening instance remains: a new client cannot get in.
        let late =
            IpcClient::connect_as_until(&path, "late", Instant::now() + Duration::from_millis(300));
        assert!(late.is_err());
        drop(client);
        drop(server);
        // Every instance is closed, so the name is free for a new generation.
        WindowsIpcServer::bind(&path)?;
        Ok(())
    }

    #[test]
    fn ipc_server_accept_once_handles_single_connection() -> TestResult {
        let path = unique_pipe_path("once");
        let server = WindowsIpcServer::bind(&path)?;
        let handled = server.accept_once(&_no_op_handler())?;
        assert!(!handled);

        let connect_path = path.clone();
        let connector = std::thread::spawn(move || -> Result<ResponsePayload, IpcError> {
            let mut client = IpcClient::connect(&connect_path)?;
            client.request(&Command::GetStatus, TEST_TIMEOUT)
        });
        let deadline = Instant::now() + TEST_TIMEOUT;
        while !server.accept_once(&_echo_status_handler())? {
            if Instant::now() >= deadline {
                return Err("client never arrived before the test deadline".into());
            }
        }
        let response = connector.join().map_err(|_| "connector panicked")??;
        assert_eq!(response, status("accept-once"));
        Ok(())
    }

    #[test]
    fn ipc_events_are_ineligible_until_handshake_is_accepted() -> TestResult {
        let path = unique_pipe_path("pre-handshake");
        let server = Arc::new(WindowsIpcServer::bind(&path)?);
        let serving = server.clone();
        let serve_thread = std::thread::spawn(move || {
            serving.serve(|_peer: PeerCredentials, _envelope: Envelope| None)
        });

        let mut raw = connect_raw(&path)?;
        assert!(wait_until(|| server.active_connection_count() == 1));
        assert_eq!(server.registered_connection_count(), 1);
        assert_eq!(
            server.publish_event(&Event::HealthChanged {
                module_id: "pre-handshake".to_owned(),
                healthy: true,
            })?,
            0,
            "pre-handshake peers must never receive event frames"
        );

        let hello = HandshakeHello::new("ordering-test");
        write_frame(&mut raw, &serde_json::to_vec(&hello)?)?;
        let first = read_frame(&mut raw)?;
        assert!(matches!(
            serde_json::from_slice::<HandshakeResponse>(&first)?,
            HandshakeResponse::Accepted { .. }
        ));
        assert_eq!(
            server.publish_event(&Event::HealthChanged {
                module_id: "post-handshake-unsubscribed".to_owned(),
                healthy: true,
            })?,
            0,
            "handshake alone must not subscribe a client"
        );

        drop(raw);
        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        assert_eq!(server.active_connection_count(), 0);
        assert_eq!(server.registered_connection_count(), 0);
        assert_eq!(server.owned_worker_count(), 0);
        Ok(())
    }

    #[test]
    fn ipc_malformed_handshake_has_terminal_registry_and_worker_cleanup() -> TestResult {
        let path = unique_pipe_path("malformed-handshake");
        let server = Arc::new(WindowsIpcServer::bind(&path)?);
        let serving = server.clone();
        let serve_thread = std::thread::spawn(move || {
            serving.serve(|_peer: PeerCredentials, _envelope: Envelope| None)
        });

        let mut raw = connect_raw(&path)?;
        write_frame(&mut raw, b"not-json")?;
        // The rejection must still be readable after the server closed its
        // end of the instance.
        let rejection = read_frame(&mut raw)?;
        assert!(matches!(
            serde_json::from_slice::<HandshakeResponse>(&rejection)?,
            HandshakeResponse::Rejected {
                code: IpcErrorCode::BadRequest,
                ..
            }
        ));
        drop(raw);

        assert!(wait_until(|| server.active_connection_count() == 0));
        assert_eq!(server.registered_connection_count(), 0);
        assert!(wait_until(|| server.owned_worker_count() == 0));

        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        Ok(())
    }

    #[test]
    fn ipc_minor_two_subscriber_receives_lifecycle_but_not_partial_variant() -> TestResult {
        let server = WindowsIpcServer::bind(&unique_pipe_path("minor-filter"))?;
        let (writer_stream, mut reader_stream) = pair().map_err(|error| error.to_string())?;
        reader_stream.set_read_timeout(Some(TEST_TIMEOUT))?;
        let entry = Arc::new(ConnectionEntry {
            control: writer_stream.try_clone()?,
            writer: Arc::new(Mutex::new(writer_stream)),
            event_eligible: AtomicBool::new(true),
            protocol_minor: AtomicU16::new(2),
        });
        lock_or_recover(&server.clients).insert(1, entry);

        assert_eq!(
            server.publish_event(&Event::TranscriptPartial {
                session_id: 7,
                transaction_id: "tx-minor-two".to_owned(),
                pipeline_id: "streaming".to_owned(),
                seq: 1,
                text: "compatibility canary".to_owned(),
            })?,
            0
        );
        assert_eq!(
            server.publish_event(&Event::HealthChanged {
                module_id: "lifecycle-compatible".to_owned(),
                healthy: true,
            })?,
            1
        );
        let bytes = read_frame(&mut reader_stream)?;
        let envelope = Envelope::from_json_bytes(&bytes)?;
        assert!(matches!(
            envelope.parse_payload::<Event>()?,
            Event::HealthChanged { module_id, healthy }
                if module_id == "lifecycle-compatible" && healthy
        ));
        lock_or_recover(&server.clients).clear();
        Ok(())
    }

    #[test]
    fn stalled_subscriber_is_dropped_within_the_event_write_bound() -> TestResult {
        let server = WindowsIpcServer::bind(&unique_pipe_path("stalled"))?;
        let (writer_stream, reader_stream) = pair().map_err(|error| error.to_string())?;
        let entry = Arc::new(ConnectionEntry {
            control: writer_stream.try_clone()?,
            writer: Arc::new(Mutex::new(writer_stream)),
            event_eligible: AtomicBool::new(true),
            protocol_minor: AtomicU16::new(PROTOCOL_MINOR),
        });
        lock_or_recover(&server.clients).insert(1, entry.clone());
        // Nobody reads `reader_stream`: the pipe buffer fills, the bounded
        // write times out, and the subscriber is made ineligible.
        let event = Event::HealthChanged {
            module_id: "x".repeat(256 * 1024),
            healthy: true,
        };
        let started = Instant::now();
        assert_eq!(server.publish_event(&event)?, 0);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!entry.event_eligible.load(Ordering::Acquire));
        assert_eq!(server.publish_event(&event)?, 0);
        drop(reader_stream);
        lock_or_recover(&server.clients).clear();
        Ok(())
    }

    #[test]
    fn shared_shutdown_budget_bounds_connection_cleanup() -> TestResult {
        let path = unique_pipe_path("shared-budget");
        let budget = ShutdownBudget::new(Duration::from_millis(300));
        let server = WindowsIpcServer::bind_with_shutdown_budget(&path, budget.clone())?;
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let handler_release = release_rx.clone();
        let handler = Arc::new(move |_peer: PeerCredentials, envelope: Envelope| {
            let _ = entered_tx.send(());
            let _ = lock_or_recover(&handler_release).recv();
            reply_envelope(envelope.request_id, &ResponsePayload::Accepted)
        });

        let client_path = path.clone();
        let client = std::thread::spawn(move || {
            let mut client = IpcClient::connect_as(&client_path, "budget-test")?;
            client.request(&Command::GetStatus, TEST_TIMEOUT)
        });
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            if let Some(stream) = server.accept_connection()? {
                server.dispatch(stream, handler.clone())?;
                break;
            }
            if Instant::now() >= deadline {
                return Err("client never arrived".into());
            }
        }
        entered_rx.recv_timeout(TEST_TIMEOUT)?;

        let started = Instant::now();
        let err = match server.shutdown_and_join(TEST_TIMEOUT) {
            Err(err) => err,
            Ok(()) => return Err("a blocked handler cannot be reported as cleanly reaped".into()),
        };
        let elapsed = started.elapsed();
        assert_eq!(err.code, IpcErrorCode::Timeout);
        assert!(
            elapsed <= Duration::from_millis(700),
            "connection cleanup spent {elapsed:?} past the shared budget"
        );
        assert!(budget.expired());

        release_tx.send(())?;
        let _ = client.join().map_err(|_| "client panicked")?;
        assert!(wait_until(|| server.active_connection_count() == 0));
        Ok(())
    }

    fn _no_op_handler() -> impl Fn(PeerCredentials, Envelope) -> Option<Envelope> + Send + Sync {
        |_credentials: PeerCredentials, _envelope: Envelope| None
    }

    fn _echo_status_handler() -> impl Fn(PeerCredentials, Envelope) -> Option<Envelope> + Send + Sync
    {
        |credentials: PeerCredentials, envelope: Envelope| {
            let same_user = sys::current_user_sid()
                .map(|sid| credentials.is_same_user(&sid))
                .unwrap_or(false);
            if !same_user {
                return None;
            }
            reply_envelope(envelope.request_id, &status("accept-once"))
        }
    }

    /// Transport constants stay pinned so drift from the Unix transport
    /// fails loudly here.
    #[test]
    fn ipc_transport_constants_pinned() {
        assert_eq!(PROTOCOL_MAJOR, 1);
        assert_eq!(MAX_CONCURRENT_CONNECTIONS, 64);
        assert_eq!(EVENT_WRITE_TIMEOUT, Duration::from_millis(50));
        assert_eq!(CONNECTION_SHUTDOWN_TIMEOUT, Duration::from_secs(2));
    }
}
