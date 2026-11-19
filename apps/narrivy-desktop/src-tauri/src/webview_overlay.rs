//! The overlay on hosts without the native Linux card (Windows): the
//! configured `overlay` web-view window, which is non-focusable, always on
//! top and kept off the taskbar (`tauri.conf.json`).
//!
//! The same [`OverlayReducer`] as the Linux card turns daemon session events
//! into presentations, with the same holds and delays; this module turns
//! them into a small content-safe view for the window's page
//! (`?overlay`), places the window for the chosen position and shows or
//! hides it. The window is shown without being activated, so dictated text
//! still goes to the window the user was in (overlay.md: never steal focus).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};

use crate::overlay_state::{
    DONE_HOLD, ERROR_HOLD, ERROR_MESSAGE, LOADING_LINGER, OverlayPresentation, OverlayReducer,
    PROCESSING_LABEL_DELAY, reading_time,
};
use crate::overlay_theme::{Appearance, Horizontal, Palette, Position};

/// The overlay window's label in `tauri.conf.json`.
pub const WINDOW: &str = "overlay";
/// The event that carries an [`OverlayView`] to that window.
const VIEW_EVENT: &str = "overlay-view";
/// The window's size: the pill, plus room for one or two lines of live text.
const WIDTH_FRACTION: f64 = 0.35;
const MIN_WIDTH: f64 = 400.0;
const MAX_WIDTH: f64 = 760.0;
const HEIGHT: f64 = 120.0;
/// Gap from the edge of the monitor's work area.
const EDGE_MARGIN: f64 = 12.0;

/// What the page draws. Content-safe in the sense of overlay.md: the only
/// text is the dictation's own live or cleaned text and fixed labels.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayView {
    /// `listening`, `processing`, `done` or `error`.
    mode: &'static str,
    label: Option<&'static str>,
    text: Option<String>,
    /// Milliseconds the dictation has been listening, when it is.
    listening_ms: Option<u64>,
    top: bool,
    palette: Palette,
}

struct Host {
    reducer: OverlayReducer,
    mode: &'static str,
    label: Option<&'static str>,
    text: Option<String>,
    listening_since: Option<Instant>,
    visible: bool,
    hold_until: Option<Instant>,
    /// Bumped on every change, so a timer set for an earlier state does
    /// nothing when it fires.
    generation: u64,
    appearance: Appearance,
    position: Position,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            reducer: OverlayReducer::default(),
            mode: "listening",
            label: None,
            text: None,
            listening_since: None,
            visible: false,
            hold_until: None,
            generation: 0,
            appearance: Appearance::default(),
            position: Position::default(),
        }
    }
}

static HOST: Mutex<Option<Host>> = Mutex::new(None);

/// Start driving the overlay window. Without the window nothing is shown.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    if app.get_webview_window(WINDOW).is_none() {
        return;
    }
    if let Ok(mut slot) = HOST.lock() {
        *slot = Some(Host::default());
    }
}

/// Feed one daemon session event (JSON). Safe to call from any thread.
pub fn apply_event<R: Runtime>(app: &AppHandle<R>, payload: &str) {
    with_host(app, |app, host| {
        if let Some(presentation) = host.reducer.apply_json(payload) {
            render(app, host, presentation);
        }
    });
}

/// The theme, accent and position the app window reports.
pub fn set_appearance<R: Runtime>(app: &AppHandle<R>, appearance: Appearance, position: Position) {
    with_host(app, |app, host| {
        host.appearance = appearance;
        let moved = host.position != position;
        host.position = position;
        if host.visible {
            if moved {
                place(app, host.position);
            }
            emit(app, host);
        }
    });
}

fn with_host<R: Runtime>(app: &AppHandle<R>, action: impl FnOnce(&AppHandle<R>, &mut Host)) {
    let Ok(mut slot) = HOST.lock() else {
        return;
    };
    if let Some(host) = slot.as_mut() {
        action(app, host);
    }
}

fn render<R: Runtime>(app: &AppHandle<R>, host: &mut Host, presentation: OverlayPresentation) {
    host.generation += 1;
    match presentation {
        OverlayPresentation::Hidden => {
            match host
                .hold_until
                .map(|until| until.saturating_duration_since(Instant::now()))
            {
                Some(rest) if !rest.is_zero() => schedule(app, host, rest, hide),
                _ => hide(app, host),
            }
            return;
        }
        OverlayPresentation::Loading => {
            host.hold_until = None;
            host.mode = "processing";
            host.label = Some("Loading model…");
            host.text = None;
            host.listening_since = None;
        }
        OverlayPresentation::LoadingFinished => {
            // Listening normally follows at once and replaces this timer.
            schedule(app, host, LOADING_LINGER, hide);
            return;
        }
        OverlayPresentation::Listening { partial } => {
            host.hold_until = None;
            host.mode = "listening";
            host.label = None;
            host.listening_since.get_or_insert_with(Instant::now);
            // Every new state entry starts clean (overlay.md truth 2).
            host.text = partial;
        }
        OverlayPresentation::ProcessingPending => {
            // Text already heard stays until the final text replaces it.
            host.mode = "processing";
            host.label = None;
            host.listening_since = None;
            // A quick stage never flashes a label; the page shows only the
            // settled mark until then.
            schedule(app, host, PROCESSING_LABEL_DELAY, |app, host| {
                render(app, host, OverlayPresentation::Processing);
            });
        }
        OverlayPresentation::Processing => {
            host.mode = "processing";
            host.label = None;
        }
        // No audio level reaches the overlay (overlay.md): nothing to draw.
        OverlayPresentation::Level(_) => return,
        OverlayPresentation::Refining { text } => {
            host.mode = "processing";
            host.label = None;
            host.listening_since = None;
            host.text = Some(text);
        }
        OverlayPresentation::Delivered(result) => {
            host.mode = "done";
            host.listening_since = None;
            // A delivered text just fades; only a result the user has to
            // act on says what happened, and stays long enough to read.
            host.label = result.needs_attention().then_some(result.label());
            let hold = if result.needs_attention() {
                ERROR_HOLD.max(host.text.as_deref().map_or(Duration::ZERO, reading_time))
            } else {
                DONE_HOLD
            };
            host.hold_until = Some(Instant::now() + hold);
        }
        OverlayPresentation::Error => {
            host.mode = "error";
            host.label = Some(ERROR_MESSAGE);
            host.text = None;
            host.listening_since = None;
            host.hold_until = Some(Instant::now() + ERROR_HOLD);
        }
    }
    if !host.visible {
        place(app, host.position);
        if let Some(window) = app.get_webview_window(WINDOW) {
            let _ = window.set_ignore_cursor_events(true);
            let _ = window.show();
        }
        host.visible = true;
    }
    emit(app, host);
}

fn hide<R: Runtime>(app: &AppHandle<R>, host: &mut Host) {
    host.hold_until = None;
    host.text = None;
    host.label = None;
    host.listening_since = None;
    if host.visible {
        if let Some(window) = app.get_webview_window(WINDOW) {
            let _ = window.hide();
        }
        host.visible = false;
    }
}

fn emit<R: Runtime>(app: &AppHandle<R>, host: &Host) {
    let view = OverlayView {
        mode: host.mode,
        label: host.label,
        text: host.text.clone(),
        listening_ms: host
            .listening_since
            .map(|since| u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)),
        top: host.position.at_top(),
        palette: host.appearance.palette(),
    };
    let _ = app.emit_to(WINDOW, VIEW_EVENT, view);
}

/// Run `action` after `delay` unless the overlay changed in between.
fn schedule<R: Runtime>(
    app: &AppHandle<R>,
    host: &Host,
    delay: Duration,
    action: fn(&AppHandle<R>, &mut Host),
) {
    let generation = host.generation;
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("narrivy-overlay-timer".to_owned())
        .spawn(move || {
            std::thread::sleep(delay);
            with_host(&app, |app, host| {
                if host.generation == generation {
                    host.generation += 1;
                    action(app, host);
                }
            });
        });
    // Without a timer thread the overlay still hides on the next state.
    drop(spawned);
}

/// Size and place the window on the monitor the pointer is on, in that
/// monitor's work area (clear of the taskbar), at the chosen position.
fn place<R: Runtime>(app: &AppHandle<R>, position: Position) {
    let Some(window) = app.get_webview_window(WINDOW) else {
        return;
    };
    let Some(monitor) = pointer_monitor(app, &window) else {
        return;
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let area_x = f64::from(area.position.x);
    let area_y = f64::from(area.position.y);
    let area_width = f64::from(area.size.width);
    let area_height = f64::from(area.size.height);
    let width = (area_width / scale * WIDTH_FRACTION).clamp(MIN_WIDTH, MAX_WIDTH) * scale;
    let height = HEIGHT * scale;
    let margin = EDGE_MARGIN * scale;
    let x = match position.horizontal() {
        Horizontal::Left => area_x + margin,
        Horizontal::Center => area_x + (area_width - width) / 2.0,
        Horizontal::Right => area_x + area_width - width - margin,
    };
    let y = if position.at_top() {
        area_y + margin
    } else {
        area_y + area_height - height - margin
    };
    let _ = window.set_size(tauri::PhysicalSize::new(
        width.round() as u32,
        height.round() as u32,
    ));
    let _ = window.set_position(tauri::PhysicalPosition::new(
        x.round() as i32,
        y.round() as i32,
    ));
}

fn pointer_monitor<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
) -> Option<tauri::Monitor> {
    app.cursor_position()
        .ok()
        .and_then(|at| app.monitor_from_point(at.x, at.y).ok().flatten())
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())
}
