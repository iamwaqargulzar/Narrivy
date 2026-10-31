//! Blocking unix-socket IPC server (`UnixIpcServer`) for narrivyd.
//!
//! Implements the endpoint lifecycle of ADR-0011/ADR-0012:
//!
//! - `0700` runtime-directory creation/tightening plus ownership checks;
//! - non-blocking `flock` single-instance lock file `<dir>/narrivyd.lock`;
//! - conservative stale-socket cleanup driven by [`SocketPathValidation`];
//! - `0600` socket mode verified after bind, directory fsynced;
//! - peer credentials read and validated **before** any privileged handling;
//! - bounded threads (one per connection, hard cap), idle timeouts, and a
//!   typed rejection envelope for every policy violation.
//!
//! Salvage-adapted from the v0.4 scaffold semantics (same-user validation,
//! secure modes, stale-path decision rules) with P1 additions: event
//! publication with monotonic sequence numbers and bounded concurrency.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::security::{
    EndpointPermissions, PeerCredentials, SocketPathValidation, current_uid, peer_credentials,
};
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

/// Poll interval for the accept loop so shutdown flags stay responsive.
pub const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum time a normal server shutdown waits for admitted connections.
///
/// This is the default total only; a composing service may install one shared
/// [`ShutdownBudget`] so connection cleanup spends the same remaining time as
/// every other shutdown owner.
pub const CONNECTION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Single-instance lock file name inside the socket directory (ADR-0012).
pub const LOCK_FILE_NAME: &str = "narrivyd.lock";

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

/// Bound unix-socket server owning one IPC endpoint.
///
/// The `flock` on `<dir>/narrivyd.lock` is held for the whole lifetime of
/// this value; dropping it releases the lock. The lock file itself is
/// intentionally kept on disk (removing live lock files re-introduces
/// instance races; see ADR-0012).
type SharedWriter = Arc<Mutex<UnixStream>>;

struct ConnectionEntry {
    writer: SharedWriter,
    control: UnixStream,
    event_eligible: AtomicBool,
    protocol_minor: AtomicU16,
}

type ConnectionRegistry = Arc<Mutex<HashMap<u64, Arc<ConnectionEntry>>>>;
type WorkerRegistry = Arc<Mutex<HashMap<u64, JoinHandle<()>>>>;

struct AdmittedConnection {
    reader: UnixStream,
    writer: SharedWriter,
    entry: Arc<ConnectionEntry>,
    guard: ConnectionGuard,
}

struct EndpointLease {
    _instance_lock: File,
    socket_path: PathBuf,
    bound_socket: Mutex<Option<SocketIdentity>>,
    // Keep the original socket inode allocated until the last admitted
    // handler releases the lease, preventing unlink/rebind inode reuse from
    // defeating the dev+inode cleanup identity check.
    listener_guard: Mutex<Option<UnixListener>>,
}

#[derive(Clone, Copy)]
struct SocketIdentity {
    device: u64,
    inode: u64,
    owner_uid: u32,
}

impl EndpointLease {
    fn cleanup_owned_socket(&self) -> Result<(), IpcError> {
        let Some(expected) = *lock_or_recover(&self.bound_socket) else {
            return Ok(());
        };
        let metadata = match std::fs::symlink_metadata(&self.socket_path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(IpcError::internal(format!(
                    "lstat owned socket {} failed: {err}",
                    self.socket_path.display()
                )));
            }
        };
        let same_identity = !metadata.file_type().is_symlink()
            && metadata.file_type().is_socket()
            && metadata.dev() == expected.device
            && metadata.ino() == expected.inode
            && metadata.uid() == expected.owner_uid;
        if !same_identity {
            return Err(IpcError::permission_denied(format!(
                "refusing to remove replaced endpoint {}",
                self.socket_path.display()
            )));
        }
        std::fs::remove_file(&self.socket_path)
            .map_err(|err| IpcError::internal(format!("remove owned socket failed: {err}")))?;
        *lock_or_recover(&self.bound_socket) = None;
        Ok(())
    }
}

impl Drop for EndpointLease {
    fn drop(&mut self) {
        let _ = self.cleanup_owned_socket();
    }
}

pub struct UnixIpcServer {
    listener: UnixListener,
    endpoint_lease: Arc<EndpointLease>,
    expected_uid: u32,
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
    #[cfg(test)]
    lifecycle_test_hook: Mutex<Option<LifecycleTestHook>>,
    #[cfg(test)]
    event_publish_test_hook: Mutex<Option<EventPublishTestHook>>,
}

#[cfg(test)]
struct LifecycleTestHook {
    admission_registered: std::sync::mpsc::SyncSender<()>,
    release_start: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
struct EventPublishTestHook {
    snapshot_ready: std::sync::mpsc::SyncSender<()>,
    release_write: std::sync::mpsc::Receiver<()>,
}

impl UnixIpcServer {
    /// Bind the canonical daemon endpoint at `socket_path`.
    ///
    /// Order matters (ADR-0012): take the instance lock FIRST so a live
    /// daemon's socket is never touched, then apply stale-path cleanup, then
    /// bind, tighten permissions and fsync the directory.
    ///
    /// Connection cleanup uses [`CONNECTION_SHUTDOWN_TIMEOUT`] as its total
    /// unless the composing service installs a shared
    /// [`UnixIpcServer::bind_with_shutdown_budget`].
    pub fn bind(socket_path: &Path) -> Result<Self, IpcError> {
        Self::bind_with_shutdown_budget(
            socket_path,
            ShutdownBudget::new(CONNECTION_SHUTDOWN_TIMEOUT),
        )
    }

    /// Bind the endpoint with one shared shutdown budget.
    ///
    /// The budget is armed by the first [`UnixIpcServer::request_shutdown`],
    /// including the signal-driven path, so the connection cleanup performed
    /// inside [`UnixIpcServer::serve`] is charged to the same total as every
    /// later daemon cleanup owner instead of receiving a fresh window.
    pub fn bind_with_shutdown_budget(
        socket_path: &Path,
        shutdown_budget: ShutdownBudget,
    ) -> Result<Self, IpcError> {
        let dir = socket_path
            .parent()
            .ok_or_else(|| IpcError::bad_request("socket path has no parent directory"))?;
        prepare_runtime_dir(dir)?;

        let instance_lock = acquire_instance_lock(dir)?;

        remove_stale_socket_if_safe(socket_path)?;

        let endpoint_lease = Arc::new(EndpointLease {
            _instance_lock: instance_lock,
            socket_path: socket_path.to_path_buf(),
            bound_socket: Mutex::new(None),
            listener_guard: Mutex::new(None),
        });

        let listener = UnixListener::bind(socket_path).map_err(|e| {
            IpcError::internal(format!("bind {} failed: {e}", socket_path.display()))
        })?;
        let bound_metadata = std::fs::symlink_metadata(socket_path)
            .map_err(|err| IpcError::internal(format!("lstat bound socket failed: {err}")))?;
        *lock_or_recover(&endpoint_lease.bound_socket) = Some(SocketIdentity {
            device: bound_metadata.dev(),
            inode: bound_metadata.ino(),
            owner_uid: bound_metadata.uid(),
        });
        *lock_or_recover(&endpoint_lease.listener_guard) = Some(
            listener
                .try_clone()
                .map_err(|err| IpcError::internal(format!("listener clone failed: {err}")))?,
        );
        // Non-blocking accept keeps the serve loop responsive to shutdown
        // without platform-specific accept timeouts.
        listener
            .set_nonblocking(true)
            .map_err(|e| IpcError::internal(format!("set_nonblocking failed: {e}")))?;

        // Post-bind hardening: force 0600 and verify owner + modes together.
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| IpcError::internal(format!("chmod socket failed: {e}")))?;
        let sock_meta = std::fs::metadata(socket_path)
            .map_err(|e| IpcError::internal(format!("stat socket failed: {e}")))?;
        let dir_meta = std::fs::metadata(dir)
            .map_err(|e| IpcError::internal(format!("stat dir failed: {e}")))?;
        EndpointPermissions {
            dir_mode: dir_meta.mode(),
            sock_mode: sock_meta.mode(),
            owner_uid: sock_meta.uid(),
        }
        .validate(current_uid())?;

        // Persist the directory entry so a crash cannot leave a half-bound
        // endpoint invisible after reboot (ADR-0012).
        let dir_handle =
            File::open(dir).map_err(|e| IpcError::internal(format!("open dir failed: {e}")))?;
        dir_handle
            .sync_all()
            .map_err(|e| IpcError::internal(format!("fsync dir failed: {e}")))?;

        Ok(Self {
            listener,
            endpoint_lease,
            expected_uid: current_uid(),
            stop: Arc::new(AtomicBool::new(false)),
            lifecycle: Mutex::new(true),
            event_seq: AtomicU64::new(0),
            next_client_id: Arc::new(AtomicU64::new(1)),
            active_connections: Arc::new(AtomicUsize::new(0)),
            clients: Arc::new(Mutex::new(HashMap::new())),
            workers: Arc::new(Mutex::new(HashMap::new())),
            shutdown_budget,
            #[cfg(test)]
            lifecycle_test_hook: Mutex::new(None),
            #[cfg(test)]
            event_publish_test_hook: Mutex::new(None),
        })
    }

    /// Socket path this server owns.
    pub fn socket_path(&self) -> &Path {
        &self.endpoint_lease.socket_path
    }

    /// Effective uid the endpoint is bound to.
    pub fn expected_uid(&self) -> u32 {
        self.expected_uid
    }

    /// This server's shared shutdown budget, armed by the first shutdown
    /// request. The composing service reads the same instance to bound every
    /// later cleanup owner by one monotonic deadline.
    #[must_use]
    pub fn shutdown_budget(&self) -> ShutdownBudget {
        self.shutdown_budget.clone()
    }

    /// Ask the serve loop to stop at the next accept poll.
    ///
    /// The first request establishes the shared shutdown epoch; later requests
    /// cannot move it.
    pub fn request_shutdown(&self) {
        self.shutdown_budget.arm();
        *lock_or_recover(&self.lifecycle) = false;
        self.stop.store(true, Ordering::SeqCst);
        self.interrupt_connections();
        // Stop new path-based connects immediately. The identity-checked
        // unlink does not release the endpoint flock or listener inode; every
        // admitted worker retains both through its EndpointLease clone.
        let _ = self.endpoint_lease.cleanup_owned_socket();
    }

    /// Close admission, interrupt every admitted socket and reap all owned
    /// connection threads within `timeout` and never past the shared shutdown
    /// deadline.
    ///
    /// A timeout leaves unfinished handles retained by this server. If the
    /// server is then dropped, those threads keep an [`EndpointLease`] clone,
    /// so the endpoint flock remains held until the final handler exits.
    pub fn shutdown_and_join(&self, timeout: Duration) -> Result<(), IpcError> {
        // The caller's bound includes admission closure, not only worker
        // reaping after request_shutdown returns. The shared budget caps it so
        // this owner cannot extend a cleanup window another owner already
        // spent.
        self.request_shutdown();
        let timeout = timeout.min(self.shutdown_budget.remaining());
        let deadline = Instant::now() + timeout;
        let cleanup_failure = self.endpoint_lease.cleanup_owned_socket().err();
        let mut first_failure = None;
        loop {
            if let Err(err) = self.reap_finished_workers() {
                first_failure.get_or_insert(err);
            }
            if lock_or_recover(&self.workers).is_empty()
                && self.active_connections.load(Ordering::Acquire) == 0
            {
                if let Some(err) = first_failure {
                    return Err(err);
                }
                return self.endpoint_lease.cleanup_owned_socket();
            }
            if Instant::now() >= deadline {
                if let Some(err) = cleanup_failure {
                    return Err(err);
                }
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
    /// until [`UnixIpcServer::request_shutdown`] or a fatal accept error.
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
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    if let Err(err) = self.dispatch(stream, handler.clone()) {
                        self.request_shutdown();
                        let _ = self.shutdown_and_join(CONNECTION_SHUTDOWN_TIMEOUT);
                        return Err(err);
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(ACCEPT_POLL_INTERVAL);
                }
                Err(e) => {
                    self.request_shutdown();
                    let _ = self.shutdown_and_join(CONNECTION_SHUTDOWN_TIMEOUT);
                    return Err(IpcError::internal(format!("accept failed: {e}")));
                }
            }
        }
        self.shutdown_and_join(CONNECTION_SHUTDOWN_TIMEOUT)
    }

    /// Handle at most one incoming connection synchronously. Returns
    /// `Ok(false)` when no peer arrived before the accept poll elapsed.
    /// Useful for tests and for embedding in external event loops. This path
    /// creates no detached worker: the caller's borrow owns the handler until
    /// the peer closes, while the connection remains visible to shutdown.
    pub fn accept_once<H>(&self, handler: &H) -> Result<bool, IpcError>
    where
        H: IpcHandler + ?Sized,
    {
        match self.listener.accept() {
            Ok((stream, _addr)) => {
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
                    self.expected_uid,
                    handler,
                );
                drop(admitted.guard);
                Ok(true)
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(IpcError::internal(format!("accept failed: {e}"))),
        }
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
        #[cfg(test)]
        self.pause_snapshotted_event_for_test();
        let mut delivered = 0;
        for entry in snapshot {
            let mut guard = match entry.writer.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
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
                    let _ = entry.control.shutdown(Shutdown::Both);
                }
            }
        }
        Ok(delivered)
    }

    fn dispatch<H>(&self, stream: UnixStream, handler: Arc<H>) -> Result<(), IpcError>
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
        let expected_uid = self.expected_uid;
        let endpoint_lease = self.endpoint_lease.clone();
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
                let _endpoint_lease = endpoint_lease;
                handle_connection(
                    admitted.reader,
                    admitted.writer,
                    admitted.entry,
                    expected_uid,
                    handler.as_ref(),
                );
                drop(admitted.guard);
            })
            .map_err(|err| IpcError::internal(format!("spawn connection worker failed: {err}")))?;
        lock_or_recover(&self.workers).insert(client_id, worker);
        #[cfg(test)]
        self.pause_registered_admission_for_test();
        // Admission remains serialized until the worker is registered. A
        // concurrent shutdown therefore cannot miss this worker or socket.
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
        stream: UnixStream,
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
        let writer_stream = match stream.try_clone() {
            Ok(writer) => writer,
            Err(err) => {
                self.active_connections.fetch_sub(1, Ordering::AcqRel);
                return Err(IpcError::internal(format!("stream clone failed: {err}")));
            }
        };
        let control = match writer_stream.try_clone() {
            Ok(control) => control,
            Err(err) => {
                self.active_connections.fetch_sub(1, Ordering::AcqRel);
                return Err(IpcError::internal(format!(
                    "control stream clone failed: {err}"
                )));
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
            let _ = entry.control.shutdown(Shutdown::Both);
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

    #[cfg(test)]
    fn pause_registered_admission_for_test(&self) {
        let hook = lock_or_recover(&self.lifecycle_test_hook).take();
        if let Some(hook) = hook {
            let _ = hook.admission_registered.send(());
            let _ = hook.release_start.recv();
        }
    }

    #[cfg(test)]
    fn pause_snapshotted_event_for_test(&self) {
        let hook = lock_or_recover(&self.event_publish_test_hook).take();
        if let Some(hook) = hook {
            let _ = hook.snapshot_ready.send(());
            let _ = hook.release_write.recv();
        }
    }
}

impl Drop for UnixIpcServer {
    fn drop(&mut self) {
        self.request_shutdown();
        let _ = self.reap_finished_workers();
        // Unfinished workers retain `endpoint_lease`, so even a typed timeout
        // cannot release the flock and admit an overlapping server generation.
        // Normal production ownership calls `serve`/`shutdown_and_join` and
        // receives the bounded typed outcome before this final fallback.
    }
}

/// Create (or tighten) the runtime directory to 0700 owned by the effective
/// user; reject anything we cannot bring into that state (ADR-0011).
fn prepare_runtime_dir(dir: &Path) -> Result<(), IpcError> {
    match std::fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(IpcError::permission_denied(format!(
                "runtime dir {} must not be a symlink",
                dir.display()
            )));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(IpcError::permission_denied(format!(
                "{} is not a directory",
                dir.display()
            )));
        }
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => {
            return Err(IpcError::internal(format!(
                "lstat runtime dir {} failed: {err}",
                dir.display()
            )));
        }
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| IpcError::internal(format!("create dir {} failed: {e}", dir.display())))?;
    let meta = std::fs::symlink_metadata(dir)
        .map_err(|e| IpcError::internal(format!("lstat dir failed: {e}")))?;
    if meta.file_type().is_symlink() {
        return Err(IpcError::permission_denied(format!(
            "runtime dir {} became a symlink",
            dir.display()
        )));
    }
    if !meta.is_dir() {
        return Err(IpcError::permission_denied(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    if meta.uid() != current_uid() {
        return Err(IpcError::permission_denied(format!(
            "runtime dir {} owned by uid {}, not effective uid {}",
            dir.display(),
            meta.uid(),
            current_uid()
        )));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| IpcError::internal(format!("chmod dir failed: {e}")))?;
    let tightened = std::fs::metadata(dir)
        .map_err(|e| IpcError::internal(format!("re-stat dir failed: {e}")))?;
    if tightened.mode() & 0o777 != 0o700 {
        return Err(IpcError::permission_denied(format!(
            "runtime dir {} mode {:o} could not be tightened to 700",
            dir.display(),
            tightened.mode() & 0o777
        )));
    }
    Ok(())
}

/// Open `<dir>/narrivyd.lock` (0600) and take a non-blocking exclusive
/// `flock`. A held lock means another daemon instance owns this endpoint.
fn acquire_instance_lock(dir: &Path) -> Result<File, IpcError> {
    let lock_path = dir.join(LOCK_FILE_NAME);
    validate_existing_lock_path(&lock_path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&lock_path)
        .map_err(|e| {
            IpcError::internal(format!("open lock {} failed: {e}", lock_path.display()))
        })?;
    tighten_and_validate_lock(&file, &lock_path)?;
    try_flock_exclusive(&file, &lock_path)?;
    Ok(file)
}

fn validate_existing_lock_path(lock_path: &Path) -> Result<(), IpcError> {
    let metadata = match std::fs::symlink_metadata(lock_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(IpcError::internal(format!(
                "lstat lock {} failed: {err}",
                lock_path.display()
            )));
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != current_uid()
        || metadata.nlink() != 1
    {
        return Err(IpcError::permission_denied(format!(
            "lock {} must be an owned non-symlink regular file with one link",
            lock_path.display()
        )));
    }
    Ok(())
}

fn tighten_and_validate_lock(file: &File, lock_path: &Path) -> Result<(), IpcError> {
    let opened = file
        .metadata()
        .map_err(|err| IpcError::internal(format!("fstat lock failed: {err}")))?;
    if !opened.file_type().is_file() || opened.uid() != current_uid() || opened.nlink() != 1 {
        return Err(IpcError::permission_denied(format!(
            "opened lock {} is not an owned single-link regular file",
            lock_path.display()
        )));
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|err| IpcError::internal(format!("chmod lock failed: {err}")))?;
    let tightened = file
        .metadata()
        .map_err(|err| IpcError::internal(format!("re-fstat lock failed: {err}")))?;
    let path_metadata = std::fs::symlink_metadata(lock_path)
        .map_err(|err| IpcError::internal(format!("re-lstat lock failed: {err}")))?;
    if tightened.mode() & 0o777 != 0o600
        || path_metadata.file_type().is_symlink()
        || path_metadata.dev() != tightened.dev()
        || path_metadata.ino() != tightened.ino()
        || path_metadata.uid() != current_uid()
        || path_metadata.nlink() != 1
    {
        return Err(IpcError::permission_denied(format!(
            "lock {} identity or 0600 mode verification failed",
            lock_path.display()
        )));
    }
    Ok(())
}

/// Non-blocking `LOCK_EX` flock; lock contention maps to DaemonUnavailable.
///
/// Only reached from unix targets (the whole module is `#[cfg(unix)]`);
/// `flock(2)` exists on Linux and the BSDs/macOS alike.
fn try_flock_exclusive(file: &File, lock_path: &Path) -> Result<(), IpcError> {
    // SAFETY: `file.as_raw_fd()` is a live open file descriptor owned by
    // `file`; flock only manipulates kernel lock state for that descriptor
    // and takes no pointers. A non-zero return leaves errno readable via
    // last_os_error; no memory owned by Rust is touched.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.kind() == ErrorKind::WouldBlock {
        return Err(IpcError::new(
            IpcErrorCode::DaemonUnavailable,
            format!("another narrivyd instance holds {}", lock_path.display()),
        ));
    }
    Err(IpcError::internal(format!(
        "flock {} failed: {err}",
        lock_path.display()
    )))
}

/// Inspect the socket path via lstat-only metadata and remove it iff it is
/// an exactly-stale socket owned by the effective user. Anything else that
/// EXISTS is refused: symlinks are never followed and foreign paths are
/// never deleted (ADR-0012 "Socket lifecycle").
fn remove_stale_socket_if_safe(socket_path: &Path) -> Result<(), IpcError> {
    let meta = match std::fs::symlink_metadata(socket_path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(IpcError::internal(format!(
                "lstat {} failed: {e}",
                socket_path.display()
            )));
        }
    };
    let validation = SocketPathValidation {
        expected_path: socket_path.to_path_buf(),
        actual_path: socket_path.to_path_buf(),
        is_symlink: meta.file_type().is_symlink(),
        is_socket: meta.file_type().is_socket(),
        owner_uid: meta.uid(),
        expected_uid: current_uid(),
    };
    if validation.should_remove_stale() {
        std::fs::remove_file(socket_path)
            .map_err(|e| IpcError::internal(format!("remove stale socket failed: {e}")))?;
        return Ok(());
    }
    Err(IpcError::permission_denied(format!(
        "refusing to replace {}: not a removable stale socket owned by uid {} \
         (symlink={}, socket={}, owner_uid={})",
        socket_path.display(),
        current_uid(),
        validation.is_symlink,
        validation.is_socket,
        validation.owner_uid
    )))
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
    stream: &mut UnixStream,
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
    let _ = write_frame(stream, &bytes);
}

fn write_shared(writer: &SharedWriter, bytes: &[u8]) -> Result<(), IpcError> {
    write_frame(&mut *lock_or_recover(writer), bytes)
}

/// Reads one frame from the stream. Counts the bytes it consumed, so a
/// quiet timeout can be told apart from one that stopped mid-frame, and
/// bounds the whole frame by [`CONNECTION_IDLE_TIMEOUT`] from its first
/// byte: the socket timeout alone restarts on every byte, so a peer sending
/// one byte at a time could otherwise hold a connection and its thread for
/// as long as it liked.
struct CountingRead<'a> {
    inner: &'a mut UnixStream,
    received: usize,
    started: Option<Instant>,
}

impl<'a> CountingRead<'a> {
    fn new(inner: &'a mut UnixStream) -> Self {
        Self {
            inner,
            received: 0,
            started: None,
        }
    }
}

impl Read for CountingRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let timeout = match self.started {
            None => CONNECTION_IDLE_TIMEOUT,
            Some(started) => CONNECTION_IDLE_TIMEOUT
                .checked_sub(started.elapsed())
                .filter(|left| !left.is_zero())
                .ok_or(ErrorKind::TimedOut)?,
        };
        self.inner.set_read_timeout(Some(timeout))?;
        let read = self.inner.read(buf)?;
        if read > 0 && self.started.is_none() {
            self.started = Some(Instant::now());
        }
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
    mut stream: UnixStream,
    writer: SharedWriter,
    entry: Arc<ConnectionEntry>,
    expected_uid: u32,
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
    if let Err(err) = credentials.validate(expected_uid) {
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
    let hello_bytes = match read_frame(&mut CountingRead::new(&mut stream)) {
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
        let mut reader = CountingRead::new(&mut stream);
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
    use narrivy_ipc::protocol::{
        ActiveSessionDto, PROTOCOL_MAJOR, PROTOCOL_MINOR, PipelineSummaryDto, StatusDto,
    };
    use narrivy_ipc::{ActivationSourceDto, BUILD_IDENTITY, Command};
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
    fn publish_until_accepted(server: &UnixIpcServer, event: &Event) -> TestResult {
        for _ in 0..300 {
            let delivered = server.publish_event(event)?;
            if delivered > 0 {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err("event was never accepted by a client".into())
    }

    #[test]
    fn a_quiet_subscriber_stays_connected_and_a_quiet_requester_does_not() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sock_path = dir.path().join("narrivyd.sock");
        let server = Arc::new(UnixIpcServer::bind(&sock_path)?);
        let srv = server.clone();
        let serve_thread = std::thread::spawn(move || {
            srv.serve(|_credentials: PeerCredentials, envelope: Envelope| {
                reply_envelope(envelope.request_id, &ResponsePayload::Accepted)
            })
        });
        let mut subscriber = IpcClient::connect(&sock_path)?;
        assert_eq!(
            subscriber.request(&Command::Subscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
        let mut requester = IpcClient::connect(&sock_path)?;
        assert_eq!(
            requester.request(&Command::GetStatus, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );

        // Several idle periods pass with nothing sent by either client.
        std::thread::sleep(CONNECTION_IDLE_TIMEOUT * 4);
        let event = Event::SessionStateChanged {
            session_id: 7,
            transaction_id: Some("tx-quiet".into()),
            pipeline_id: Some("clean".into()),
            state: "committed".into(),
            seq: 9,
            delivery: Some("pasted".into()),
        };
        assert_eq!(server.publish_event(&event)?, 1);
        assert_eq!(subscriber.next_event(TEST_TIMEOUT)?, event);
        assert!(
            requester
                .request(&Command::GetStatus, TEST_TIMEOUT)
                .is_err()
        );

        server.request_shutdown();
        serve_thread.join().map_err(|_| "serve thread panicked")??;
        Ok(())
    }

    #[test]
    fn ipc_end_to_end_status_and_event_over_unix_socket() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sock_path = dir.path().join("narrivyd.sock");
        let server = Arc::new(UnixIpcServer::bind(&sock_path)?);
        assert_eq!(server.expected_uid(), current_uid());

        let srv = server.clone();
        let serve_thread = std::thread::spawn(move || {
            srv.serve(|credentials: PeerCredentials, envelope: Envelope| {
                // Same-user policy is already enforced before this runs.
                if !credentials.is_same_user(current_uid()) {
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

        let mut client = IpcClient::connect(&sock_path)?;
        assert_eq!(client.daemon_build(), BUILD_IDENTITY);
        assert_eq!(client.negotiated_minor(), PROTOCOL_MINOR);

        // 1. Request GetStatus; verify the typed StatusDto round-trip with
        //    first-class pipeline/transaction identity.
        let status = client.request(&Command::GetStatus, TEST_TIMEOUT)?;
        let ResponsePayload::Status(dto) = status else {
            return Err("expected Status payload".into());
        };
        assert_eq!(dto.daemon_build, "narrivyd-test-0.0.0");
        assert_eq!(dto.protocol_minor, PROTOCOL_MINOR);
        let active = match dto.active_session {
            Some(active) => active,
            None => return Err("active session must be set".into()),
        };
        assert_eq!(active.session_id, 42u128);
        assert_eq!(active.transaction_id.as_deref(), Some("tx-abc"));
        assert_eq!(active.pipeline_id.as_deref(), Some("default"));
        assert_eq!(active.state, "listening");

        assert_eq!(
            client.request(&Command::Subscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );

        // 2. Daemon emits one SessionStateChanged event; client receives it
        //    with daemon-assigned monotonic sequence and full identity.
        let event = Event::SessionStateChanged {
            session_id: 42,
            transaction_id: Some("tx-abc".into()),
            pipeline_id: Some("default".into()),
            state: "listening".into(),
            seq: 1,
            delivery: None,
        };
        publish_until_accepted(&server, &event)?;
        let received = client.next_event(TEST_TIMEOUT)?;
        match received {
            Event::SessionStateChanged {
                session_id,
                transaction_id,
                pipeline_id,
                state,
                seq,
                delivery,
            } => {
                assert_eq!(delivery, None);
                assert_eq!(session_id, 42u128);
                assert_eq!(transaction_id.as_deref(), Some("tx-abc"));
                assert_eq!(pipeline_id.as_deref(), Some("default"));
                assert_eq!(state, "listening");
                assert_eq!(seq, 1);
            }
            other => return Err(format!("unexpected event {other:?}").into()),
        }

        // 3. Pipelines listing (v1.3 first-class Pipelines).
        let pipelines = client.request(&Command::ListPipelines, TEST_TIMEOUT)?;
        let ResponsePayload::Pipelines(list) = pipelines else {
            return Err("expected Pipelines payload".into());
        };
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "default");
        assert!(list[0].enabled);

        // 4. Explicit unsubscribe removes event eligibility.
        assert_eq!(
            client.request(&Command::Unsubscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
        assert_eq!(server.publish_event(&event)?, 0);

        // 5. Clean disconnect + graceful shutdown.
        drop(client);
        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        Ok(())
    }

    #[test]
    fn ipc_unsubscribe_ack_blocks_an_event_from_an_older_snapshot() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sock_path = dir.path().join("unsubscribe-race.sock");
        let server = Arc::new(UnixIpcServer::bind(&sock_path)?);
        let srv = server.clone();
        let serve_thread = std::thread::spawn(move || {
            srv.serve(|_credentials: PeerCredentials, envelope: Envelope| {
                reply_envelope(envelope.request_id, &ResponsePayload::Accepted)
            })
        });

        let mut client = IpcClient::connect(&sock_path)?;
        assert_eq!(
            client.request(&Command::Subscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );

        let (snapshot_tx, snapshot_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        *lock_or_recover(&server.event_publish_test_hook) = Some(EventPublishTestHook {
            snapshot_ready: snapshot_tx,
            release_write: release_rx,
        });
        let event = Event::HealthChanged {
            module_id: "controlled".into(),
            healthy: true,
        };
        let publish_server = server.clone();
        let publisher = std::thread::spawn(move || publish_server.publish_event(&event));
        snapshot_rx.recv_timeout(TEST_TIMEOUT)?;

        assert_eq!(
            client.request(&Command::Unsubscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
        release_tx.send(())?;
        let delivered = publisher.join().map_err(|_| "event publisher panicked")??;
        assert_eq!(delivered, 0);
        let event_error = match client.next_event(Duration::from_millis(100)) {
            Err(error) => error,
            Ok(event) => return Err(format!("event escaped after unsubscribe: {event:?}").into()),
        };
        assert_eq!(event_error.code, IpcErrorCode::Timeout);

        drop(client);
        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        Ok(())
    }

    #[test]
    fn ipc_request_ids_correlate_across_interleaved_events() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sock_path = dir.path().join("corr.sock");
        let server = Arc::new(UnixIpcServer::bind(&sock_path)?);
        let srv = server.clone();

        // Handler encodes which request id it answered into the payload so
        // correlation is observable end to end; slow enough that events can
        // interleave with each in-flight request.
        let serve_thread = std::thread::spawn(move || {
            srv.serve(|_credentials: PeerCredentials, envelope: Envelope| {
                if envelope.kind == MessageKind::Subscribe {
                    return reply_envelope(envelope.request_id, &ResponsePayload::Accepted);
                }
                std::thread::sleep(Duration::from_millis(120));
                let request_id = envelope.request_id;
                reply_envelope(
                    request_id,
                    &ResponsePayload::Status(StatusDto {
                        daemon_build: format!("reply-for-{request_id}"),
                        protocol_minor: PROTOCOL_MINOR,
                        active_session: None,
                        last_session_timings_us: None,
                        capabilities: vec![],
                        installed_model: None,
                        model_provisioning: None,
                        portal_keyboard: None,
                    }),
                )
            })
        });

        let mut client = IpcClient::connect(&sock_path)?;
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

        // Both requests correlate strictly by id even though two events are
        // pushed while the requests are in flight.
        let first = client.request(&Command::GetStatus, TEST_TIMEOUT)?;
        let second = client.request(&Command::GetStatus, TEST_TIMEOUT)?;
        publisher.join().map_err(|_| "publisher panicked")??;
        let ResponsePayload::Status(first_dto) = first else {
            return Err("expected Status".into());
        };
        let ResponsePayload::Status(second_dto) = second else {
            return Err("expected Status".into());
        };
        assert_eq!(first_dto.daemon_build, "reply-for-2");
        assert_eq!(second_dto.daemon_build, "reply-for-3");

        // Queued events arrive in order with their own sequence numbers.
        for seq in [1u64, 2] {
            let event = client.next_event(TEST_TIMEOUT)?;
            let Event::HealthChanged { module_id, .. } = event else {
                return Err(format!("unexpected event {event:?}").into());
            };
            assert_eq!(module_id, format!("module-{seq}"));
        }
        // Queue drained: the next wait must time out, not hang forever.
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
    fn ipc_server_second_instance_lock_rejected() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sock_path = dir.path().join("narrivyd.sock");
        let lock_path = dir.path().join(LOCK_FILE_NAME);
        let first = UnixIpcServer::bind(&sock_path)?;
        assert!(lock_path.exists(), "lock file must exist while running");

        let second = match UnixIpcServer::bind(&sock_path) {
            Err(second) => second,
            Ok(_) => return Err("second bind must fail while first instance lives".into()),
        };
        assert_eq!(second.code, IpcErrorCode::DaemonUnavailable);
        assert!(second.to_string().contains("another narrivyd instance"));

        drop(first); // releases flock
        UnixIpcServer::bind(&sock_path)?; // immediate re-bind succeeds
        Ok(())
    }

    #[test]
    fn ipc_server_stale_socket_rebound_after_drop() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sock_path = dir.path().join("narrivyd.sock");

        // Simulate a crashed daemon: a raw listener whose socket FILE stays
        // behind after close (std never unlinks on drop).
        {
            let listener = UnixListener::bind(&sock_path)?;
            assert!(listener.local_addr()?.as_pathname().is_some());
        }
        let meta = std::fs::symlink_metadata(&sock_path)?;
        assert!(
            meta.file_type().is_socket(),
            "stale socket file must remain"
        );

        // Bind must classify the leftover as exactly-stale and take over.
        let server = Arc::new(UnixIpcServer::bind(&sock_path)?);
        let s = server.clone();
        let serve_thread = std::thread::spawn(move || {
            s.serve(|_c: PeerCredentials, e: Envelope| {
                reply_envelope(e.request_id, &ResponsePayload::Accepted)
            })
        });
        let mut client = IpcClient::connect(&sock_path)?;
        let response = client.request(&Command::Unsubscribe, TEST_TIMEOUT)?;
        assert_eq!(response, ResponsePayload::Accepted);
        drop(client);
        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        Ok(())
    }

    #[test]
    fn ipc_server_refuses_non_stale_socket_paths() -> TestResult {
        let dir = tempfile::tempdir()?;

        // Regular file squatting on the socket path: refuse, never delete.
        let regular = dir.path().join("regular.sock");
        std::fs::write(&regular, b"not a socket")?;
        let err = match UnixIpcServer::bind(&regular) {
            Err(err) => err,
            Ok(_) => return Err("must refuse regular file on socket path".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        assert!(
            std::fs::symlink_metadata(&regular)?.is_file(),
            "file untouched"
        );

        // Symlink on the path (even dangling): refuse without following.
        let symlinked = dir.path().join("linked.sock");
        std::os::unix::fs::symlink("/nonexistent/target.sock", &symlinked)?;
        let err = match UnixIpcServer::bind(&symlinked) {
            Err(err) => err,
            Ok(_) => return Err("must refuse symlink on socket path".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        assert!(
            std::fs::symlink_metadata(&symlinked)?
                .file_type()
                .is_symlink(),
            "symlink untouched"
        );
        Ok(())
    }

    #[test]
    fn ipc_endpoint_lock_tightens_mode_and_rejects_symlink_or_non_regular() -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;

        let broad_dir = tempfile::tempdir()?;
        let broad_lock = broad_dir.path().join(LOCK_FILE_NAME);
        std::fs::write(&broad_lock, b"")?;
        std::fs::set_permissions(&broad_lock, std::fs::Permissions::from_mode(0o666))?;
        let server = UnixIpcServer::bind(&broad_dir.path().join("broad.sock"))?;
        assert_eq!(
            std::fs::metadata(&broad_lock)?.permissions().mode() & 0o777,
            0o600
        );
        drop(server);

        let symlink_dir = tempfile::tempdir()?;
        let target = symlink_dir.path().join("target");
        std::fs::write(&target, b"preserve")?;
        std::os::unix::fs::symlink(&target, symlink_dir.path().join(LOCK_FILE_NAME))?;
        let err = match UnixIpcServer::bind(&symlink_dir.path().join("symlink.sock")) {
            Err(err) => err,
            Ok(_) => return Err("symlink lock must be rejected".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        assert_eq!(std::fs::read(&target)?, b"preserve");

        let directory_dir = tempfile::tempdir()?;
        std::fs::create_dir(directory_dir.path().join(LOCK_FILE_NAME))?;
        let err = match UnixIpcServer::bind(&directory_dir.path().join("directory.sock")) {
            Err(err) => err,
            Ok(_) => return Err("non-regular lock must be rejected".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);

        let hardlink_dir = tempfile::tempdir()?;
        let hardlink_target = hardlink_dir.path().join("lock-target");
        let hardlink_path = hardlink_dir.path().join(LOCK_FILE_NAME);
        std::fs::write(&hardlink_target, b"preserve")?;
        std::fs::hard_link(&hardlink_target, &hardlink_path)?;
        let err = match UnixIpcServer::bind(&hardlink_dir.path().join("hardlink.sock")) {
            Err(err) => err,
            Ok(_) => return Err("multi-link lock must be rejected".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        assert_eq!(std::fs::read(&hardlink_target)?, b"preserve");
        assert_eq!(std::fs::read(&hardlink_path)?, b"preserve");
        Ok(())
    }

    #[test]
    fn ipc_runtime_directory_symlink_is_rejected_without_touching_target() -> TestResult {
        let root = tempfile::tempdir()?;
        let real = root.path().join("real-runtime");
        std::fs::create_dir(&real)?;
        let linked = root.path().join("linked-runtime");
        std::os::unix::fs::symlink(&real, &linked)?;

        let err = match UnixIpcServer::bind(&linked.join("narrivyd.sock")) {
            Err(err) => err,
            Ok(_) => return Err("symlink runtime directory must be rejected".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        assert!(std::fs::read_dir(&real)?.next().is_none());
        Ok(())
    }

    #[test]
    fn ipc_cleanup_preserves_replacement_socket_identity() -> TestResult {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("replacement.sock");
        let server = UnixIpcServer::bind(&socket)?;
        let original = std::fs::symlink_metadata(&socket)?;
        std::fs::remove_file(&socket)?;
        let replacement = UnixListener::bind(&socket)?;
        let replacement_metadata = std::fs::symlink_metadata(&socket)?;
        assert_ne!(original.ino(), replacement_metadata.ino());

        let err = match server.shutdown_and_join(TEST_TIMEOUT) {
            Err(err) => err,
            Ok(()) => return Err("replaced endpoint cleanup must fail closed".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        drop(server);
        assert_eq!(
            std::fs::symlink_metadata(&socket)?.ino(),
            replacement_metadata.ino(),
            "server cleanup must not unlink the replacement"
        );
        drop(replacement);
        Ok(())
    }

    #[test]
    fn ipc_server_accept_once_handles_single_connection() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sock_path = dir.path().join("once.sock");
        let server = UnixIpcServer::bind(&sock_path)?;

        // No peer yet: one poll returns false instead of blocking forever.
        let handled = server.accept_once(&_no_op_handler())?;
        assert!(!handled);

        let connector = std::thread::spawn(move || -> Result<ResponsePayload, IpcError> {
            let mut client = IpcClient::connect(&sock_path)?;
            client.request(&Command::GetStatus, TEST_TIMEOUT)
        });
        let deadline = std::time::Instant::now() + TEST_TIMEOUT;
        loop {
            let handled = server.accept_once(&_echo_status_handler())?;
            if handled {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err("client never arrived before the test deadline".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let response = connector.join().map_err(|_| "connector panicked")??;
        let ResponsePayload::Status(status) = response else {
            return Err("accept_once handler returned an unexpected response".into());
        };
        assert_eq!(status.daemon_build, "accept-once");
        Ok(())
    }

    #[test]
    fn ipc_events_are_ineligible_until_handshake_is_accepted() -> TestResult {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("pre-handshake.sock");
        let server = Arc::new(UnixIpcServer::bind(&socket)?);
        let serving = server.clone();
        let serve_thread = std::thread::spawn(move || {
            serving.serve(|_peer: PeerCredentials, _envelope: Envelope| None)
        });

        let mut raw = UnixStream::connect(&socket)?;
        let deadline = Instant::now() + TEST_TIMEOUT;
        while server.active_connection_count() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.active_connection_count(), 1);
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
    fn ipc_minor_two_subscriber_receives_lifecycle_but_not_partial_variant() -> TestResult {
        let dir = tempfile::tempdir()?;
        let server = UnixIpcServer::bind(&dir.path().join("minor-filter.sock"))?;
        let (writer_stream, mut reader_stream) = UnixStream::pair()?;
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
        Ok(())
    }

    #[test]
    fn ipc_concurrent_event_and_response_frames_remain_intact() -> TestResult {
        const ROUNDS: usize = 32;
        const EVENT_PAD_BYTES: usize = 64 * 1024;
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("serialized-writes.sock");
        let server = Arc::new(UnixIpcServer::bind(&socket)?);
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

        let mut client = IpcClient::connect_as(&socket, "write-race-test")?;
        assert_eq!(
            client.request(&Command::Subscribe, TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
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
        assert_eq!(publisher.join().map_err(|_| "publisher panicked")??, ROUNDS);
        for round in 0..ROUNDS {
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
    fn ipc_malformed_handshake_has_terminal_registry_and_worker_cleanup() -> TestResult {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("malformed-handshake.sock");
        let server = Arc::new(UnixIpcServer::bind(&socket)?);
        let serving = server.clone();
        let serve_thread = std::thread::spawn(move || {
            serving.serve(|_peer: PeerCredentials, _envelope: Envelope| None)
        });

        let mut raw = UnixStream::connect(&socket)?;
        write_frame(&mut raw, b"not-json")?;
        let rejection = read_frame(&mut raw)?;
        assert!(matches!(
            serde_json::from_slice::<HandshakeResponse>(&rejection)?,
            HandshakeResponse::Rejected {
                code: IpcErrorCode::BadRequest,
                ..
            }
        ));
        drop(raw);

        let deadline = Instant::now() + TEST_TIMEOUT;
        while server.active_connection_count() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.active_connection_count(), 0);
        assert_eq!(server.registered_connection_count(), 0);
        let reap_deadline = Instant::now() + TEST_TIMEOUT;
        while server.owned_worker_count() != 0 && Instant::now() < reap_deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.owned_worker_count(), 0);

        server.request_shutdown();
        join_serve_thread(serve_thread)?;
        Ok(())
    }

    #[test]
    fn ipc_abandoned_admission_cleans_registry_before_worker_start() -> TestResult {
        let dir = tempfile::tempdir()?;
        let server = UnixIpcServer::bind(&dir.path().join("spawn-boundary.sock"))?;
        let (accepted, peer) = UnixStream::pair()?;
        let admission = lock_or_recover(&server.lifecycle);
        let admitted = match server.admit_open_connection(accepted)? {
            Some(admitted) => admitted,
            None => return Err("open server unexpectedly rejected admission".into()),
        };
        assert_eq!(server.active_connection_count(), 1);
        assert_eq!(server.registered_connection_count(), 1);

        // This is the exact ownership state dropped when Builder::spawn
        // fails before a worker exists: streams and the RAII guard are still
        // held by the unspawned closure/admission value.
        drop(admitted);
        drop(admission);
        drop(peer);
        assert_eq!(server.active_connection_count(), 0);
        assert_eq!(server.registered_connection_count(), 0);
        assert_eq!(server.owned_worker_count(), 0);
        Ok(())
    }

    #[test]
    fn ipc_admission_and_shutdown_linearize_before_worker_start() -> TestResult {
        let dir = tempfile::tempdir()?;
        let server = Arc::new(UnixIpcServer::bind(
            &dir.path().join("admission-shutdown.sock"),
        )?);
        let (admission_registered_tx, admission_registered_rx) = std::sync::mpsc::sync_channel(0);
        let (release_start_tx, release_start_rx) = std::sync::mpsc::sync_channel(0);
        *lock_or_recover(&server.lifecycle_test_hook) = Some(LifecycleTestHook {
            admission_registered: admission_registered_tx,
            release_start: release_start_rx,
        });

        let (accepted, peer) = UnixStream::pair()?;
        let dispatch_server = server.clone();
        let dispatch = std::thread::spawn(move || {
            dispatch_server.dispatch(accepted, Arc::new(_no_op_handler()))
        });
        admission_registered_rx.recv_timeout(TEST_TIMEOUT)?;
        assert_eq!(server.active_connection_count(), 1);
        assert_eq!(server.registered_connection_count(), 1);
        assert_eq!(server.owned_worker_count(), 1);
        assert!(matches!(
            server.lifecycle.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));

        let shutdown_server = server.clone();
        let (shutdown_done_tx, shutdown_done_rx) = std::sync::mpsc::sync_channel(1);
        let shutdown = std::thread::spawn(move || {
            let started = Instant::now();
            let result = shutdown_server.shutdown_and_join(TEST_TIMEOUT);
            let _ = shutdown_done_tx.send((started.elapsed(), result));
        });
        assert!(
            shutdown_done_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "shutdown must contend with the serialized in-flight admission"
        );

        release_start_tx.send(())?;
        dispatch.join().map_err(|_| "dispatch panicked")??;
        let (elapsed, shutdown_result) = shutdown_done_rx.recv_timeout(TEST_TIMEOUT)?;
        shutdown_result?;
        shutdown.join().map_err(|_| "shutdown panicked")?;
        assert!(elapsed <= TEST_TIMEOUT);
        assert_eq!(server.active_connection_count(), 0);
        assert_eq!(server.registered_connection_count(), 0);
        assert_eq!(server.owned_worker_count(), 0);

        // Admission stays closed after the linearized shutdown. The rejected
        // connection never enters either registry or starts a handler.
        let (late, mut late_peer) = UnixStream::pair()?;
        server.dispatch(late, Arc::new(_no_op_handler()))?;
        let rejection = Envelope::from_json_bytes(&read_frame(&mut late_peer)?)?;
        assert!(matches!(
            rejection.parse_payload::<ResponsePayload>()?,
            ResponsePayload::Rejected {
                code: IpcErrorCode::DaemonUnavailable,
                ..
            }
        ));
        assert_eq!(server.active_connection_count(), 0);
        assert_eq!(server.registered_connection_count(), 0);
        assert_eq!(server.owned_worker_count(), 0);
        drop(late_peer);
        drop(peer);
        Ok(())
    }

    #[test]
    fn ipc_shutdown_admission_rejects_a_late_atomic_toggle_before_the_handler() -> TestResult {
        // Shutdown admission dominates every request kind, including the
        // additive atomic toggle: once admission closes, a ToggleSession is
        // answered with a typed rejection and never reaches the handler, so it
        // cannot start capture during shutdown.
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("toggle-shutdown.sock");
        let server = Arc::new(UnixIpcServer::bind(&socket)?);
        let seen = Arc::new(Mutex::new(Vec::<Command>::new()));
        let handler_seen = Arc::clone(&seen);
        let handler = Arc::new(move |_peer: PeerCredentials, envelope: Envelope| {
            let command: Command = envelope.parse_payload().ok()?;
            lock_or_recover(&handler_seen).push(command);
            reply_envelope(envelope.request_id, &ResponsePayload::Accepted)
        });
        let serve_server = Arc::clone(&server);
        let serve_handler = Arc::clone(&handler);
        let serve = std::thread::spawn(move || serve_server.serve(move |p, e| serve_handler(p, e)));

        let toggle = || Command::ToggleSession {
            pipeline_id: Some("clean".to_owned()),
            activation: ActivationSourceDto::InstalledCliCommand,
            recognition: None,
        };

        // Admitted toggle: the handler really serves this command.
        let mut client = IpcClient::connect_as(&socket, "toggle-before-shutdown")?;
        assert_eq!(
            client.request(&toggle(), TEST_TIMEOUT)?,
            ResponsePayload::Accepted
        );
        assert_eq!(lock_or_recover(&seen).len(), 1);
        drop(client);

        server.shutdown_and_join(TEST_TIMEOUT)?;

        // Late toggle: refused before any handler invocation.
        let (late, mut late_peer) = UnixStream::pair()?;
        server.dispatch(late, handler)?;
        let rejection = Envelope::from_json_bytes(&read_frame(&mut late_peer)?)?;
        assert!(matches!(
            rejection.parse_payload::<ResponsePayload>()?,
            ResponsePayload::Rejected {
                code: IpcErrorCode::DaemonUnavailable,
                ..
            }
        ));
        let recorded = lock_or_recover(&seen);
        assert_eq!(
            recorded.len(),
            1,
            "the rejected toggle must not reach the handler: {recorded:?}"
        );
        assert!(matches!(recorded[0], Command::ToggleSession { .. }));
        drop(recorded);
        drop(late_peer);
        assert_eq!(server.active_connection_count(), 0);
        assert_eq!(server.registered_connection_count(), 0);
        assert_eq!(server.owned_worker_count(), 0);
        serve.join().map_err(|_| "serve thread panicked")??;
        Ok(())
    }

    #[test]
    fn ipc_shutdown_timeout_retains_endpoint_lease_until_handler_exits() -> TestResult {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("lease-retention.sock");
        let server = UnixIpcServer::bind(&socket)?;
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let handler_release = release_rx.clone();
        let handler = Arc::new(move |_peer: PeerCredentials, envelope: Envelope| {
            let _ = entered_tx.send(());
            let _ = lock_or_recover(&handler_release).recv();
            reply_envelope(envelope.request_id, &ResponsePayload::Accepted)
        });

        let client_socket = socket.clone();
        let client = std::thread::spawn(move || {
            let mut client = IpcClient::connect_as(&client_socket, "lease-test")?;
            client.request(&Command::GetStatus, TEST_TIMEOUT)
        });
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            match server.listener.accept() {
                Ok((stream, _)) => {
                    server.dispatch(stream, handler.clone())?;
                    break;
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => return Err(err.into()),
            }
        }
        entered_rx.recv_timeout(TEST_TIMEOUT)?;

        let err = match server.shutdown_and_join(Duration::from_millis(50)) {
            Err(err) => err,
            Ok(()) => return Err("blocked handler must produce a bounded shutdown timeout".into()),
        };
        assert_eq!(err.code, IpcErrorCode::Timeout);
        assert!(
            !socket.exists(),
            "closed admission must unlink the owned endpoint path"
        );
        let connect_err = match IpcClient::connect_as(&socket, "late-client") {
            Err(err) => err,
            Ok(_) => return Err("late client connected to a non-accepting endpoint".into()),
        };
        assert_eq!(connect_err.code, IpcErrorCode::DaemonUnavailable);
        let still_owned = match UnixIpcServer::bind(&socket) {
            Err(err) => err,
            Ok(_) => return Err("successor acquired the endpoint during timed-out shutdown".into()),
        };
        assert_eq!(still_owned.code, IpcErrorCode::DaemonUnavailable);
        drop(server);

        let overlapping = match UnixIpcServer::bind(&socket) {
            Err(err) => err,
            Ok(_) => return Err("successor overlapped a timed-out connection handler".into()),
        };
        assert_eq!(overlapping.code, IpcErrorCode::DaemonUnavailable);

        release_tx.send(())?;
        let _ = client.join().map_err(|_| "client panicked")?;
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            match UnixIpcServer::bind(&socket) {
                Ok(successor) => {
                    drop(successor);
                    break;
                }
                Err(err)
                    if err.code == IpcErrorCode::DaemonUnavailable && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// One shared budget must bound connection cleanup too: a blocked handler
    /// cannot make the server spend its own fresh window past the deadline the
    /// rest of the service is already using.
    #[test]
    fn shared_shutdown_budget_bounds_connection_cleanup() -> TestResult {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("shared-budget.sock");
        let budget = ShutdownBudget::new(Duration::from_millis(300));
        let server = UnixIpcServer::bind_with_shutdown_budget(&socket, budget.clone())?;
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let handler_release = release_rx.clone();
        let handler = Arc::new(move |_peer: PeerCredentials, envelope: Envelope| {
            let _ = entered_tx.send(());
            let _ = lock_or_recover(&handler_release).recv();
            reply_envelope(envelope.request_id, &ResponsePayload::Accepted)
        });

        let client_socket = socket.clone();
        let client = std::thread::spawn(move || {
            let mut client = IpcClient::connect_as(&client_socket, "budget-test")?;
            client.request(&Command::GetStatus, TEST_TIMEOUT)
        });
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            match server.listener.accept() {
                Ok((stream, _)) => {
                    server.dispatch(stream, handler.clone())?;
                    break;
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => return Err(err.into()),
            }
        }
        entered_rx.recv_timeout(TEST_TIMEOUT)?;

        let started = Instant::now();
        let err = match server.shutdown_and_join(TEST_TIMEOUT) {
            Err(err) => err,
            Ok(()) => {
                return Err("a blocked handler cannot be reported as cleanly reaped".into());
            }
        };
        let elapsed = started.elapsed();
        assert_eq!(err.code, IpcErrorCode::Timeout);
        assert!(
            elapsed <= Duration::from_millis(700),
            "connection cleanup spent {elapsed:?} past the shared budget"
        );
        assert!(
            budget.expired(),
            "the shared budget was not consumed by connection cleanup"
        );

        release_tx.send(())?;
        let _ = client.join().map_err(|_| "client panicked")?;
        let deadline = Instant::now() + TEST_TIMEOUT;
        while server.active_connection_count() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.active_connection_count(), 0);
        Ok(())
    }

    fn _no_op_handler() -> impl Fn(PeerCredentials, Envelope) -> Option<Envelope> + Send + Sync {
        |_credentials: PeerCredentials, _envelope: Envelope| None
    }

    fn _echo_status_handler() -> impl Fn(PeerCredentials, Envelope) -> Option<Envelope> + Send + Sync
    {
        |credentials: PeerCredentials, envelope: Envelope| {
            if !credentials.is_same_user(current_uid()) {
                return None;
            }
            reply_envelope(
                envelope.request_id,
                &ResponsePayload::Status(StatusDto {
                    daemon_build: "accept-once".into(),
                    protocol_minor: PROTOCOL_MINOR,
                    active_session: None,
                    last_session_timings_us: None,
                    capabilities: vec![],
                    installed_model: None,
                    model_provisioning: None,
                    portal_keyboard: None,
                }),
            )
        }
    }

    /// Protocol constants stay pinned so wire drift fails loudly here.
    #[test]
    fn ipc_protocol_versions_pinned() {
        assert_eq!(PROTOCOL_MAJOR, 1);
        // Minor 5 added offline model provisioning; minor 6 the optional typed
        // capture end reason on the recovery-entry payload; minor 7 the
        // additive atomic toggle request; minor 8 content-free timings;
        // minor 9 bounded recognition settings on session start/toggle;
        // minor 10 deleting one optional-History entry; minor 11 the desktop
        // shortcut commands; minor 12 the optional transcription time on
        // History rows; minor 13 the model listing; minor 14 choosing the
        // model in use; minor 15 a model and device per pipeline; minor 16
        // downloading a model; minor 17 the Store; minor 18 the delivery
        // result on `committed`; minor 19 the AI assistant; minor 20 the
        // language models; minor 21 the microphone level; minor 22 unloading
        // the assistant; minor 23 copying a History entry; minor 24 the writing assistant; minor 25 meetings; minor 26 meeting notes; minor 27 audio options; minor 28 the voice filter; minor 29 noise suppression levels; minor 30 the vocabulary; minor 31 choosing the microphone; minor 32 press and hold; minor 33 start and stop sounds; minor 34 History settings; minor 35 text options; minor 36 per-pipeline Voice settings; minor 37 meeting import; minor 38 the delivery method; minor 39 meeting options; minor 40 unloading the clean-up model when idle; minor 41 voice strictness; minor 42 spoken line breaks; minor 43 meeting devices.
        // Minor 54: the dictation language.
        assert_eq!(PROTOCOL_MINOR, 54);
        assert_eq!(MAX_CONCURRENT_CONNECTIONS, 64);
        assert_eq!(LOCK_FILE_NAME, "narrivyd.lock");
    }
}

#[cfg(test)]
#[path = "untrusted_peer_tests.rs"]
mod untrusted_peer_tests;
