//! Narrow direct-zbus owner for XDG RemoteDesktop permission and EIS setup.
//!
//! This adapter calls only the permission/session methods approved by
//! ADR-0023 and owns the minimal text-only EIS sender approved by ADR-0026.
//! Daemon startup composes it for explicit desktop permission requests. Text
//! submission remains capability-disabled until separately qualified.

use std::collections::HashMap;
use std::future::Future;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::{FutureExt, StreamExt};
use rand::distr::{Alphanumeric, SampleString};
use tokio::runtime::Runtime;
use zbus::proxy::SignalStream;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Str};

use crate::portal_eis::{
    PortalCancellation, PortalDriverEvent, PortalEisTextFailure, PortalEisTextPayload,
    PortalEisTextSubmission, PortalParentWindow, PortalSessionDriver, PortalSessionFailure,
};
use crate::portal_eis_sender::PortalEisSender;

const DESKTOP_DESTINATION: &str = "org.freedesktop.portal.Desktop";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const REMOTE_DESKTOP_INTERFACE: &str = "org.freedesktop.portal.RemoteDesktop";
const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";
const SESSION_INTERFACE: &str = "org.freedesktop.portal.Session";
const REQUEST_PATH_PREFIX: &str = "/org/freedesktop/portal/desktop/request";
const SESSION_PATH_PREFIX: &str = "/org/freedesktop/portal/desktop/session";
const KEYBOARD_DEVICE: u32 = 1;
const NO_PERSISTENCE: u32 = 0;
const POLL_SLICE: Duration = Duration::from_millis(5);
const CLEANUP_RESERVE: Duration = Duration::from_millis(100);

type PortalResults = HashMap<String, OwnedValue>;

#[derive(Debug, PartialEq, Eq)]
enum SystemCallFailure {
    Bus,
    Malformed,
    PathMismatch(OwnedObjectPath),
    LocalCancelled,
    TimedOut,
    PortalCancelled,
    PortalFailed,
    Revoked,
}

/// Production-capable permission/session owner for the daemon portal actor.
///
/// Construction is inert: it creates a private current-thread runtime but does
/// not connect to D-Bus, open a portal request, or create an EI context. All
/// externally visible errors are stable typed codes and never retain D-Bus or
/// payload text.
pub struct DirectZbusPortalDriver {
    runtime: Runtime,
    connection: Option<zbus::Connection>,
    pending_request: Option<OwnedObjectPath>,
    session_path: Option<OwnedObjectPath>,
    session_closed: Option<SignalStream<'static>>,
    eis_sender: Option<PortalEisSender>,
    session_may_exist: Arc<AtomicBool>,
}

impl DirectZbusPortalDriver {
    pub fn new() -> Result<Self, PortalSessionFailure> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| PortalSessionFailure::PortalUnavailable)?;
        Ok(Self {
            runtime,
            connection: None,
            pending_request: None,
            session_path: None,
            session_closed: None,
            eis_sender: None,
            session_may_exist: Arc::new(AtomicBool::new(false)),
        })
    }

    fn ensure_connection(
        &mut self,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<zbus::Connection, PortalSessionFailure> {
        if let Some(connection) = &self.connection {
            return Ok(connection.clone());
        }
        let connection = wait_cancellable(
            &self.runtime,
            deadline,
            cancellation,
            Box::pin(async {
                zbus::Connection::session()
                    .await
                    .map_err(|_| SystemCallFailure::Bus)
            }),
        )
        .map_err(map_setup_failure)?;
        self.connection = Some(connection.clone());
        Ok(connection)
    }

    fn create_session(
        &mut self,
        connection: &zbus::Connection,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<(), PortalSessionFailure> {
        let request_token = next_token("create");
        let session_token = next_token("session");
        let request_path = predicted_path(connection, REQUEST_PATH_PREFIX, &request_token)?;
        let session_path = predicted_path(connection, SESSION_PATH_PREFIX, &session_token)?;

        let session_proxy = wait_cancellable(
            &self.runtime,
            operation_deadline(deadline),
            cancellation,
            Box::pin(build_proxy(
                connection.clone(),
                session_path.clone(),
                SESSION_INTERFACE,
            )),
        )
        .map_err(map_request_failure)?;
        let session_closed = wait_cancellable(
            &self.runtime,
            operation_deadline(deadline),
            cancellation,
            Box::pin(async move {
                session_proxy
                    .receive_signal("Closed")
                    .await
                    .map_err(|_| SystemCallFailure::Bus)
            }),
        )
        .map_err(map_request_failure)?;

        // Retain the predicted session before issuing CreateSession. If the
        // call succeeds but its response is malformed, lifecycle cleanup can
        // still close the only session handle Narrivy requested.
        self.session_path = Some(session_path.clone());
        self.session_closed = Some(session_closed);

        let mut options = PortalOptions::new();
        options.string("handle_token", &request_token);
        options.string("session_handle_token", &session_token);
        let results = self.portal_request(
            connection,
            request_path,
            "CreateSession",
            options.into_map(),
            PortalRequestControl {
                deadline,
                cancellation,
                authority_may_exist: Some(Arc::clone(&self.session_may_exist)),
            },
        )?;
        self.accept_session_handle(&session_path, &results)
    }

    fn accept_session_handle(
        &mut self,
        predicted: &OwnedObjectPath,
        results: &PortalResults,
    ) -> Result<(), PortalSessionFailure> {
        let returned = session_handle(results).ok_or(PortalSessionFailure::RequestFailed)?;
        if &returned == predicted {
            return Ok(());
        }
        // Reject the protocol mismatch, but retain the authoritative returned
        // handle so lifecycle cleanup closes the real session.
        self.session_path = Some(returned);
        self.session_closed = None;
        Err(PortalSessionFailure::RequestFailed)
    }

    fn select_keyboard(
        &mut self,
        connection: &zbus::Connection,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<(), PortalSessionFailure> {
        let session = self
            .session_path
            .clone()
            .ok_or(PortalSessionFailure::RequestFailed)?;
        let token = next_token("select");
        let request = predicted_path(connection, REQUEST_PATH_PREFIX, &token)?;
        let mut options = PortalOptions::new();
        options.string("handle_token", &token);
        options.u32("types", KEYBOARD_DEVICE);
        options.u32("persist_mode", NO_PERSISTENCE);
        self.portal_request(
            connection,
            request,
            "SelectDevices",
            (session, options.into_map()),
            PortalRequestControl {
                deadline,
                cancellation,
                authority_may_exist: None,
            },
        )?;
        Ok(())
    }

    fn start_session(
        &mut self,
        connection: &zbus::Connection,
        parent: &PortalParentWindow,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<(), PortalSessionFailure> {
        let session = self
            .session_path
            .clone()
            .ok_or(PortalSessionFailure::RequestFailed)?;
        let token = next_token("start");
        let request = predicted_path(connection, REQUEST_PATH_PREFIX, &token)?;
        let mut options = PortalOptions::new();
        options.string("handle_token", &token);
        let results = self.portal_request(
            connection,
            request,
            "Start",
            (session, parent.as_str().to_owned(), options.into_map()),
            PortalRequestControl {
                deadline,
                cancellation,
                authority_may_exist: None,
            },
        )?;
        if selected_devices(&results) != Some(KEYBOARD_DEVICE) {
            return Err(PortalSessionFailure::PermissionDenied);
        }
        Ok(())
    }

    fn portal_request<B>(
        &mut self,
        connection: &zbus::Connection,
        request_path: OwnedObjectPath,
        method: &'static str,
        body: B,
        control: PortalRequestControl<'_>,
    ) -> Result<PortalResults, PortalSessionFailure>
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType + Send + Sync + 'static,
    {
        let deadline = control.deadline;
        self.pending_request = Some(request_path.clone());
        let request_may_exist = Arc::new(AtomicBool::new(false));
        let result = wait_cancellable_with_revocation(
            &self.runtime,
            operation_deadline(deadline),
            control.cancellation,
            Box::pin(call_portal_request(
                connection.clone(),
                request_path,
                method,
                body,
                Arc::clone(&request_may_exist),
                control.authority_may_exist,
            )),
            self.session_closed.as_mut(),
        );
        match result {
            Ok(results) => {
                self.pending_request = None;
                Ok(results)
            }
            Err(
                failure @ (SystemCallFailure::PortalCancelled | SystemCallFailure::PortalFailed),
            ) => {
                // A terminal Response removes the Request object. Calling
                // Request.Close afterward would manufacture a cleanup failure.
                self.pending_request = None;
                Err(map_request_failure(failure))
            }
            Err(SystemCallFailure::Revoked) => {
                self.session_may_exist.store(false, Ordering::Release);
                Err(PortalSessionFailure::Revoked)
            }
            Err(SystemCallFailure::PathMismatch(actual)) => {
                // A nonconforming portal returned a different authoritative
                // request handle. Close that real object, never only the
                // predicted path.
                self.pending_request = Some(actual);
                if self.close_pending_request(deadline).is_err() {
                    return Err(PortalSessionFailure::CloseFailed);
                }
                Err(PortalSessionFailure::RequestFailed)
            }
            Err(failure) => {
                if !request_may_exist.load(Ordering::Acquire) {
                    self.pending_request = None;
                    return Err(map_request_failure(failure));
                }
                if self.close_pending_request(deadline).is_err() {
                    return Err(PortalSessionFailure::CloseFailed);
                }
                Err(map_request_failure(failure))
            }
        }
    }

    fn close_pending_request(&mut self, deadline: Instant) -> Result<(), PortalSessionFailure> {
        let Some(path) = self.pending_request.clone() else {
            return Ok(());
        };
        let connection = self
            .connection
            .clone()
            .ok_or(PortalSessionFailure::CloseFailed)?;
        wait_uncancellable(
            &self.runtime,
            deadline,
            Box::pin(close_object(connection, path, REQUEST_INTERFACE)),
        )
        .map_err(|_| PortalSessionFailure::CloseFailed)?;
        self.pending_request = None;
        Ok(())
    }

    fn close_session(&mut self, deadline: Instant) -> Result<(), PortalSessionFailure> {
        let Some(path) = self.session_path.clone() else {
            return Ok(());
        };
        if !self.session_may_exist.load(Ordering::Acquire) {
            self.session_path = None;
            self.session_closed = None;
            self.eis_sender = None;
            return Ok(());
        }
        let connection = self
            .connection
            .clone()
            .ok_or(PortalSessionFailure::CloseFailed)?;
        wait_uncancellable(
            &self.runtime,
            deadline,
            Box::pin(close_object(connection, path, SESSION_INTERFACE)),
        )
        .map_err(|_| PortalSessionFailure::CloseFailed)?;
        self.session_path = None;
        self.session_closed = None;
        self.eis_sender = None;
        self.session_may_exist.store(false, Ordering::Release);
        Ok(())
    }
}

impl PortalSessionDriver for DirectZbusPortalDriver {
    fn request_keyboard_grant(
        &mut self,
        parent: &PortalParentWindow,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<(), PortalSessionFailure> {
        if self.session_path.is_some()
            || self.pending_request.is_some()
            || self.eis_sender.is_some()
        {
            return Err(PortalSessionFailure::RequestFailed);
        }
        let connection = self.ensure_connection(deadline, cancellation)?;
        self.create_session(&connection, deadline, cancellation)?;
        self.select_keyboard(&connection, deadline, cancellation)?;
        self.start_session(&connection, parent, deadline, cancellation)
    }

    fn connect_transport(
        &mut self,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<(), PortalSessionFailure> {
        if self.eis_sender.is_some() {
            return Err(PortalSessionFailure::TransportFailed);
        }
        let connection = self.ensure_connection(deadline, cancellation)?;
        let session = self
            .session_path
            .clone()
            .ok_or(PortalSessionFailure::TransportFailed)?;
        let future = Box::pin(async move {
            let proxy = build_proxy(
                connection,
                OwnedObjectPath::try_from(DESKTOP_PATH)
                    .map_err(|_| SystemCallFailure::Malformed)?,
                REMOTE_DESKTOP_INTERFACE,
            )
            .await?;
            let options: HashMap<String, OwnedValue> = HashMap::new();
            proxy
                .call::<_, _, zbus::zvariant::OwnedFd>("ConnectToEIS", &(session, options))
                .await
                .map(OwnedFd::from)
                .map_err(|_| SystemCallFailure::Bus)
        });
        let fd = wait_cancellable_with_revocation(
            &self.runtime,
            deadline,
            cancellation,
            future,
            self.session_closed.as_mut(),
        );
        if matches!(fd, Err(SystemCallFailure::Revoked)) {
            self.session_may_exist.store(false, Ordering::Release);
        }
        let fd = fd.map_err(map_transport_failure)?;
        let sender = PortalEisSender::connect(UnixStream::from(fd), deadline, cancellation)?;
        self.eis_sender = Some(sender);
        Ok(())
    }

    fn close(&mut self, deadline: Instant) -> Result<(), PortalSessionFailure> {
        let request = self.close_pending_request(deadline);
        let session = self.close_session(deadline);
        if request.is_err() || session.is_err() {
            Err(PortalSessionFailure::CloseFailed)
        } else {
            Ok(())
        }
    }

    fn poll_event(&mut self) -> Result<Option<PortalDriverEvent>, PortalSessionFailure> {
        let Some(stream) = self.session_closed.as_mut() else {
            return Ok(None);
        };
        let portal_event = match stream.next().now_or_never() {
            None => Ok(None),
            Some(Some(_)) => {
                self.session_may_exist.store(false, Ordering::Release);
                Ok(Some(PortalDriverEvent::Revoked))
            }
            Some(None) => Err(PortalSessionFailure::TransportFailed),
        }?;
        if portal_event.is_some() {
            return Ok(portal_event);
        }
        if let Some(sender) = self.eis_sender.as_mut()
            && sender.poll_revoked()?
        {
            return Err(PortalSessionFailure::TransportFailed);
        }
        Ok(None)
    }

    fn submit_text(
        &mut self,
        payload: PortalEisTextPayload,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> PortalEisTextSubmission {
        let Some(sender) = self.eis_sender.take() else {
            return PortalEisTextSubmission::RejectedBeforeDispatch(
                PortalEisTextFailure::SenderUnavailable,
            );
        };
        let (sender, outcome) = sender.submit(payload, deadline, cancellation);
        self.eis_sender = sender;
        match outcome {
            PortalEisTextSubmission::RejectedBeforeDispatch(
                PortalEisTextFailure::Revoked | PortalEisTextFailure::TransportFailed,
            ) if self.eis_sender.is_none() => PortalEisTextSubmission::RejectedBeforeDispatch(
                PortalEisTextFailure::TransportFailed,
            ),
            outcome => outcome,
        }
    }

    fn abort_local(&mut self) {
        self.eis_sender = None;
        self.session_closed = None;
        self.session_path = None;
        self.pending_request = None;
        self.connection = None;
        self.session_may_exist.store(false, Ordering::Release);
    }
}

struct PortalRequestControl<'a> {
    deadline: Instant,
    cancellation: &'a dyn PortalCancellation,
    authority_may_exist: Option<Arc<AtomicBool>>,
}

struct PortalOptions(HashMap<String, OwnedValue>);

impl PortalOptions {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn string(&mut self, key: &str, value: &str) {
        self.0
            .insert(key.to_owned(), OwnedValue::from(Str::from(value)));
    }

    fn u32(&mut self, key: &str, value: u32) {
        self.0.insert(key.to_owned(), OwnedValue::from(value));
    }

    fn into_map(self) -> HashMap<String, OwnedValue> {
        self.0
    }
}

async fn build_proxy(
    connection: zbus::Connection,
    path: OwnedObjectPath,
    interface: &'static str,
) -> Result<zbus::Proxy<'static>, SystemCallFailure> {
    zbus::proxy::Builder::new(&connection)
        .destination(DESKTOP_DESTINATION)
        .map_err(|_| SystemCallFailure::Malformed)?
        .path(path)
        .map_err(|_| SystemCallFailure::Malformed)?
        .interface(interface)
        .map_err(|_| SystemCallFailure::Malformed)?
        .build()
        .await
        .map_err(|_| SystemCallFailure::Bus)
}

async fn call_portal_request<B>(
    connection: zbus::Connection,
    request_path: OwnedObjectPath,
    method: &'static str,
    body: B,
    request_may_exist: Arc<AtomicBool>,
    authority_may_exist: Option<Arc<AtomicBool>>,
) -> Result<PortalResults, SystemCallFailure>
where
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType + Sync,
{
    let request_proxy =
        build_proxy(connection.clone(), request_path.clone(), REQUEST_INTERFACE).await?;
    let mut responses = request_proxy
        .receive_signal("Response")
        .await
        .map_err(|_| SystemCallFailure::Bus)?;
    let portal_path =
        OwnedObjectPath::try_from(DESKTOP_PATH).map_err(|_| SystemCallFailure::Malformed)?;
    let portal = build_proxy(connection, portal_path, REMOTE_DESKTOP_INTERFACE).await?;
    // From this point onward the method call may have reached the portal. A
    // cancelling owner must close the predictable Request path before it can
    // report bounded cleanup.
    request_may_exist.store(true, Ordering::Release);
    if let Some(authority) = authority_may_exist.as_ref() {
        authority.store(true, Ordering::Release);
    }
    let returned: OwnedObjectPath = portal
        .call(method, &body)
        .await
        .map_err(|_| SystemCallFailure::Bus)?;
    validate_request_path(&request_path, returned)?;
    let response = responses.next().await.ok_or(SystemCallFailure::Bus)?;
    let (code, results): (u32, PortalResults) = response
        .body()
        .deserialize()
        .map_err(|_| SystemCallFailure::Malformed)?;
    match code {
        0 => Ok(results),
        1 => {
            if let Some(authority) = authority_may_exist {
                authority.store(false, Ordering::Release);
            }
            Err(SystemCallFailure::PortalCancelled)
        }
        _ => {
            if let Some(authority) = authority_may_exist {
                authority.store(false, Ordering::Release);
            }
            Err(SystemCallFailure::PortalFailed)
        }
    }
}

async fn close_object(
    connection: zbus::Connection,
    path: OwnedObjectPath,
    interface: &'static str,
) -> Result<(), SystemCallFailure> {
    let result = build_proxy(connection, path, interface)
        .await?
        .call::<_, _, ()>("Close", &())
        .await;
    result.map_err(|_| SystemCallFailure::Bus)
}

fn wait_cancellable<T>(
    runtime: &Runtime,
    deadline: Instant,
    cancellation: &dyn PortalCancellation,
    mut future: Pin<Box<dyn Future<Output = Result<T, SystemCallFailure>> + Send + '_>>,
) -> Result<T, SystemCallFailure> {
    loop {
        if cancellation.is_cancelled() {
            return Err(SystemCallFailure::LocalCancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(SystemCallFailure::TimedOut);
        }
        let slice = remaining.min(POLL_SLICE);
        match runtime.block_on(tokio::time::timeout(slice, future.as_mut())) {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}

fn wait_cancellable_with_revocation<T>(
    runtime: &Runtime,
    deadline: Instant,
    cancellation: &dyn PortalCancellation,
    mut future: Pin<Box<dyn Future<Output = Result<T, SystemCallFailure>> + Send + '_>>,
    mut session_closed: Option<&mut SignalStream<'static>>,
) -> Result<T, SystemCallFailure> {
    wait_cancellable_with_event(runtime, deadline, cancellation, future.as_mut(), || {
        let Some(stream) = session_closed.as_deref_mut() else {
            return Ok(false);
        };
        match stream.next().now_or_never() {
            Some(Some(_)) => Ok(true),
            Some(None) => Err(SystemCallFailure::Bus),
            None => Ok(false),
        }
    })
}

fn wait_cancellable_with_event<T>(
    runtime: &Runtime,
    deadline: Instant,
    cancellation: &dyn PortalCancellation,
    mut future: Pin<&mut (dyn Future<Output = Result<T, SystemCallFailure>> + Send + '_)>,
    mut event_ready: impl FnMut() -> Result<bool, SystemCallFailure>,
) -> Result<T, SystemCallFailure> {
    loop {
        if event_ready()? {
            return Err(SystemCallFailure::Revoked);
        }
        if cancellation.is_cancelled() {
            return Err(SystemCallFailure::LocalCancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(SystemCallFailure::TimedOut);
        }
        let slice = remaining.min(POLL_SLICE);
        match runtime.block_on(tokio::time::timeout(slice, future.as_mut())) {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}

fn wait_uncancellable<T>(
    runtime: &Runtime,
    deadline: Instant,
    future: Pin<Box<dyn Future<Output = Result<T, SystemCallFailure>> + Send + '_>>,
) -> Result<T, SystemCallFailure> {
    struct NeverCancelled;
    impl PortalCancellation for NeverCancelled {
        fn is_cancelled(&self) -> bool {
            false
        }
    }
    wait_cancellable(runtime, deadline, &NeverCancelled, future)
}

fn operation_deadline(deadline: Instant) -> Instant {
    deadline
        .checked_sub(CLEANUP_RESERVE)
        .unwrap_or_else(Instant::now)
}

fn next_token(prefix: &str) -> String {
    let mut rng = rand::rng();
    let opaque = Alphanumeric.sample_string(&mut rng, 24);
    format!("narrivy_{prefix}_{opaque}")
}

fn predicted_path(
    connection: &zbus::Connection,
    prefix: &str,
    token: &str,
) -> Result<OwnedObjectPath, PortalSessionFailure> {
    let unique = connection
        .unique_name()
        .ok_or(PortalSessionFailure::PortalUnavailable)?;
    let sender = unique.as_str().trim_start_matches(':').replace('.', "_");
    OwnedObjectPath::try_from(format!("{prefix}/{sender}/{token}"))
        .map_err(|_| PortalSessionFailure::RequestFailed)
}

fn session_handle(results: &PortalResults) -> Option<OwnedObjectPath> {
    let value = results.get("session_handle")?;
    if let Ok(path) = <&zbus::zvariant::ObjectPath<'_>>::try_from(value) {
        return OwnedObjectPath::try_from(path.as_str()).ok();
    }
    let path = <&str>::try_from(value).ok()?;
    OwnedObjectPath::try_from(path).ok()
}

fn selected_devices(results: &PortalResults) -> Option<u32> {
    u32::try_from(results.get("devices")?).ok()
}

fn validate_request_path(
    predicted: &OwnedObjectPath,
    returned: OwnedObjectPath,
) -> Result<(), SystemCallFailure> {
    if &returned == predicted {
        Ok(())
    } else {
        Err(SystemCallFailure::PathMismatch(returned))
    }
}

fn map_setup_failure(failure: SystemCallFailure) -> PortalSessionFailure {
    match failure {
        SystemCallFailure::LocalCancelled => PortalSessionFailure::Cancelled,
        SystemCallFailure::TimedOut => PortalSessionFailure::TimedOut,
        _ => PortalSessionFailure::PortalUnavailable,
    }
}

fn map_request_failure(failure: SystemCallFailure) -> PortalSessionFailure {
    match failure {
        SystemCallFailure::LocalCancelled => PortalSessionFailure::Cancelled,
        SystemCallFailure::TimedOut => PortalSessionFailure::TimedOut,
        SystemCallFailure::PortalCancelled => PortalSessionFailure::PermissionDenied,
        SystemCallFailure::PortalFailed => PortalSessionFailure::RequestFailed,
        SystemCallFailure::Revoked => PortalSessionFailure::Revoked,
        SystemCallFailure::Bus
        | SystemCallFailure::Malformed
        | SystemCallFailure::PathMismatch(_) => PortalSessionFailure::RequestFailed,
    }
}

fn map_transport_failure(failure: SystemCallFailure) -> PortalSessionFailure {
    match failure {
        SystemCallFailure::LocalCancelled => PortalSessionFailure::Cancelled,
        SystemCallFailure::TimedOut => PortalSessionFailure::TimedOut,
        SystemCallFailure::Revoked => PortalSessionFailure::Revoked,
        _ => PortalSessionFailure::TransportFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use narrivy_types::SessionCancellation;

    fn test_io<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, std::io::Error> {
        result.map_err(|error| std::io::Error::other(format!("{error:?}")))
    }

    #[test]
    fn construction_is_inert_and_owns_no_external_capability()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut driver = test_io(DirectZbusPortalDriver::new())?;
        assert!(driver.connection.is_none());
        assert!(driver.pending_request.is_none());
        assert!(driver.session_path.is_none());
        assert!(driver.session_closed.is_none());
        assert!(driver.eis_sender.is_none());
        assert_eq!(
            driver.submit_text(
                test_io(crate::portal_eis::PortalEisTextPayload::parse("controlled",))?,
                Instant::now() + Duration::from_secs(1),
                &narrivy_types::SessionCancellation::new(),
            ),
            crate::portal_eis::PortalEisTextSubmission::RejectedBeforeDispatch(
                crate::portal_eis::PortalEisTextFailure::SenderUnavailable,
            )
        );
        Ok(())
    }

    #[test]
    fn option_maps_are_keyboard_only_nonpersistent_and_token_bounded()
    -> Result<(), Box<dyn std::error::Error>> {
        let tokens = (0..64)
            .map(|_| next_token("test"))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(tokens.len(), 64);
        let token = tokens
            .into_iter()
            .next()
            .ok_or_else(|| std::io::Error::other("token set must be nonempty"))?;
        assert!(token.len() >= "narrivy_test_".len() + 24);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        );

        let mut options = PortalOptions::new();
        options.string("handle_token", &token);
        options.u32("types", KEYBOARD_DEVICE);
        options.u32("persist_mode", NO_PERSISTENCE);
        let options = options.into_map();
        assert_eq!(u32::try_from(&options["types"]), Ok(KEYBOARD_DEVICE));
        assert_eq!(u32::try_from(&options["persist_mode"]), Ok(NO_PERSISTENCE));
        assert!(!options.contains_key("restore_token"));
        assert!(!options.contains_key("clipboard_enabled"));
        Ok(())
    }

    #[test]
    fn method_bodies_match_the_introspected_portal_signatures()
    -> Result<(), Box<dyn std::error::Error>> {
        use zbus::zvariant::DynamicType;

        let session =
            OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/narrivy/test")?;
        let mut options = PortalOptions::new();
        options.string("handle_token", "narrivy_test");
        let options = options.into_map();
        assert_eq!(options.signature().to_string(), "a{sv}");
        assert_eq!(
            (session.clone(), options.clone()).signature().to_string(),
            "(oa{sv})"
        );
        assert_eq!(
            (session.clone(), "wayland:test".to_owned(), options.clone())
                .signature()
                .to_string(),
            "(osa{sv})"
        );
        assert_eq!(
            (session, HashMap::<String, OwnedValue>::new())
                .signature()
                .to_string(),
            "(oa{sv})"
        );
        Ok(())
    }

    #[test]
    fn create_response_accepts_only_valid_string_or_object_session_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let expected = "/org/freedesktop/portal/desktop/session/narrivy/test";
        let mut results = PortalResults::new();
        results.insert(
            "session_handle".to_owned(),
            OwnedValue::from(Str::from(expected)),
        );
        assert_eq!(
            session_handle(&results)
                .as_deref()
                .map(|path| path.as_str()),
            Some(expected)
        );

        let path = zbus::zvariant::ObjectPath::try_from(expected)?;
        results.insert("session_handle".to_owned(), OwnedValue::from(path));
        assert_eq!(
            session_handle(&results)
                .as_deref()
                .map(|path| path.as_str()),
            Some(expected)
        );

        results.insert(
            "session_handle".to_owned(),
            OwnedValue::from(Str::from("not-an-object-path")),
        );
        assert_eq!(session_handle(&results), None);
        Ok(())
    }

    #[test]
    fn mismatched_authoritative_paths_are_retained_for_cleanup()
    -> Result<(), Box<dyn std::error::Error>> {
        let predicted =
            OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/narrivy/predicted")?;
        let actual =
            OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/narrivy/actual")?;
        assert_eq!(
            validate_request_path(&predicted, actual.clone()),
            Err(SystemCallFailure::PathMismatch(actual.clone()))
        );

        let mut driver = test_io(DirectZbusPortalDriver::new())?;
        driver.session_path = Some(predicted.clone());
        let mut results = PortalResults::new();
        results.insert(
            "session_handle".to_owned(),
            OwnedValue::from(zbus::zvariant::ObjectPath::from(actual.clone())),
        );
        assert_eq!(
            driver.accept_session_handle(&predicted, &results),
            Err(PortalSessionFailure::RequestFailed)
        );
        assert_eq!(driver.session_path, Some(actual));
        Ok(())
    }

    #[test]
    fn selected_devices_requires_the_exact_keyboard_only_grant() {
        let mut results = PortalResults::new();
        results.insert("devices".to_owned(), OwnedValue::from(KEYBOARD_DEVICE));
        assert_eq!(selected_devices(&results), Some(KEYBOARD_DEVICE));
        results.insert("devices".to_owned(), OwnedValue::from(3_u32));
        assert_ne!(selected_devices(&results), Some(KEYBOARD_DEVICE));
        results.remove("devices");
        assert_eq!(selected_devices(&results), None);
    }

    #[test]
    fn cleanup_reserve_never_extends_the_callers_absolute_deadline() {
        let deadline = Instant::now() + Duration::from_secs(1);
        let operation = operation_deadline(deadline);
        assert!(operation <= deadline);
        assert!(deadline.duration_since(operation) >= CLEANUP_RESERVE);
    }

    #[test]
    fn local_cancellation_and_deadline_remain_distinct_from_portal_denial()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let cancellation = SessionCancellation::new();
        cancellation.cancel();
        let cancelled: Result<(), _> = wait_cancellable(
            &runtime,
            Instant::now() + Duration::from_secs(1),
            &cancellation,
            Box::pin(std::future::pending()),
        );
        assert_eq!(cancelled, Err(SystemCallFailure::LocalCancelled));
        assert_eq!(
            map_request_failure(SystemCallFailure::LocalCancelled),
            PortalSessionFailure::Cancelled
        );

        let timed_out: Result<(), _> = wait_cancellable(
            &runtime,
            Instant::now(),
            &SessionCancellation::new(),
            Box::pin(std::future::pending()),
        );
        assert_eq!(timed_out, Err(SystemCallFailure::TimedOut));
        assert_eq!(
            map_request_failure(SystemCallFailure::TimedOut),
            PortalSessionFailure::TimedOut
        );
        assert_eq!(
            map_request_failure(SystemCallFailure::PortalCancelled),
            PortalSessionFailure::PermissionDenied
        );
        assert_eq!(
            map_transport_failure(SystemCallFailure::LocalCancelled),
            PortalSessionFailure::Cancelled
        );
        assert_eq!(
            map_transport_failure(SystemCallFailure::TimedOut),
            PortalSessionFailure::TimedOut
        );
        Ok(())
    }

    #[test]
    fn observed_session_closed_preempts_a_pending_driver_operation()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let mut pending = Box::pin(std::future::pending::<Result<(), SystemCallFailure>>());
        let mut polls = 0_u32;
        let result = wait_cancellable_with_event(
            &runtime,
            Instant::now() + Duration::from_secs(1),
            &SessionCancellation::new(),
            pending.as_mut(),
            || {
                polls += 1;
                Ok(true)
            },
        );
        assert_eq!(result, Err(SystemCallFailure::Revoked));
        assert_eq!(polls, 1);
        assert_eq!(
            map_transport_failure(SystemCallFailure::Revoked),
            PortalSessionFailure::Revoked
        );
        Ok(())
    }
}
