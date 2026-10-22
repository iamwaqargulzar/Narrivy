//! Typed global-trigger abstraction for the Linux platform adapter (P2 prep).
//!
//! P1 lands the contract, truthful route probing, and an offline-tested
//! synthetic source; live portal/X11 listeners are wired in P2 (walking
//! skeleton: `trigger -> microphone -> ASR -> delivery/Inbox` with a
//! `pipeline_id` per activation event).
//!
//! # Evidence baseline (P0F, COSMIC Wayland)
//! `agent/evidence/P0F/p0f-activation-cosmic.json`: xdg-desktop-portal 1.18.4
//! (`cosmic`/`gtk` impls) exposes **no GlobalShortcuts interface**, so
//! toggle-via-portal is unqualified on this host; 50/50 synthetic activation
//! cycles were leak/hang-free; portal restart path exists. Consequences that
//! are encoded here:
//!
//! - [`TriggerRoute::PortalGlobalShortcuts`] is probed by a real, bounded
//!   direct D-Bus query at decision time and truthfully reports
//!   [`RouteAvailability::Unavailable`] on this host class. Never assumed, and
//!   never inferred from the desktop name.
//! - [`TriggerRoute::X11HotkeyFallback`] covers the XWayland lane that *is*
//!   present on the probe host (`DISPLAY` reachable through XWayland).
//! - [`TriggerRoute::SyntheticTest`] always reports available but is gated
//!   out of routing unless explicitly allowed (`TriggerRouterConfig::
//!   allow_synthetic`, mirroring the uinput opt-in least-privilege rule) and
//!   must **never** be exposed via IPC or UI. It exists for deterministic
//!   offline tests only.
//!
//! # Rules honored (AGENTS.md)
//! - Probes are direct, in-process D-Bus queries on the canonical portal
//!   object ([`probe_portal_interface`]); no external helper process is
//!   spawned and the whole query is bounded by one timeout.
//! - Probe failures map to typed outcomes and then to
//!   [`RouteAvailability::Unavailable { reason }`]; there is no panic path in
//!   non-test code.
//! - Reason strings are short OS/bus diagnostics only; no transcript or
//!   user content can flow through this module.
//! - [`TriggerKind`] deliberately mirrors only the two hotkey kinds of
//!   `narrivy_pipeline::TriggerKind` (`HoldHotkey`, `ToggleHotkey`) as a
//!   local enum to avoid coupling this adapter to the pipeline crate for
//!   now; a conversion can be added when P2 wires live sources.

use std::fmt;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Kinds, routes, availability
// ---------------------------------------------------------------------------

/// What semantic activation a trigger source provides.
///
/// Local mirror of the hotkey variants of `narrivy_pipeline::TriggerKind`;
/// kept adapter-local until P2 decides on a shared conversion (see module
/// docs). PTT (`HoldHotkey`) stays capability-dependent per the locked
/// decision: it requires proven key-release semantics on the chosen route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    /// Press-and-hold push-to-talk.
    HoldHotkey,
    /// Toggle start/stop (the default activation mode).
    ToggleHotkey,
}

impl TriggerKind {
    /// Stable short label for diagnostics (never user content).
    pub fn label(self) -> &'static str {
        match self {
            TriggerKind::HoldHotkey => "hold-hotkey",
            TriggerKind::ToggleHotkey => "toggle-hotkey",
        }
    }
}

/// Which OS mechanism a trigger source uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerRoute {
    /// xdg-desktop-portal `org.freedesktop.portal.GlobalShortcuts`
    /// (catalog module `linux-global-shortcut`). Probed, never assumed;
    /// absent on the P0F COSMIC host (portal 1.18.4).
    PortalGlobalShortcuts,
    /// X11/XWayland hotkey lane (catalog module `linux-x11-hotkey`,
    /// status `fallback`); available where `DISPLAY` reaches an X server
    /// (XWayland included). Qualification as a real listener route is P2 work.
    X11HotkeyFallback,
    /// In-process channel-driven fake for tests. Always reports available,
    /// but routers must gate it behind an explicit opt-in flag and IPC/UI
    /// must never offer it.
    SyntheticTest,
}

impl TriggerRoute {
    /// Stable short label for diagnostics (never user content).
    pub fn label(self) -> &'static str {
        match self {
            TriggerRoute::PortalGlobalShortcuts => "portal-global-shortcuts",
            TriggerRoute::X11HotkeyFallback => "x11-hotkey-fallback",
            TriggerRoute::SyntheticTest => "synthetic-test",
        }
    }
}

/// Truthful result of probing one route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteAvailability {
    /// The route can currently be attempted. `note` carries a short,
    /// content-free qualifier (e.g. `"XWayland lane ..."`).
    Available { note: String },
    /// The route cannot be used right now; `reason` is a short stable
    /// diagnostic (OS/tool error text, no transcript or user content).
    Unavailable { reason: String },
}

impl RouteAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, RouteAvailability::Available { .. })
    }

    /// The `reason` if unavailable, else `None`.
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            RouteAvailability::Available { .. } => None,
            RouteAvailability::Unavailable { reason } => Some(reason),
        }
    }

    /// The `note` if available, else `None`.
    pub fn available_note(&self) -> Option<&str> {
        match self {
            RouteAvailability::Available { note } => Some(note),
            RouteAvailability::Unavailable { .. } => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Typed trigger failure. Variants carry short diagnostic strings only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerError {
    /// No allowed + available route exists for this kind.
    NoRoute { kind: TriggerKind },
    /// A selected route refused the press/release at event time.
    RouteUnavailable { route: TriggerRoute, reason: String },
    /// Backend-specific failure (e.g. synthetic channel state).
    Backend(String),
}

impl fmt::Display for TriggerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TriggerError::NoRoute { kind } => {
                write!(f, "no available trigger route for kind {}", kind.label())
            }
            TriggerError::RouteUnavailable { route, reason } => {
                write!(f, "trigger route {} unavailable: {}", route.label(), reason)
            }
            TriggerError::Backend(msg) => write!(f, "trigger backend failure: {msg}"),
        }
    }
}

impl std::error::Error for TriggerError {}

// ---------------------------------------------------------------------------
// Source trait
// ---------------------------------------------------------------------------

/// One trigger input delivering semantic press/release events.
///
/// Real listeners (portal GlobalShortcuts, X11 grab) land in P2; P1 ships
/// this contract plus the synthetic implementation used by offline tests.
pub trait TriggerSource {
    /// Which activation semantics this source provides.
    fn kind(&self) -> TriggerKind;

    /// Which OS mechanism backs this source.
    fn route(&self) -> TriggerRoute;

    /// Current availability of this source's backing route. The default
    /// defers to the static route probe ([`route_availability`]) so simple
    /// sources stay declarative; live P2 sources override with runtime state.
    fn availability(&self) -> RouteAvailability {
        route_availability(self.route())
    }

    /// Begin activation (semantic press).
    fn press(&mut self) -> Result<(), TriggerError>;

    /// End activation (semantic release).
    fn release(&mut self) -> Result<(), TriggerError>;
}

// ---------------------------------------------------------------------------
// Synthetic test source (channel-driven)
// ---------------------------------------------------------------------------

const SYNTHETIC_NOTE: &str = "synthetic test-only source; never exposed via IPC/UI";

/// Recorded phase of a [`SyntheticTrigger`], for deterministic assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntheticPhase {
    Press,
    Release,
}

/// Test-side handle pushing synthetic events into a [`SyntheticTrigger`].
///
/// Each successful [`SyntheticTrigger::press`] consumes one token sent on
/// `press_tx`; each successful [`SyntheticTrigger::release`] consumes one
/// `release_tx` token. Tokens queue deterministically in send order.
#[derive(Debug, Clone)]
pub struct SyntheticTriggerHandle {
    pub press_tx: Sender<()>,
    pub release_tx: Sender<()>,
}

/// Deterministic in-process trigger source for offline tests.
///
/// Semantics (all non-blocking, all deterministic):
/// - `press` succeeds only if a queued press token exists and the source is
///   currently idle;
/// - `release` succeeds only if the source is pressed and a queued release
///   token exists;
/// - every outcome maps to [`TriggerError::Backend`] with a stable reason.
#[derive(Debug)]
pub struct SyntheticTrigger {
    kind: TriggerKind,
    press_rx: Receiver<()>,
    release_rx: Receiver<()>,
    pressed: bool,
    events: Vec<SyntheticPhase>,
}

impl SyntheticTrigger {
    /// Build the `(handle, source)` pair: the handle feeds tokens, the
    /// source consumes them under the documented semantics.
    pub fn channel_pair(kind: TriggerKind) -> (SyntheticTriggerHandle, SyntheticTrigger) {
        let (press_tx, press_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let handle = SyntheticTriggerHandle {
            press_tx,
            release_tx,
        };
        let source = SyntheticTrigger {
            kind,
            press_rx,
            release_rx,
            pressed: false,
            events: Vec::new(),
        };
        (handle, source)
    }

    /// Phases recorded so far, in completion order.
    pub fn recorded(&self) -> &[SyntheticPhase] {
        &self.events
    }

    /// Whether the source currently models an active press.
    pub fn is_pressed(&self) -> bool {
        self.pressed
    }
}

impl TriggerSource for SyntheticTrigger {
    fn kind(&self) -> TriggerKind {
        self.kind
    }

    fn route(&self) -> TriggerRoute {
        TriggerRoute::SyntheticTest
    }

    fn availability(&self) -> RouteAvailability {
        RouteAvailability::Available {
            note: SYNTHETIC_NOTE.to_string(),
        }
    }

    fn press(&mut self) -> Result<(), TriggerError> {
        if self.pressed {
            return Err(TriggerError::Backend("synthetic already pressed".into()));
        }
        match self.press_rx.try_recv() {
            Ok(()) => {
                self.pressed = true;
                self.events.push(SyntheticPhase::Press);
                Ok(())
            }
            Err(TryRecvError::Empty) => Err(TriggerError::Backend(
                "no synthetic press token queued".into(),
            )),
            Err(TryRecvError::Disconnected) => Err(TriggerError::Backend(
                "synthetic press channel closed".into(),
            )),
        }
    }

    fn release(&mut self) -> Result<(), TriggerError> {
        if !self.pressed {
            return Err(TriggerError::Backend(
                "synthetic release without active press".into(),
            ));
        }
        match self.release_rx.try_recv() {
            Ok(()) => {
                self.pressed = false;
                self.events.push(SyntheticPhase::Release);
                Ok(())
            }
            Err(TryRecvError::Empty) => Err(TriggerError::Backend(
                "no synthetic release token queued".into(),
            )),
            Err(TryRecvError::Disconnected) => Err(TriggerError::Backend(
                "synthetic release channel closed".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Route probing (bounded, direct, truthful)
// ---------------------------------------------------------------------------

/// Bounded wait applied by the default portal probe. Generous enough for a
/// local D-Bus round trip, tight enough to never stall routing decisions or
/// daemon status composition.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(750);

pub const PORTAL_BUS_NAME: &str = "org.freedesktop.portal.Desktop";

/// Canonical XDG desktop portal object path.
///
/// The object path segment is lowercase `desktop`; the capitalized spelling
/// belongs only to the well-known bus name ([`PORTAL_BUS_NAME`]). Probing the
/// capitalized path silently reports a missing interface on every host, so
/// this constant is pinned and asserted by tests.
pub const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";

pub const GLOBALSHORTCUTS_INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

/// The `version` property every GlobalShortcuts-capable portal exposes.
/// Querying exactly this interface/property is a presence test: an absent
/// interface answers a missing-interface D-Bus error (`InvalidArgs` on the
/// measured host, `UnknownInterface`/`UnknownProperty` elsewhere), so presence
/// is never inferred from the desktop name or from the portal merely answering.
pub const GLOBALSHORTCUTS_VERSION_PROPERTY: &str = "version";

const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const PROPERTY_GET_METHOD: &str = "Get";

/// Typed, content-free outcome of the direct portal-interface probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalInterfaceProbe {
    /// The exact interface is present and reported this `version`.
    Present { version: u32 },
    /// The portal object answered but does not expose the interface (the
    /// measured P0F COSMIC outcome).
    Absent,
    /// No usable session bus, or a transport-level query failure.
    RuntimeUnavailable,
    /// The single bounded D-Bus round trip did not complete in time.
    TimedOut,
}

impl PortalInterfaceProbe {
    /// True only when the exact interface was actually observed.
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Present { .. })
    }

    /// Short, content-free diagnostic for this outcome.
    pub fn detail(&self) -> String {
        match self {
            Self::Present { version } => {
                format!("{GLOBALSHORTCUTS_INTERFACE} present (version {version})")
            }
            Self::Absent => format!("{GLOBALSHORTCUTS_INTERFACE} absent from {PORTAL_OBJECT_PATH}"),
            Self::RuntimeUnavailable => "session bus unavailable for portal probe".to_owned(),
            Self::TimedOut => {
                format!("portal interface probe timed out after {PROBE_TIMEOUT:?}")
            }
        }
    }

    /// Route availability derived from this one observed fact. Presence of the
    /// interface is a probe result only: it never claims a listener exists.
    pub fn route_availability(&self) -> RouteAvailability {
        if self.is_present() {
            RouteAvailability::Available {
                note: self.detail(),
            }
        } else {
            RouteAvailability::Unavailable {
                reason: self.detail(),
            }
        }
    }
}

/// Raw result of one bounded query, before interpretation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalInterfaceQuery {
    /// The exact interface/property answered with this version.
    Present { version: u32 },
    /// The bus answered but the exact interface/property is absent.
    Missing,
    /// No session bus, or the query failed at the transport level.
    Unavailable,
    /// The bounded query expired.
    TimedOut,
}

/// Probe the canonical portal object with the query injected, so tests stay
/// fully offline and deterministic while production uses one real boundary.
pub fn probe_portal_interface_with(
    query: impl FnOnce() -> PortalInterfaceQuery,
) -> PortalInterfaceProbe {
    match query() {
        PortalInterfaceQuery::Present { version } => PortalInterfaceProbe::Present { version },
        PortalInterfaceQuery::Missing => PortalInterfaceProbe::Absent,
        PortalInterfaceQuery::Unavailable => PortalInterfaceProbe::RuntimeUnavailable,
        PortalInterfaceQuery::TimedOut => PortalInterfaceProbe::TimedOut,
    }
}

/// Truthful direct probe of `org.freedesktop.portal.GlobalShortcuts` over the
/// session bus: one `org.freedesktop.DBus.Properties.Get` round trip against
/// the canonical object path, bounded by one [`PROBE_TIMEOUT`]. No helper
/// process is spawned and no desktop name is consulted.
pub fn probe_portal_interface() -> PortalInterfaceProbe {
    probe_portal_interface_with(direct_portal_interface_query)
}

/// The one real zbus boundary used by [`probe_portal_interface`].
fn direct_portal_interface_query() -> PortalInterfaceQuery {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    else {
        return PortalInterfaceQuery::Unavailable;
    };
    // The timeout future is constructed inside `block_on` so it is created
    // inside the runtime context (timers require an active reactor).
    match runtime.block_on(async {
        tokio::time::timeout(PROBE_TIMEOUT, query_portal_interface_property()).await
    }) {
        Ok(query) => query,
        Err(_) => PortalInterfaceQuery::TimedOut,
    }
}

/// One bounded `Properties.Get` round trip on the canonical portal object.
async fn query_portal_interface_property() -> PortalInterfaceQuery {
    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(_) => return PortalInterfaceQuery::Unavailable,
    };
    let proxy: Result<zbus::Proxy<'static>, zbus::Error> = zbus::Proxy::new(
        &connection,
        PORTAL_BUS_NAME,
        PORTAL_OBJECT_PATH,
        PROPERTIES_INTERFACE,
    )
    .await;
    let Ok(proxy) = proxy else {
        return PortalInterfaceQuery::Unavailable;
    };
    match proxy
        .call_method(
            PROPERTY_GET_METHOD,
            &(GLOBALSHORTCUTS_INTERFACE, GLOBALSHORTCUTS_VERSION_PROPERTY),
        )
        .await
    {
        Ok(reply) => match reply.body().deserialize::<(zbus::zvariant::OwnedValue,)>() {
            Ok((value,)) => u32::try_from(&value)
                .map(|version| PortalInterfaceQuery::Present { version })
                .unwrap_or(PortalInterfaceQuery::Missing),
            Err(_) => PortalInterfaceQuery::Missing,
        },
        Err(error) => classify_portal_call_error(&error),
    }
}

/// Map one D-Bus method error to the closed raw outcome set. Only the errors a
/// portal returns when the requested interface/property does not exist on the
/// canonical object mean "the portal answered but does not expose it";
/// everything else is a runtime/transport failure.
fn classify_portal_call_error(error: &zbus::Error) -> PortalInterfaceQuery {
    match error {
        zbus::Error::MethodError(name, _, _) => classify_portal_error_name(name.as_str()),
        zbus::Error::InterfaceNotFound => PortalInterfaceQuery::Missing,
        _ => PortalInterfaceQuery::Unavailable,
    }
}

/// Closed classification of one D-Bus error name.
///
/// `InvalidArgs` is the observed xdg-desktop-portal answer for a missing
/// interface (`No such interface "org.freedesktop.portal.GlobalShortcuts"`);
/// the unknown-* names are the generic bus spellings. All mean the exact
/// interface/property is not present, and all fail closed to `Missing`.
fn classify_portal_error_name(name: &str) -> PortalInterfaceQuery {
    const MISSING_INTERFACE_ERRORS: [&str; 3] = [
        "org.freedesktop.DBus.Error.InvalidArgs",
        "org.freedesktop.DBus.Error.UnknownInterface",
        "org.freedesktop.DBus.Error.UnknownProperty",
    ];
    if MISSING_INTERFACE_ERRORS.contains(&name) {
        PortalInterfaceQuery::Missing
    } else {
        PortalInterfaceQuery::Unavailable
    }
}

/// Probe any route with the default (real) runners.
pub fn route_availability(route: TriggerRoute) -> RouteAvailability {
    match route {
        TriggerRoute::PortalGlobalShortcuts => probe_portal_interface().route_availability(),
        TriggerRoute::X11HotkeyFallback => probe_x11_hotkey_fallback(),
        // Always available BY DESIGN for tests; gating happens in the router
        // and exposure via IPC/UI is forbidden by contract (see module docs).
        TriggerRoute::SyntheticTest => RouteAvailability::Available {
            note: SYNTHETIC_NOTE.to_string(),
        },
    }
}

/// Truthful portal probe as route availability (compatibility helper for the
/// router and the diagnostic capability check).
pub fn probe_portal_globalshortcuts() -> RouteAvailability {
    probe_portal_interface().route_availability()
}

/// Probe the X11/XWayland fallback lane. Available exactly when `DISPLAY` is
/// set and non-empty (an X server such as XWayland is presumably reachable);
/// no external process is needed. Actual key-grab qualification is P2 work.
pub fn probe_x11_hotkey_fallback() -> RouteAvailability {
    let display = std::env::var("DISPLAY").ok();
    probe_x11_hotkey_fallback_with(display.as_deref())
}

/// Injected variant of [`probe_x11_hotkey_fallback`].
pub fn probe_x11_hotkey_fallback_with(display: Option<&str>) -> RouteAvailability {
    match display.map(str::trim).filter(|d| !d.is_empty()) {
        Some(display) => RouteAvailability::Available {
            note: format!("XWayland lane (DISPLAY={display})"),
        },
        None => RouteAvailability::Unavailable {
            reason: "DISPLAY unset or empty: no X11/XWayland hotkey lane".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Router policy. The derived default (`allow_synthetic == false`) mirrors
/// the uinput rule: test/synthetic paths are never default-enabled.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TriggerRouterConfig {
    /// Explicitly allow selecting [`TriggerRoute::SyntheticTest`]. Off by
    /// default; even when on, synthetic routes must stay out of IPC/UI.
    pub allow_synthetic: bool,
}

/// Dispatches semantic press/release events to the first available source
/// matching a requested [`TriggerKind`], honoring registration order, an
/// optional route preference, and the synthetic gate.
pub struct TriggerRouter {
    sources: Vec<Box<dyn TriggerSource>>,
    config: TriggerRouterConfig,
}

impl TriggerRouter {
    pub fn new(config: TriggerRouterConfig) -> Self {
        Self {
            sources: Vec::new(),
            config,
        }
    }

    /// Register a candidate source. Earlier registrations win ties.
    pub fn register(&mut self, source: Box<dyn TriggerSource>) {
        self.sources.push(source);
    }

    /// Number of registered sources.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Whether no source is registered.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Routing policy in force.
    pub fn config(&self) -> &TriggerRouterConfig {
        &self.config
    }

    /// Pick the route for `kind`: sources offering the preferred route are
    /// considered first (registration order within each group); the winner
    /// is the first considered source whose route is available and allowed.
    ///
    /// [`TriggerRoute::SyntheticTest`] is skipped unless explicitly allowed,
    /// including when it is the preference. Returns `None` when nothing
    /// qualifies. Dispatch ([`TriggerRouter::press`]/[`release`]) targets
    /// exactly the source chosen here.
    pub fn select_route(
        &self,
        kind: TriggerKind,
        preference: Option<TriggerRoute>,
    ) -> Option<TriggerRoute> {
        let index = self.choose_index(kind, preference)?;
        self.sources.get(index).map(|s| s.route())
    }

    /// Shared selection core so routing decisions and event dispatch can
    /// never diverge. Prefers sources whose route matches `preference`,
    /// then plain registration order; skips gated or unavailable sources.
    fn choose_index(&self, kind: TriggerKind, preference: Option<TriggerRoute>) -> Option<usize> {
        let synthetic_allowed = self.config.allow_synthetic;
        let scan = |prefer_only: bool| {
            self.sources
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.kind() == kind
                        && (s.route() != TriggerRoute::SyntheticTest || synthetic_allowed)
                        && (!prefer_only || preference == Some(s.route()))
                })
                .find(|(_, s)| s.availability().is_available())
                .map(|(index, _)| index)
        };
        scan(true).or_else(|| scan(false))
    }

    /// Dispatch a semantic press to the selected source.
    pub fn press(
        &mut self,
        kind: TriggerKind,
        preference: Option<TriggerRoute>,
    ) -> Result<(), TriggerError> {
        self.select_source_mut(kind, preference)?.press()
    }

    /// Dispatch a semantic release to the selected source.
    pub fn release(
        &mut self,
        kind: TriggerKind,
        preference: Option<TriggerRoute>,
    ) -> Result<(), TriggerError> {
        self.select_source_mut(kind, preference)?.release()
    }

    fn select_source_mut(
        &mut self,
        kind: TriggerKind,
        preference: Option<TriggerRoute>,
    ) -> Result<&mut dyn TriggerSource, TriggerError> {
        let index = self
            .choose_index(kind, preference)
            .ok_or(TriggerError::NoRoute { kind })?;
        Ok(self.sources[index].as_mut())
    }
}

// ---------------------------------------------------------------------------
// Tests (offline: fakes only, plus coreutils echo/sleep for the bounded
// runner's fast and timeout paths on Linux)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
