//! Win32 FFI for the named-pipe transport. This is the only module in the
//! crate allowed to contain `unsafe`; every block states the invariant that
//! keeps it sound. Callers above this module see owned handles, safe slices
//! and typed errors only.
//!
//! I/O is always overlapped and always waited to completion before a call
//! returns: a timed-out or cancelled operation is cancelled with
//! `CancelIoEx` and then drained with a blocking `GetOverlappedResult`, so
//! the kernel never holds a pointer into a buffer or `OVERLAPPED` that Rust
//! has already released.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_INSUFFICIENT_BUFFER, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING,
    ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, FALSE,
    GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, IsValidSid, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, ResetEvent, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};

use narrivy_ipc::error::{IpcError, IpcErrorCode};

use crate::security::UserSid;

/// Kernel buffer size for each direction of a pipe instance. Frames larger
/// than this simply take several writes.
const PIPE_BUFFER_BYTES: u32 = 64 * 1024;

/// Upper bound for a `TOKEN_USER` buffer: the structure plus the largest SID.
const MAX_TOKEN_USER_BYTES: u32 = 1024;

/// Raw OS errors that mean "the other end has gone": reads see EOF.
const PEER_GONE_ERRORS: [u32; 3] = [ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED];

fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle()
}

/// True when `error` means the peer closed or disconnected the pipe.
pub(crate) fn is_peer_gone(error: &io::Error) -> bool {
    error
        .raw_os_error()
        .is_some_and(|code| PEER_GONE_ERRORS.contains(&(code as u32)))
}

/// UTF-16 with a terminating NUL; interior NULs are refused because Win32
/// would silently truncate the name at them.
pub(crate) fn wide_nul(text: &str) -> io::Result<Vec<u16>> {
    if text.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "name contains NUL",
        ));
    }
    Ok(text.encode_utf16().chain(std::iter::once(0)).collect())
}

/// Wrap a handle a Win32 constructor returned, rejecting both failure
/// sentinels (`NULL` and `INVALID_HANDLE_VALUE`).
fn own(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `handle` was just returned by a successful Win32 constructor
    // (checked against both failure sentinels above), is open, and has no
    // other owner, so OwnedHandle may close it exactly once on drop.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// Manual-reset, initially non-signalled event (overlapped I/O requires
/// manual reset; cancellation events must stay signalled once set).
pub(crate) fn create_event() -> io::Result<OwnedHandle> {
    // SAFETY: null security attributes and name are documented as valid
    // (default security, unnamed event); the call takes no other pointers.
    own(unsafe { CreateEventW(std::ptr::null(), 1, FALSE, std::ptr::null()) })
}

pub(crate) fn set_event(event: &OwnedHandle) -> io::Result<()> {
    // SAFETY: `event` is a live event handle owned by the caller.
    if unsafe { SetEvent(raw(event)) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn is_signalled(event: &OwnedHandle) -> bool {
    // SAFETY: `event` is a live waitable handle; a zero timeout only polls.
    unsafe { WaitForSingleObject(raw(event), 0) == WAIT_OBJECT_0 }
}

/// Cancel every outstanding I/O on `handle`, from any thread. Each waiter
/// then drains its own operation; see the module docs.
pub(crate) fn cancel_all_io(handle: &OwnedHandle) {
    // SAFETY: `handle` is live; a null OVERLAPPED pointer means "all I/O
    // issued on this handle" and dereferences nothing. Failure (nothing to
    // cancel) is harmless.
    let _ = unsafe { CancelIoEx(raw(handle), std::ptr::null()) };
}

/// Self-relative security descriptor allocated by
/// `ConvertStringSecurityDescriptorToSecurityDescriptorW`, freed on drop.
pub(crate) struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is immutable after creation, owned solely by this
// value, and only ever read (by CreateNamedPipeW); LocalFree may run on any
// thread.
unsafe impl Send for SecurityDescriptor {}
// SAFETY: as above; shared access is read-only.
unsafe impl Sync for SecurityDescriptor {}

impl SecurityDescriptor {
    pub(crate) fn from_sddl(sddl: &str) -> io::Result<Self> {
        let wide = wide_nul(sddl)?;
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated UTF-16 that outlives the call;
        // `descriptor` is a valid out-pointer; the size out-pointer may be
        // null. On success the function allocates the descriptor with
        // LocalAlloc and transfers ownership to us.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || descriptor.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: the pointer came from a successful SDDL conversion, which
        // allocates with LocalAlloc, and is freed exactly once here.
        unsafe { LocalFree(self.0) };
    }
}

/// Create one server instance of the pipe `name` (NUL-terminated UTF-16).
///
/// `first` adds `FILE_FLAG_FIRST_PIPE_INSTANCE`, so creation fails with
/// `ERROR_ACCESS_DENIED` if any instance of the name already exists, whoever
/// created it; that is both the single-instance lock and the anti-squatting
/// check. Every instance refuses remote clients and carries `descriptor`.
pub(crate) fn create_pipe_instance(
    name: &[u16],
    descriptor: &SecurityDescriptor,
    first: bool,
) -> io::Result<OwnedHandle> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: FALSE,
    };
    let open_mode = PIPE_ACCESS_DUPLEX
        | FILE_FLAG_OVERLAPPED
        | if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
    let pipe_mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
    debug_assert_eq!(name.last(), Some(&0));
    // SAFETY: `name` is NUL-terminated UTF-16 (built by `wide_nul`) and
    // `attributes` points at a live descriptor; both outlive the call, which
    // copies what it needs. Failure returns INVALID_HANDLE_VALUE, checked by
    // `own`.
    own(unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            pipe_mode,
            PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            &attributes,
        )
    })
}

/// Open the client end of pipe `name` for overlapped duplex I/O.
///
/// `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION` caps what the server
/// may do with the client's identity: it can identify, never impersonate,
/// so a squatting server cannot act as the user.
pub(crate) fn open_client_pipe(name: &[u16]) -> io::Result<OwnedHandle> {
    debug_assert_eq!(name.last(), Some(&0));
    // SAFETY: `name` is NUL-terminated UTF-16 that outlives the call; null
    // security attributes and template handle are documented as valid.
    own(unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            std::ptr::null_mut(),
        )
    })
}

/// Wait up to `timeout` for an instance of busy pipe `name` to free up.
pub(crate) fn wait_named_pipe(name: &[u16], timeout: Duration) -> io::Result<()> {
    // A zero wait means "use the pipe's default"; always wait at least 1 ms.
    let millis = timeout_millis(Some(timeout)).max(1);
    // SAFETY: `name` is NUL-terminated UTF-16 that outlives the call.
    if unsafe { WaitNamedPipeW(name.as_ptr(), millis) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn pipe_client_process_id(pipe: &OwnedHandle) -> io::Result<u32> {
    let mut pid = 0_u32;
    // SAFETY: `pipe` is a live server-end pipe handle and `pid` a valid
    // out-pointer for the duration of the call.
    if unsafe { GetNamedPipeClientProcessId(raw(pipe), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid)
}

pub(crate) fn pipe_server_process_id(pipe: &OwnedHandle) -> io::Result<u32> {
    let mut pid = 0_u32;
    // SAFETY: `pipe` is a live client-end pipe handle and `pid` a valid
    // out-pointer for the duration of the call.
    if unsafe { GetNamedPipeServerProcessId(raw(pipe), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid)
}

/// User SID of this process's primary token, read from the kernel rather
/// than from environment variables.
pub fn current_user_sid() -> Result<UserSid, IpcError> {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no
    // closing and is always valid for this process.
    let process = unsafe { GetCurrentProcess() };
    token_sid_of(process)
}

/// User SID of process `pid`'s primary token. Fails closed: a process that
/// cannot be opened or queried is never treated as the same user.
pub(crate) fn process_user_sid(pid: u32) -> Result<UserSid, IpcError> {
    // SAFETY: OpenProcess takes no pointers; failure returns NULL, which
    // `own` rejects.
    let process = own(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) })
        .map_err(|error| denied("open peer process", &error))?;
    token_sid_of(raw(&process))
}

fn token_sid_of(process: HANDLE) -> Result<UserSid, IpcError> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `process` is a live process handle (or the current-process
    // pseudo-handle) for the duration of the call; `token` is a valid
    // out-pointer. On success the new token handle is ours to close.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(denied("open process token", &io::Error::last_os_error()));
    }
    let token = own(token).map_err(|error| denied("open process token", &error))?;
    token_user_sid(&token)
}

fn token_user_sid(token: &OwnedHandle) -> Result<UserSid, IpcError> {
    let mut needed = 0_u32;
    // SAFETY: a null buffer of length 0 is the documented size query; only
    // `needed` (a valid out-pointer) is written.
    let sized =
        unsafe { GetTokenInformation(raw(token), TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    if sized != 0 {
        return Err(IpcError::permission_denied(
            "token user query returned no size",
        ));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || needed == 0
        || needed > MAX_TOKEN_USER_BYTES
    {
        return Err(denied("size token user", &error));
    }
    // u64 storage gives the pointer alignment TOKEN_USER requires.
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
    let capacity = (buffer.len() * 8) as u32;
    // SAFETY: `buffer` is writable for `capacity` bytes, 8-byte aligned, and
    // outlives the call; `needed` is a valid out-pointer.
    let ok = unsafe {
        GetTokenInformation(
            raw(token),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            capacity,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(denied("read token user", &io::Error::last_os_error()));
    }
    let start = buffer.as_ptr() as usize;
    let end = start + capacity as usize;
    // SAFETY: on success the buffer begins with an initialised, aligned
    // TOKEN_USER; it is only read while `buffer` is alive and unmodified.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let sid_start = sid as usize;
    // The SID must lie inside our buffer before any byte of it is read.
    if sid.is_null() || sid_start < start || sid_start + 8 > end {
        return Err(IpcError::permission_denied(
            "token user SID is out of bounds",
        ));
    }
    // SAFETY: `sid` points at least 8 readable bytes inside `buffer`
    // (checked above); IsValidSid and GetLengthSid read only the SID header
    // and sub-authorities it declares.
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(IpcError::permission_denied("token user SID is invalid"));
    }
    // SAFETY: `sid` is a valid SID inside `buffer` (checked above).
    let length = unsafe { GetLengthSid(sid) } as usize;
    if sid_start + length > end {
        return Err(IpcError::permission_denied(
            "token user SID is out of bounds",
        ));
    }
    // SAFETY: `[sid, sid + length)` lies inside the live, initialised
    // `buffer` (checked above) and nothing mutates it while borrowed.
    let bytes = unsafe { std::slice::from_raw_parts(sid.cast::<u8>().cast_const(), length) };
    UserSid::from_bytes(bytes)
}

fn denied(action: &str, error: &io::Error) -> IpcError {
    IpcError::new(
        IpcErrorCode::PermissionDenied,
        format!("{action} failed: {error}"),
    )
}

/// One overlapped operation on a pipe handle.
pub(crate) enum Operation<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
    /// `ConnectNamedPipe` on a server instance.
    Connect,
}

/// Error kind reported when the caller's cancellation event fired.
pub(crate) const CANCELLED_KIND: io::ErrorKind = io::ErrorKind::ConnectionAborted;

/// Run one overlapped operation to completion, a timeout, or cancellation.
///
/// `event` must be a manual-reset event used by no other concurrent
/// operation. Returns bytes transferred; [`io::ErrorKind::TimedOut`] after
/// `timeout`; [`CANCELLED_KIND`] once `cancel` is (or was already)
/// signalled. An operation that completed before its cancellation took
/// effect still reports its bytes, so no data is lost.
pub(crate) fn overlapped(
    pipe: &OwnedHandle,
    event: &OwnedHandle,
    cancel: Option<&OwnedHandle>,
    timeout: Option<Duration>,
    operation: Operation<'_>,
) -> io::Result<u32> {
    if cancel.is_some_and(is_signalled) {
        return Err(io::Error::new(CANCELLED_KIND, "pipe operation cancelled"));
    }
    let is_connect = matches!(operation, Operation::Connect);
    // Only completion of the operation below may signal `event`; a stale
    // signal from an earlier operation must not end the wait early.
    // SAFETY: `event` is a live event handle owned by the caller.
    if unsafe { ResetEvent(raw(event)) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OVERLAPPED is plain data for which all-zero is the documented
    // initial state.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = raw(event);
    // SAFETY (all three arms): `pipe` is a live handle opened with
    // FILE_FLAG_OVERLAPPED; the buffer slices and `overlapped` stay alive and
    // unmoved until this function returns, and every path below that can
    // leave an operation in flight first waits for its completion (directly
    // or after CancelIoEx), so the kernel never writes through a dangling
    // pointer. Lengths are clamped to u32.
    let started = unsafe {
        match operation {
            Operation::Read(buffer) => ReadFile(
                raw(pipe),
                buffer.as_mut_ptr(),
                buffer.len().min(u32::MAX as usize) as u32,
                std::ptr::null_mut(),
                &mut overlapped,
            ),
            Operation::Write(buffer) => WriteFile(
                raw(pipe),
                buffer.as_ptr(),
                buffer.len().min(u32::MAX as usize) as u32,
                std::ptr::null_mut(),
                &mut overlapped,
            ),
            Operation::Connect => ConnectNamedPipe(raw(pipe), &mut overlapped),
        }
    };
    if started == 0 {
        let error = io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_IO_PENDING) => {}
            // A client connected between instance creation and this call.
            Some(ERROR_PIPE_CONNECTED) if is_connect => return Ok(0),
            // No operation was queued; nothing references `overlapped`.
            _ => return Err(error),
        }
    }

    let mut handles = [raw(event), std::ptr::null_mut()];
    let count = match cancel {
        Some(cancel) => {
            handles[1] = raw(cancel);
            2
        }
        None => 1,
    };
    // SAFETY: `handles[..count]` are live waitable handles; the array
    // outlives the call.
    let wait =
        unsafe { WaitForMultipleObjects(count, handles.as_ptr(), FALSE, timeout_millis(timeout)) };
    if wait == WAIT_OBJECT_0 {
        return finish(pipe, &overlapped);
    }
    // Timed out, cancelled, or the wait itself failed: cancel this exact
    // operation, then drain it so the kernel releases our pointers.
    // SAFETY: `overlapped` identifies the operation started above on the
    // live `pipe`; CancelIoEx only reads it to match the request.
    let _ = unsafe { CancelIoEx(raw(pipe), &overlapped) };
    match finish(pipe, &overlapped) {
        Ok(transferred) => Ok(transferred),
        Err(error) if error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) => {
            if wait == WAIT_TIMEOUT {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "pipe operation deadline",
                ))
            } else {
                Err(io::Error::new(CANCELLED_KIND, "pipe operation cancelled"))
            }
        }
        Err(error) => Err(error),
    }
}

/// Block until the operation behind `overlapped` has completed and return
/// its result; after this the kernel no longer references it.
fn finish(pipe: &OwnedHandle, overlapped: &OVERLAPPED) -> io::Result<u32> {
    loop {
        let mut transferred = 0_u32;
        // SAFETY: `overlapped` describes an operation issued on the live
        // `pipe`; bWait = TRUE blocks on its event until it completes;
        // `transferred` is a valid out-pointer.
        if unsafe { GetOverlappedResult(raw(pipe), overlapped, &mut transferred, 1) } != 0 {
            return Ok(transferred);
        }
        let error = io::Error::last_os_error();
        // Still pending (the event was signalled early): wait again rather
        // than return while the kernel still owns the buffer.
        if error.raw_os_error() != Some(ERROR_IO_INCOMPLETE as i32) {
            return Err(error);
        }
    }
}

/// Milliseconds for a Win32 wait: `None` waits forever; durations round up
/// so a sub-millisecond budget still waits, and stay below `INFINITE`.
fn timeout_millis(timeout: Option<Duration>) -> u32 {
    match timeout {
        None => INFINITE,
        Some(timeout) => {
            let millis = timeout.as_nanos().div_ceil(1_000_000);
            u32::try_from(millis)
                .unwrap_or(INFINITE - 1)
                .min(INFINITE - 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_millis_rounds_up_and_never_means_infinite() {
        assert_eq!(timeout_millis(None), INFINITE);
        assert_eq!(timeout_millis(Some(Duration::ZERO)), 0);
        assert_eq!(timeout_millis(Some(Duration::from_nanos(1))), 1);
        assert_eq!(timeout_millis(Some(Duration::from_millis(1500))), 1500);
        assert_eq!(
            timeout_millis(Some(Duration::from_secs(u64::MAX))),
            INFINITE - 1
        );
    }

    #[test]
    fn wide_names_are_nul_terminated_and_reject_interior_nul() -> io::Result<()> {
        assert_eq!(wide_nul("ab")?, vec![u16::from(b'a'), u16::from(b'b'), 0]);
        assert!(wide_nul("a\0b").is_err());
        Ok(())
    }

    #[test]
    fn peer_gone_errors_read_as_eof() {
        for code in PEER_GONE_ERRORS {
            assert!(is_peer_gone(&io::Error::from_raw_os_error(code as i32)));
        }
        assert!(!is_peer_gone(&io::Error::from_raw_os_error(
            ERROR_OPERATION_ABORTED as i32
        )));
    }

    #[test]
    fn current_user_sid_matches_windows_string_form() -> Result<(), Box<dyn std::error::Error>> {
        let sid = current_user_sid()?;
        let text = sid.to_string();
        assert!(text.starts_with("S-1-"));
        assert_eq!(UserSid::parse(&text)?, sid);
        // Same answer through the pid path the transport uses for peers.
        assert_eq!(process_user_sid(std::process::id())?, sid);
        Ok(())
    }

    #[test]
    fn unknown_process_is_never_the_same_user() {
        // Process id 0 is the System Idle Process, which cannot be opened.
        assert!(process_user_sid(0).is_err());
    }
}
