//! Offline tests for the trigger abstraction.
//!
//! Contract: no D-Bus, no X server, no portal process and no helper process is
//! touched. Portal probing is exercised through the injected raw-query seam
//! (per the `probe_*_with` design), X11 probing through injected `DISPLAY`
//! values, and routing through fake sources.

use std::cell::RefCell;
use std::rc::Rc;

use super::{
    GLOBALSHORTCUTS_INTERFACE, GLOBALSHORTCUTS_VERSION_PROPERTY, PORTAL_BUS_NAME,
    PORTAL_OBJECT_PATH, PortalInterfaceProbe, PortalInterfaceQuery, SyntheticPhase,
    SyntheticTrigger, TriggerError, TriggerKind, TriggerRoute, TriggerRouter, TriggerRouterConfig,
    TriggerSource, probe_portal_interface_with, probe_x11_hotkey_fallback_with, route_availability,
};

// ---------------------------------------------------------------------------
// Portal probe
// ---------------------------------------------------------------------------

#[test]
fn portal_object_path_is_the_canonical_lowercase_path() {
    // The capitalized spelling belongs only to the bus name; probing the
    // wrong path reports a missing interface on every host.
    assert_eq!(PORTAL_OBJECT_PATH, "/org/freedesktop/portal/desktop");
    assert!(!PORTAL_OBJECT_PATH.contains("/Desktop"));
    assert_eq!(PORTAL_BUS_NAME, "org.freedesktop.portal.Desktop");
    assert_eq!(
        GLOBALSHORTCUTS_INTERFACE,
        "org.freedesktop.portal.GlobalShortcuts"
    );
    assert_eq!(GLOBALSHORTCUTS_VERSION_PROPERTY, "version");
}

#[test]
fn portal_probe_reports_present_only_when_the_exact_interface_answers() {
    let present = probe_portal_interface_with(|| PortalInterfaceQuery::Present { version: 1 });
    assert!(present.is_present());
    assert_eq!(present, PortalInterfaceProbe::Present { version: 1 });
    let availability = present.route_availability();
    assert!(availability.is_available());
    assert!(
        availability
            .available_note()
            .unwrap_or_default()
            .contains(GLOBALSHORTCUTS_INTERFACE)
    );
}

#[test]
fn portal_probe_truthfully_reports_absent_interface() {
    // P0F outcome: portal reachable, GlobalShortcuts interface NOT found.
    let absent = probe_portal_interface_with(|| PortalInterfaceQuery::Missing);
    assert_eq!(absent, PortalInterfaceProbe::Absent);
    assert!(!absent.is_present());
    let availability = absent.route_availability();
    assert!(!availability.is_available());
    let reason = availability.unavailable_reason().unwrap_or_default();
    assert!(
        reason.contains("absent"),
        "reason should say absent: {reason}"
    );
    assert!(
        reason.contains(PORTAL_OBJECT_PATH),
        "reason names the canonical object path: {reason}"
    );
}

#[test]
fn portal_probe_distinguishes_runtime_failure_from_absent_interface() {
    let unavailable = probe_portal_interface_with(|| PortalInterfaceQuery::Unavailable);
    assert_eq!(unavailable, PortalInterfaceProbe::RuntimeUnavailable);
    assert!(!unavailable.is_present());
    assert!(
        unavailable.detail().contains("session bus unavailable"),
        "runtime failure must not be reported as an absent interface: {}",
        unavailable.detail()
    );
    assert!(!unavailable.route_availability().is_available());
}

#[test]
fn portal_probe_times_out_boundedly() {
    let timed_out = probe_portal_interface_with(|| PortalInterfaceQuery::TimedOut);
    assert_eq!(timed_out, PortalInterfaceProbe::TimedOut);
    let reason = timed_out.route_availability();
    assert!(!reason.is_available());
    assert!(
        reason
            .unavailable_reason()
            .unwrap_or_default()
            .contains("timed out")
    );
    // The bounded wait is a small constant, never unbounded.
    assert!(super::PROBE_TIMEOUT <= std::time::Duration::from_secs(1));
}

#[test]
fn portal_probe_source_uses_the_direct_boundary_without_spawning_a_helper() {
    // Structural regression: the production probe path must not resolve a
    // PATH executable (`busctl`) or spawn any subprocess.
    let source = include_str!("../trigger.rs");
    assert!(
        !source.contains("busctl"),
        "the portal probe must not shell out to busctl"
    );
    assert!(
        !source.contains("std::process::Command"),
        "the portal probe must not spawn a helper process"
    );
    assert!(
        source.contains("zbus::Connection::session()"),
        "the portal probe must use the crate's direct zbus boundary"
    );
    assert!(
        source.contains("PORTAL_OBJECT_PATH"),
        "the direct boundary must target the canonical portal object path"
    );
    assert!(
        source.contains("GLOBALSHORTCUTS_VERSION_PROPERTY"),
        "the direct boundary must query the exact interface/property"
    );
}

#[test]
fn route_availability_for_gated_synthetic_route_is_unchanged() {
    // `route_availability` for the synthetic route stays available by design
    // (gating happens in the router); the real portal route is probed.
    assert!(route_availability(TriggerRoute::SyntheticTest).is_available());
}

#[test]
fn portal_error_names_classify_missing_versus_runtime_failure() {
    // Measured on the P0F COSMIC host: the portal answers a missing
    // GlobalShortcuts interface with `InvalidArgs` and the explanation
    // `No such interface "org.freedesktop.portal.GlobalShortcuts"`. That is
    // an absent interface, not a broken bus.
    use super::classify_portal_error_name;
    assert_eq!(
        classify_portal_error_name("org.freedesktop.DBus.Error.InvalidArgs"),
        PortalInterfaceQuery::Missing
    );
    assert_eq!(
        classify_portal_error_name("org.freedesktop.DBus.Error.UnknownInterface"),
        PortalInterfaceQuery::Missing
    );
    assert_eq!(
        classify_portal_error_name("org.freedesktop.DBus.Error.UnknownProperty"),
        PortalInterfaceQuery::Missing
    );
    for runtime_failure in [
        "org.freedesktop.DBus.Error.ServiceUnknown",
        "org.freedesktop.DBus.Error.NoReply",
        "org.freedesktop.DBus.Error.AccessDenied",
        "org.freedesktop.DBus.Error.Failed",
    ] {
        assert_eq!(
            classify_portal_error_name(runtime_failure),
            PortalInterfaceQuery::Unavailable,
            "{runtime_failure} must stay a runtime failure"
        );
    }
}

#[test]
fn production_probe_is_typed_and_bounded_in_this_environment() {
    // Exercise the real direct-zbus boundary (no scripted query). Whatever
    // this environment exposes, the probe must return one typed outcome
    // inside a bound rather than hanging or panicking; a live portal with the
    // interface would truthfully report `Present`.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let outcome = super::probe_portal_interface();
        let _ = tx.send(outcome);
    });
    let received = rx.recv_timeout(std::time::Duration::from_secs(5));
    assert!(
        received.is_ok(),
        "the bounded portal probe must return within the test window"
    );
    let Ok(outcome) = received else {
        return;
    };
    match outcome {
        PortalInterfaceProbe::Present { version } => assert!(version >= 1),
        PortalInterfaceProbe::Absent
        | PortalInterfaceProbe::RuntimeUnavailable
        | PortalInterfaceProbe::TimedOut => {}
    }
}

// ---------------------------------------------------------------------------
// X11 fallback probe
// ---------------------------------------------------------------------------

#[test]
fn x11_probe_available_when_display_set() {
    let avail = probe_x11_hotkey_fallback_with(Some(":0"));
    assert!(avail.is_available());
    let note = avail.available_note().map(str::to_owned);
    assert!(
        note.as_deref()
            .is_some_and(|note| note.starts_with("XWayland lane")),
        "note should carry the lane label, got: {note:?}"
    );
    assert!(
        note.as_deref()
            .is_some_and(|note| note.contains("DISPLAY=:0"))
    );
}

#[test]
fn x11_probe_unavailable_when_display_missing_or_blank() {
    for display in [None, Some(""), Some("   ")] {
        let avail = probe_x11_hotkey_fallback_with(display);
        assert!(
            !avail.is_available(),
            "display {display:?} must be unavailable"
        );
        assert!(avail.unavailable_reason().is_some());
    }
}

// ---------------------------------------------------------------------------
// Synthetic route + route_availability dispatch
// ---------------------------------------------------------------------------

#[test]
fn synthetic_route_always_reports_available() {
    let avail = route_availability(TriggerRoute::SyntheticTest);
    assert!(avail.is_available());
    let note = avail.available_note();
    assert!(
        note.is_some_and(|note| note.contains("test-only")),
        "got: {note:?}"
    );
    assert!(
        note.is_some_and(|note| note.contains("never exposed via IPC/UI")),
        "got: {note:?}"
    );
}

// ---------------------------------------------------------------------------
// Synthetic trigger semantics
// ---------------------------------------------------------------------------

fn press_then_release_ok(trigger: &mut SyntheticTrigger, handle: &super::SyntheticTriggerHandle) {
    assert!(handle.press_tx.send(()).is_ok());
    assert!(trigger.press().is_ok());
    assert!(handle.release_tx.send(()).is_ok());
    assert!(trigger.release().is_ok());
}

#[test]
fn synthetic_trigger_reports_kind_route_and_availability() {
    let (_handle, trigger) = SyntheticTrigger::channel_pair(TriggerKind::ToggleHotkey);
    assert_eq!(trigger.kind(), TriggerKind::ToggleHotkey);
    assert_eq!(trigger.route(), TriggerRoute::SyntheticTest);
    assert!(trigger.availability().is_available());
    // Availability must not require any external resource.
    assert!(trigger.availability().is_available());
}

#[test]
fn synthetic_press_without_queued_token_fails() {
    let (_handle, mut trigger) = SyntheticTrigger::channel_pair(TriggerKind::HoldHotkey);
    let result = trigger.press();
    assert!(
        matches!(&result, Err(TriggerError::Backend(msg)) if msg.contains("press")),
        "expected Backend with press reason, got {result:?}"
    );
    assert!(!trigger.is_pressed());
}

#[test]
fn synthetic_release_without_press_fails() {
    let (handle, mut trigger) = SyntheticTrigger::channel_pair(TriggerKind::ToggleHotkey);
    assert!(handle.release_tx.send(()).is_ok());
    let result = trigger.release();
    assert!(
        matches!(&result, Err(TriggerError::Backend(msg)) if msg.contains("without active press")),
        "expected Backend without-active-press, got {result:?}"
    );
    // State-first check must not consume the queued release token.
    assert!(handle.press_tx.send(()).is_ok());
    assert!(trigger.press().is_ok());
    assert!(trigger.release().is_ok());
    assert_eq!(
        trigger.recorded(),
        &[SyntheticPhase::Press, SyntheticPhase::Release]
    );
}

#[test]
fn synthetic_double_press_fails_and_state_stays_consistent() {
    let (handle, mut trigger) = SyntheticTrigger::channel_pair(TriggerKind::ToggleHotkey);
    assert!(handle.press_tx.send(()).is_ok());
    assert!(handle.press_tx.send(()).is_ok()); // queued extra press token
    assert!(trigger.press().is_ok());
    let result = trigger.press();
    assert!(
        matches!(&result, Err(TriggerError::Backend(msg)) if msg.contains("already pressed")),
        "expected Backend already-pressed, got {result:?}"
    );
    assert!(handle.release_tx.send(()).is_ok());
    assert!(trigger.release().is_ok());
    assert!(!trigger.is_pressed());
    assert_eq!(
        trigger.recorded(),
        &[SyntheticPhase::Press, SyntheticPhase::Release]
    );
}

#[test]
fn synthetic_full_cycle_is_deterministic_and_ordered() {
    let (handle, mut trigger) = SyntheticTrigger::channel_pair(TriggerKind::HoldHotkey);
    // Queue several cycles up-front; consumption stays in send order.
    for _ in 0..3 {
        press_then_release_ok(&mut trigger, &handle);
    }
    assert_eq!(trigger.recorded().len(), 6);
    let expected: Vec<SyntheticPhase> = (0..3)
        .flat_map(|_| [SyntheticPhase::Press, SyntheticPhase::Release])
        .collect();
    assert_eq!(trigger.recorded(), expected.as_slice());
}

#[test]
fn synthetic_closed_channel_maps_to_backend_error() {
    let (handle, mut trigger) = SyntheticTrigger::channel_pair(TriggerKind::ToggleHotkey);
    drop(handle);
    let result = trigger.press();
    assert!(
        matches!(&result, Err(TriggerError::Backend(msg)) if msg.contains("closed")),
        "expected Backend closed-channel, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Router: fake sources, selection matrix, gating, dispatch
// ---------------------------------------------------------------------------

type SharedLog = Rc<RefCell<Vec<String>>>;

struct FakeSource {
    kind: TriggerKind,
    route: TriggerRoute,
    avail: super::RouteAvailability,
    fail_press: Option<String>,
    fail_release: Option<String>,
    name: &'static str,
    log: SharedLog,
}

impl FakeSource {
    fn available(
        name: &'static str,
        kind: TriggerKind,
        route: TriggerRoute,
        log: &SharedLog,
    ) -> Box<FakeSource> {
        Box::new(FakeSource {
            kind,
            route,
            avail: super::RouteAvailability::Available {
                note: format!("{name} ok"),
            },
            fail_press: None,
            fail_release: None,
            name,
            log: Rc::clone(log),
        })
    }

    fn unavailable(
        name: &'static str,
        kind: TriggerKind,
        route: TriggerRoute,
        reason: &str,
        log: &SharedLog,
    ) -> Box<FakeSource> {
        Box::new(FakeSource {
            kind,
            route,
            avail: super::RouteAvailability::Unavailable {
                reason: reason.to_string(),
            },
            fail_press: None,
            fail_release: None,
            name,
            log: Rc::clone(log),
        })
    }
}

impl TriggerSource for FakeSource {
    fn kind(&self) -> TriggerKind {
        self.kind
    }

    fn route(&self) -> TriggerRoute {
        self.route
    }

    fn availability(&self) -> super::RouteAvailability {
        self.avail.clone()
    }

    fn press(&mut self) -> Result<(), TriggerError> {
        self.log.borrow_mut().push(format!("{}:press", self.name));
        match &self.fail_press {
            Some(reason) => Err(TriggerError::Backend(reason.clone())),
            None => Ok(()),
        }
    }

    fn release(&mut self) -> Result<(), TriggerError> {
        self.log.borrow_mut().push(format!("{}:release", self.name));
        match &self.fail_release {
            Some(reason) => Err(TriggerError::Backend(reason.clone())),
            None => Ok(()),
        }
    }
}

/// Standard matrix: portal down (P0F reality), XWayland up, synthetic
/// registered last.
fn p0f_style_router(log: &SharedLog) -> TriggerRouter {
    let mut router = TriggerRouter::new(TriggerRouterConfig::default());
    router.register(FakeSource::unavailable(
        "portal",
        TriggerKind::ToggleHotkey,
        TriggerRoute::PortalGlobalShortcuts,
        "GlobalShortcuts absent from portal introspection",
        log,
    ));
    router.register(FakeSource::available(
        "x11",
        TriggerKind::ToggleHotkey,
        TriggerRoute::X11HotkeyFallback,
        log,
    ));
    router.register(FakeSource::available(
        "synthetic",
        TriggerKind::ToggleHotkey,
        TriggerRoute::SyntheticTest,
        log,
    ));
    router
}

#[test]
fn router_portal_unavailable_falls_to_x11() {
    let log = SharedLog::default();
    let router = p0f_style_router(&log);
    assert_eq!(
        router.select_route(TriggerKind::ToggleHotkey, None),
        Some(TriggerRoute::X11HotkeyFallback)
    );
}

#[test]
fn router_synthetic_gated_by_default_even_when_only_option() {
    let log = SharedLog::default();
    let mut router = TriggerRouter::new(TriggerRouterConfig::default());
    router.register(FakeSource::unavailable(
        "portal",
        TriggerKind::ToggleHotkey,
        TriggerRoute::PortalGlobalShortcuts,
        "absent",
        &log,
    ));
    router.register(FakeSource::unavailable(
        "x11",
        TriggerKind::ToggleHotkey,
        TriggerRoute::X11HotkeyFallback,
        "DISPLAY unset",
        &log,
    ));
    router.register(FakeSource::available(
        "synthetic",
        TriggerKind::ToggleHotkey,
        TriggerRoute::SyntheticTest,
        &log,
    ));
    // Default config mirrors the uinput rule: synthetic never default.
    assert!(!router.config().allow_synthetic);
    assert_eq!(router.select_route(TriggerKind::ToggleHotkey, None), None);
    assert_eq!(
        router.press(TriggerKind::ToggleHotkey, None),
        Err(TriggerError::NoRoute {
            kind: TriggerKind::ToggleHotkey
        })
    );
}

#[test]
fn router_synthetic_selected_only_when_explicitly_allowed() {
    let log = SharedLog::default();
    let mut router = TriggerRouter::new(TriggerRouterConfig {
        allow_synthetic: true,
    });
    router.register(FakeSource::unavailable(
        "portal",
        TriggerKind::ToggleHotkey,
        TriggerRoute::PortalGlobalShortcuts,
        "absent",
        &log,
    ));
    router.register(FakeSource::unavailable(
        "x11",
        TriggerKind::ToggleHotkey,
        TriggerRoute::X11HotkeyFallback,
        "DISPLAY unset",
        &log,
    ));
    router.register(FakeSource::available(
        "synthetic",
        TriggerKind::ToggleHotkey,
        TriggerRoute::SyntheticTest,
        &log,
    ));
    assert_eq!(
        router.select_route(TriggerKind::ToggleHotkey, None),
        Some(TriggerRoute::SyntheticTest)
    );
}

#[test]
fn router_prefers_requested_route_when_available_and_allowed() {
    let log = SharedLog::default();
    let mut router = p0f_style_router(&log);
    router.register(FakeSource::available(
        "portal2",
        TriggerKind::ToggleHotkey,
        TriggerRoute::PortalGlobalShortcuts,
        &log,
    ));
    // Preference overrides registration order.
    assert_eq!(
        router.select_route(
            TriggerKind::ToggleHotkey,
            Some(TriggerRoute::PortalGlobalShortcuts)
        ),
        Some(TriggerRoute::PortalGlobalShortcuts)
    );
    // ...but an unavailable preferred route falls back to the matrix order.
    assert_eq!(
        router.select_route(
            TriggerKind::ToggleHotkey,
            Some(TriggerRoute::X11HotkeyFallback)
        ),
        Some(TriggerRoute::X11HotkeyFallback)
    );
}

#[test]
fn router_preference_for_unknown_route_is_ignored() {
    let log = SharedLog::default();
    let router = p0f_style_router(&log);
    assert_eq!(
        router.select_route(
            TriggerKind::ToggleHotkey,
            Some(TriggerRoute::PortalGlobalShortcuts)
        ),
        Some(TriggerRoute::X11HotkeyFallback),
        "preferred route has no available source; fall through"
    );
}

#[test]
fn router_preference_for_synthetic_is_gated() {
    let log = SharedLog::default();
    let router = p0f_style_router(&log);
    assert_eq!(
        router.select_route(TriggerKind::ToggleHotkey, Some(TriggerRoute::SyntheticTest)),
        Some(TriggerRoute::X11HotkeyFallback),
        "synthetic preference must not bypass the default gate"
    );
}

#[test]
fn router_kinds_are_isolated() {
    let log = SharedLog::default();
    let mut router = TriggerRouter::new(TriggerRouterConfig::default());
    router.register(FakeSource::available(
        "hold-x11",
        TriggerKind::HoldHotkey,
        TriggerRoute::X11HotkeyFallback,
        &log,
    ));
    assert_eq!(
        router.select_route(TriggerKind::HoldHotkey, None),
        Some(TriggerRoute::X11HotkeyFallback)
    );
    assert_eq!(
        router.select_route(TriggerKind::ToggleHotkey, None),
        None,
        "hold-only source must not satisfy toggle"
    );
}

#[test]
fn router_dispatch_hits_selected_source_in_order() {
    let log = SharedLog::default();
    let mut router = p0f_style_router(&log);
    assert!(router.press(TriggerKind::ToggleHotkey, None).is_ok());
    assert!(router.release(TriggerKind::ToggleHotkey, None).is_ok());
    assert_eq!(
        log.borrow().as_slice(),
        ["x11:press", "x11:release"],
        "portal is unavailable, synthetic gated; only x11 fires"
    );
}

#[test]
fn router_propagates_source_errors() {
    let log = SharedLog::default();
    let mut router = TriggerRouter::new(TriggerRouterConfig::default());
    let mut source = FakeSource::available(
        "x11",
        TriggerKind::ToggleHotkey,
        TriggerRoute::X11HotkeyFallback,
        &log,
    );
    source.fail_press = Some("grab failed".to_string());
    router.register(source);
    let result = router.press(TriggerKind::ToggleHotkey, None);
    assert_eq!(
        result,
        Err(TriggerError::Backend("grab failed".to_string())),
        "source failure must surface typed, not be swallowed"
    );
}

#[test]
fn router_empty_reports_no_route() {
    let mut router = TriggerRouter::new(TriggerRouterConfig::default());
    assert!(router.is_empty());
    assert_eq!(
        router.release(TriggerKind::HoldHotkey, None),
        Err(TriggerError::NoRoute {
            kind: TriggerKind::HoldHotkey
        })
    );
}

#[test]
fn router_synthetic_dispatch_works_when_allowed() {
    let (handle, synthetic) = SyntheticTrigger::channel_pair(TriggerKind::ToggleHotkey);
    let mut router = TriggerRouter::new(TriggerRouterConfig {
        allow_synthetic: true,
    });
    router.register(Box::new(synthetic));
    assert!(handle.press_tx.send(()).is_ok());
    assert!(handle.release_tx.send(()).is_ok());
    assert!(router.press(TriggerKind::ToggleHotkey, None).is_ok());
    assert!(router.release(TriggerKind::ToggleHotkey, None).is_ok());
}

#[test]
fn trigger_error_display_is_stable_and_content_free() {
    let no_route = TriggerError::NoRoute {
        kind: TriggerKind::HoldHotkey,
    };
    assert_eq!(
        no_route.to_string(),
        "no available trigger route for kind hold-hotkey"
    );
    let route_down = TriggerError::RouteUnavailable {
        route: TriggerRoute::PortalGlobalShortcuts,
        reason: "absent".to_string(),
    };
    assert!(
        route_down
            .to_string()
            .contains("portal-global-shortcuts unavailable: absent")
    );
}
