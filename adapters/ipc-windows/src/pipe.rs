//! A connected named-pipe end with socket-like semantics: per-direction
//! timeouts, cheap clones that share one connection, and a `shutdown` that
//! interrupts every clone's blocked I/O from another thread.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::os::windows::io::OwnedHandle;
use std::sync::Arc;
use std::time::Duration;

use crate::sys::{self, Operation};

/// The connection itself; closed when the last clone drops.
struct Connection {
    pipe: OwnedHandle,
    /// Manual-reset event, signalled once by [`PipeStream::shutdown`].
    closed: OwnedHandle,
}

/// One handle onto a connected pipe end.
///
/// Like a socket clone, every `PipeStream` from [`PipeStream::try_clone`]
/// shares the connection but owns its own completion event and timeouts, so
/// one clone can read while another writes. Two clones must not read (or
/// write) concurrently, exactly as with a byte stream.
pub(crate) struct PipeStream {
    connection: Arc<Connection>,
    event: OwnedHandle,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
}

impl PipeStream {
    pub(crate) fn from_handle(pipe: OwnedHandle) -> io::Result<Self> {
        let connection = Arc::new(Connection {
            pipe,
            closed: sys::create_event()?,
        });
        Ok(Self {
            connection,
            event: sys::create_event()?,
            read_timeout: None,
            write_timeout: None,
        })
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            connection: self.connection.clone(),
            event: sys::create_event()?,
            read_timeout: self.read_timeout,
            write_timeout: self.write_timeout,
        })
    }

    /// `None` blocks indefinitely; a zero duration is refused, matching
    /// `std::os::unix::net::UnixStream`.
    pub(crate) fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout = checked_timeout(timeout)?;
        Ok(())
    }

    pub(crate) fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.write_timeout = checked_timeout(timeout)?;
        Ok(())
    }

    /// Interrupt and end the connection for every clone: blocked and later
    /// reads see EOF, writes fail with `BrokenPipe`. The handle itself closes
    /// when the last clone drops.
    pub(crate) fn shutdown(&self) {
        let _ = sys::set_event(&self.connection.closed);
        sys::cancel_all_io(&self.connection.pipe);
    }

    pub(crate) fn handle(&self) -> &OwnedHandle {
        &self.connection.pipe
    }
}

fn checked_timeout(timeout: Option<Duration>) -> io::Result<Option<Duration>> {
    if timeout.is_some_and(|timeout| timeout.is_zero()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot set a 0 duration timeout",
        ));
    }
    Ok(timeout)
}

impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        match sys::overlapped(
            &self.connection.pipe,
            &self.event,
            Some(&self.connection.closed),
            self.read_timeout,
            Operation::Read(buffer),
        ) {
            Ok(count) => Ok(count as usize),
            // A closed peer or a local shutdown both read as EOF.
            Err(error) if sys::is_peer_gone(&error) || error.kind() == sys::CANCELLED_KIND => Ok(0),
            Err(error) => Err(error),
        }
    }
}

impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        match sys::overlapped(
            &self.connection.pipe,
            &self.event,
            Some(&self.connection.closed),
            self.write_timeout,
            Operation::Write(buffer),
        ) {
            Ok(count) => Ok(count as usize),
            Err(error) if sys::is_peer_gone(&error) || error.kind() == sys::CANCELLED_KIND => {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"))
            }
            Err(error) => Err(error),
        }
    }

    /// Completed writes already sit in the kernel pipe buffer, as with a
    /// socket. `FlushFileBuffers` is deliberately not used: it blocks without
    /// a timeout until the peer reads everything.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Test fixtures: a connected server/client pair on a unique private pipe.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::PipeStream;
    use crate::security::endpoint_sddl;
    use crate::sys::{self, Operation, SecurityDescriptor};

    /// Unique pipe path for one test, owned by the current user.
    pub(crate) fn unique_pipe_path(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        std::path::PathBuf::from(format!(
            r"\\.\pipe\narrivy-test-{label}-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Owner-only first instance of a fresh pipe, not yet connected.
    pub(crate) fn listening_instance(
        label: &str,
    ) -> Result<(std::path::PathBuf, std::os::windows::io::OwnedHandle), Box<dyn std::error::Error>>
    {
        let path = unique_pipe_path(label);
        let descriptor = SecurityDescriptor::from_sddl(&endpoint_sddl(&sys::current_user_sid()?))?;
        let name = sys::wide_nul(path.to_str().ok_or("pipe path")?)?;
        let instance = sys::create_pipe_instance(&name, &descriptor, true)?;
        Ok((path, instance))
    }

    /// Accept one client on `instance` within `timeout`.
    pub(crate) fn accept(
        instance: std::os::windows::io::OwnedHandle,
        timeout: Duration,
    ) -> Result<PipeStream, Box<dyn std::error::Error>> {
        let event = sys::create_event()?;
        sys::overlapped(&instance, &event, None, Some(timeout), Operation::Connect)?;
        Ok(PipeStream::from_handle(instance)?)
    }

    /// Raw client end of an existing pipe, with no handshake or peer check.
    pub(crate) fn connect_raw(
        path: &std::path::Path,
    ) -> Result<PipeStream, Box<dyn std::error::Error + Send + Sync>> {
        let name = sys::wide_nul(path.to_str().ok_or("pipe path")?)?;
        let mut stream = PipeStream::from_handle(sys::open_client_pipe(&name)?)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(stream)
    }

    /// Connected `(server, client)` pair, like `UnixStream::pair`.
    pub(crate) fn pair() -> Result<(PipeStream, PipeStream), Box<dyn std::error::Error>> {
        let (path, instance) = listening_instance("pair")?;
        let name = sys::wide_nul(path.to_str().ok_or("pipe path")?)?;
        let client = PipeStream::from_handle(sys::open_client_pipe(&name)?)?;
        let server = accept(instance, Duration::from_secs(5))?;
        Ok((server, client))
    }
}
