//! Linux overlay: a native GTK layer-shell card that never takes focus.
//!
//! Follows the approach proven on Pop!_OS COSMIC in the owner's Handy fork
//! (see `docs/product/ux/overlay.md`, "COSMIC/Wayland platform truths"):
//! - a native GTK window becomes a wlr layer-shell surface on the overlay
//!   layer with **no keyboard interactivity**, so it can never take focus
//!   from the application being dictated into (F-H11);
//! - it holds no web view at all: on COSMIC a WebKitGTK view converted to a
//!   layer surface can stay mapped without submitting a buffer, and a web
//!   view would keep a WebKit process (about 190 MB) running for nothing.
//!   So on Linux the overlay window is not in the Tauri configuration
//!   (`tauri.linux.conf.json` lists only the main window);
//! - the monitor is chosen by mapping an invisible 1×1 layer-shell probe and
//!   reading its output. Global pointer coordinates are never trusted;
//! - it sits at the bottom centre unless the user picks another of the six
//!   places in Settings;
//! - colours come from the app's theme and accent (`overlay_theme`), sent by
//!   the app window whenever they change;
//! - the surface is click-through: its input region is empty.
//!
//! The card is drawn, not laid out from widgets, so every change of size and
//! content can ease:
//! - **Compact:** a small pill with the logo's waveform at its centre. While
//!   listening the waves move and a timer counts; processing calms the waves
//!   and a light runs along them.
//! - **Streaming:** once live text arrives the pill widens to 35% of the
//!   monitor (clamped to 400–760 px), then grows downwards line by line with
//!   the logo centred above the text. Beyond half the monitor height the
//!   text scrolls, fading out at the top.
//!
//! The surface is a fixed transparent canvas as large as the card can grow;
//! the card is drawn against the anchored edge, so growing never resizes the
//! surface. When the compositor has no layer-shell support the overlay is
//! **never shown**: a regular window could take focus while text is typed.
//!
//! All functions run on the GTK main thread (Tauri's main thread on Linux).

use std::cell::RefCell;
use std::f64::consts::{PI, TAU};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{cairo, glib, pango};
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use tauri::AppHandle;

use crate::overlay_state::{
    ERROR_MESSAGE, OverlayPresentation, OverlayReducer, PROCESSING_LABEL_DELAY,
};
use crate::overlay_theme::{
    Appearance, Horizontal, LIVE_TAIL_WORDS, Palette, Position, elapsed_label, rgba, tail_start,
};

const OVERLAY_TITLE: &str = "Narrivy Overlay";

/// Streaming width: 35% of the monitor, clamped (overlay.md truth 3).
const MIN_WIDTH: f64 = 400.0;
const MAX_WIDTH: f64 = 760.0;
const WIDTH_FRACTION: f64 = 0.35;

/// The compact pill.
const PILL_HEIGHT: f64 = 40.0;
const PILL_PAD: f64 = 16.0;
const PILL_GAP: f64 = 10.0;

/// The logo's waveform.
const WAVE_WIDTH: f64 = 52.0;
const WAVE_HEIGHT: f64 = 22.0;

/// The streaming card: the logo row, then the text.
const TOP_PAD: f64 = 12.0;
const ROW_HEIGHT: f64 = 26.0;
const ROW_GAP: f64 = 6.0;
const SIDE_PAD: f64 = 22.0;
const BOTTOM_PAD: f64 = 16.0;
/// Height of the fade over text that has scrolled out at the top.
const SCROLL_FADE: f64 = 20.0;

const TEXT_PX: f64 = 15.0;
const LABEL_PX: f64 = 13.0;
const TIMER_PX: f64 = 12.0;

/// Gap between the card and the screen edge it sits against.
const EDGE_MARGIN: i32 = 12;
/// Transparent room around the card so its shadow and glow are not clipped.
const SHADOW_SIDE: f64 = 36.0;
const SHADOW_TOP: f64 = 12.0;
const SHADOW_BOTTOM: f64 = 44.0;

/// How fast each eased value closes on its target (per second).
const RATE_WIDTH: f64 = 13.0;
const RATE_HEIGHT: f64 = 11.0;
const RATE_SCROLL: f64 = 12.0;
const RATE_ENERGY: f64 = 6.0;
const RATE_PRESENCE: f64 = 14.0;
const RATE_FADE: f64 = 10.0;
/// New words fade in over this long.
const WORD_FADE: f64 = 0.22;

/// After a model load ends with no dictation following, the card stays this
/// long so a dictation that does follow never makes it blink.
const LOADING_LINGER: Duration = Duration::from_millis(400);
/// A failure stays visible at least this long, even if the daemon reports
/// idle immediately afterwards (F-M8).
const ERROR_HOLD: Duration = Duration::from_secs(4);
/// How long a delivered result stays before the card fades: the text is in
/// the window already, so only a beat.
const DONE_HOLD: Duration = Duration::from_millis(450);
/// The card's opacity.
const SURFACE_ALPHA: f64 = 0.75;

/// The logo (`BrandMark` in `src/ui/icons.tsx`) in its 32×32 box: a flat
/// lead-in, seven peaks, and a flat tail on the baseline y = 17.
const LOGO_POINTS: [(f64, f64); 10] = [
    (2.0, 17.0),
    (5.0, 17.0),
    (7.2, 11.0),
    (10.2, 24.0),
    (13.4, 5.0),
    (16.6, 27.0),
    (19.6, 12.0),
    (22.2, 21.0),
    (24.2, 17.0),
    (30.0, 17.0),
];
const LOGO_BASELINE: f64 = 17.0;
/// How many recent levels the peaks show, and which peak shows each, the
/// newest in the middle.
const LEVEL_HISTORY: usize = 6;
const PEAK_ORDER: [usize; LEVEL_HISTORY] = [4, 5, 3, 6, 2, 7];
/// A quiet peak keeps a little of the logo's shape.
const REST_PEAK: f64 = 0.1;
/// How fast a peak follows its level.
const RATE_PEAK: f64 = 18.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Listening,
    Processing,
    /// The text was delivered: the waves rest and the result shows.
    Done,
    Error,
}

/// A value that eases towards its target. Without animations it is always
/// at its target.
#[derive(Clone, Copy)]
struct Eased {
    value: f64,
    target: f64,
}

impl Eased {
    const fn at(value: f64) -> Self {
        Self {
            value,
            target: value,
        }
    }

    /// Close on the target: exponential, so a target that moves mid-way is
    /// followed without a jolt.
    fn step(&mut self, seconds: f64, rate: f64) {
        self.value += (self.target - self.value) * (1.0 - (-rate * seconds).exp());
        if (self.target - self.value).abs() < 0.01 {
            self.value = self.target;
        }
    }

    fn settled(&self) -> bool {
        self.value == self.target
    }
}

/// Everything the card draws, shared by the draw handler and the state
/// handling. Only touched on the main thread.
struct Scene {
    palette: Palette,
    animate: bool,
    position: Position,
    mode: Mode,
    /// Status shown in the pill (`Loading model…`, errors, a result to act on).
    label: Option<&'static str>,
    /// When listening began; drives the timer.
    listening_since: Option<Instant>,
    /// The live text; `None` once it is cleared. `text` keeps the last text
    /// so it can fade out while the card shrinks.
    streaming: bool,
    text: Option<String>,
    /// Where the most recent change to the text begins, and when it came.
    fresh_from: usize,
    fresh_at: Instant,
    /// While cleaning up: the byte ranges of words the clean-up changed,
    /// shown in the accent colour. `None` for live text.
    corrections: Option<Vec<(usize, usize)>>,
    /// The monitor the card is on (logical px), once known.
    monitor: Option<(f64, f64)>,
    /// Restarts the waves' motion each time the card appears.
    epoch: Instant,
    last_frame: Option<Instant>,
    hiding: bool,
    width: Eased,
    height: Eased,
    scroll: Eased,
    energy: Eased,
    /// The interior peaks while listening, each following the microphone
    /// level from a moment ago, the newest in the middle: the waves move
    /// with the voice and rest when it is quiet.
    peaks: [Eased; 10],
    /// Recent microphone levels, newest first (0.0..=1.0).
    levels: [f64; LEVEL_HISTORY],
    presence: Eased,
    text_alpha: Eased,
    label_alpha: Eased,
}

impl Scene {
    fn new(palette: Palette, animate: bool) -> Self {
        let now = Instant::now();
        Self {
            palette,
            animate,
            position: Position::default(),
            mode: Mode::Listening,
            label: None,
            listening_since: None,
            streaming: false,
            text: None,
            fresh_from: 0,
            fresh_at: now,
            corrections: None,
            monitor: None,
            epoch: now,
            last_frame: None,
            hiding: false,
            width: Eased::at(compact_width(None)),
            height: Eased::at(PILL_HEIGHT),
            scroll: Eased::at(0.0),
            energy: Eased::at(1.0),
            peaks: [Eased::at(REST_PEAK); 10],
            levels: [0.0; LEVEL_HISTORY],
            presence: Eased::at(0.0),
            text_alpha: Eased::at(0.0),
            label_alpha: Eased::at(0.0),
        }
    }

    /// Forget the previous session entirely (overlay.md truth 2).
    fn reset(&mut self) {
        self.streaming = false;
        self.text = None;
        self.fresh_from = 0;
        self.corrections = None;
        self.levels = [0.0; LEVEL_HISTORY];
        self.peaks = [Eased::at(REST_PEAK); 10];
        self.label = None;
        self.listening_since = None;
        self.scroll = Eased::at(0.0);
        self.text_alpha = Eased::at(0.0);
        self.label_alpha = Eased::at(0.0);
        self.height = Eased::at(PILL_HEIGHT);
        self.width = Eased::at(compact_width(None));
    }

    fn set_text(&mut self, text: String) {
        let previous = self.text.as_deref().unwrap_or("");
        let shared = previous
            .char_indices()
            .zip(text.chars())
            .find(|((_, old), new)| old != new)
            .map_or(previous.len().min(text.len()), |((index, _), _)| index);
        if shared < text.len() {
            self.fresh_from = shared;
            self.fresh_at = Instant::now();
        }
        self.streaming = true;
        self.corrections = None;
        self.text = Some(text);
    }

    /// A new microphone level (0..=1000): it enters the middle peak and the
    /// older ones move outwards.
    fn set_level(&mut self, level: u16) {
        self.levels.rotate_right(1);
        self.levels[0] = f64::from(level.min(1000)) / 1000.0;
        for (slot, &peak) in PEAK_ORDER.iter().enumerate() {
            self.peaks[peak].target = REST_PEAK + (1.0 - REST_PEAK) * self.levels[slot];
        }
        if !self.animate {
            for peak in &mut self.peaks {
                peak.value = peak.target;
            }
        }
    }

    /// Show the text being cleaned up. The first call shows it as
    /// recognised; the next replaces it with the cleaned text, marking the
    /// words that changed.
    fn set_refined_text(&mut self, text: String) {
        let corrections = match (&self.corrections, self.text.as_deref()) {
            (Some(_), Some(previous)) => changed_words(previous, &text),
            _ => Vec::new(),
        };
        self.fresh_from = text.len();
        self.fresh_at = Instant::now();
        self.streaming = true;
        self.corrections = Some(corrections);
        self.text = Some(text);
    }

    fn streaming_width(&self) -> f64 {
        self.monitor.map_or(MIN_WIDTH, |(width, _)| {
            (width * WIDTH_FRACTION).round().clamp(MIN_WIDTH, MAX_WIDTH)
        })
    }

    /// The tallest the text may grow before it scrolls: half the monitor,
    /// less the card's own chrome.
    fn max_text_height(&self) -> f64 {
        let monitor = self.monitor.map_or(1080.0, |(_, height)| height);
        (monitor / 2.0 - (TOP_PAD + ROW_HEIGHT + ROW_GAP + BOTTOM_PAD)).max(TEXT_PX * 3.0)
    }

    /// The surface: big enough for the widest and tallest card.
    fn surface_size(&self) -> (i32, i32) {
        let width = self.streaming_width() + 2.0 * SHADOW_SIDE;
        let height = TOP_PAD
            + ROW_HEIGHT
            + ROW_GAP
            + self.max_text_height()
            + BOTTOM_PAD
            + SHADOW_TOP
            + SHADOW_BOTTOM;
        (width.ceil() as i32, height.ceil() as i32)
    }
}

/// Width of the pill for its content: the waves, and the timer or a label.
fn compact_width(side: Option<f64>) -> f64 {
    let content = WAVE_WIDTH + side.map_or(0.0, |width| PILL_GAP + width);
    content + 2.0 * PILL_PAD
}

struct Card {
    window: gtk::Window,
    canvas: gtk::DrawingArea,
    scene: Rc<RefCell<Scene>>,
    reducer: OverlayReducer,
    /// Mapped (including while fading out).
    visible: bool,
    timer: Option<glib::SourceId>,
    /// Redraws the timer when animations are off.
    clock: Option<glib::SourceId>,
    hold_until: Option<Instant>,
}

thread_local! {
    static CARD: RefCell<Option<Card>> = const { RefCell::new(None) };
}

/// Create the overlay: a native layer-shell card. Returns false (and never
/// shows an overlay) when layer-shell is unavailable. Runs on the main
/// thread during setup, once GTK is initialised.
pub fn install(_app: &AppHandle) -> bool {
    if !gtk_layer_shell::is_supported() {
        return false;
    }
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(OVERLAY_TITLE);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_skip_taskbar_hint(true);
    window.set_skip_pager_hint(true);

    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_exclusive_zone(0);
    place(&window, Position::default());
    window.set_accept_focus(false);
    window.set_focus_on_map(false);

    window.set_app_paintable(true);
    // Pointer input passes through to whatever is underneath.
    let pass_through = |window: &gtk::Window| {
        if let Some(surface) = window.window() {
            surface.input_shape_combine_region(&cairo::Region::create(), 0, 0);
        }
    };
    pass_through(&window);
    window.connect_realize(pass_through);
    if let Some(screen) = WidgetExt::screen(&window)
        && let Some(visual) = screen.rgba_visual()
    {
        window.set_visual(Some(&visual));
    }
    // A transparent window background; the card is all that is drawn.
    window.connect_draw(|_, context| {
        context.set_operator(cairo::Operator::Source);
        context.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        let _ = context.paint();
        context.set_operator(cairo::Operator::Over);
        glib::Propagation::Proceed
    });

    // Reduced motion: the desktop's animation setting turns every animation off.
    let animate =
        gtk::Settings::default().is_none_or(|settings| settings.is_gtk_enable_animations());
    let scene = Rc::new(RefCell::new(Scene::new(
        Appearance::default().palette(),
        animate,
    )));
    let canvas = gtk::DrawingArea::new();
    let drawn = Rc::clone(&scene);
    canvas.connect_draw(move |canvas, context| {
        let size = (
            f64::from(canvas.allocated_width()),
            f64::from(canvas.allocated_height()),
        );
        let finished = paint(
            canvas,
            context,
            &mut drawn.borrow_mut(),
            size,
            Instant::now(),
        );
        if finished {
            // The fade-out is over: unmap once this frame is done.
            glib::idle_add_local_once(|| {
                CARD.with(|slot| {
                    if let Some(card) = slot.borrow_mut().as_mut() {
                        finish_hide(card);
                    }
                });
            });
        }
        glib::Propagation::Proceed
    });
    if animate {
        // Redraw on the frame clock; it only runs while the card is mapped.
        canvas.add_tick_callback(|canvas, _| {
            canvas.queue_draw();
            glib::ControlFlow::Continue
        });
    }
    window.add(&canvas);
    canvas.show();
    let (width, height) = scene.borrow().surface_size();
    set_surface_size(&window, width, height);
    window.hide();

    CARD.with(|slot| {
        *slot.borrow_mut() = Some(Card {
            window,
            canvas,
            scene,
            reducer: OverlayReducer::default(),
            visible: false,
            timer: None,
            clock: None,
            hold_until: None,
        });
    });
    true
}

/// Feed one daemon session event (JSON) to the overlay. Safe to call from
/// any thread: the work runs on the main thread. No-op when not installed.
pub fn apply_event(app: &AppHandle, payload: String) {
    let _ = app.run_on_main_thread(move || {
        CARD.with(|slot| {
            if let Some(card) = slot.borrow_mut().as_mut()
                && let Some(presentation) = card.reducer.apply_json(&payload)
            {
                render(card, presentation);
            }
        });
    });
}

/// Recolour the card to the app's theme and accent. Safe to call from any
/// thread. No-op when not installed.
pub fn set_appearance(app: &AppHandle, appearance: Appearance) {
    let _ = app.run_on_main_thread(move || {
        CARD.with(|slot| {
            if let Some(card) = slot.borrow_mut().as_mut() {
                card.scene.borrow_mut().palette = appearance.palette();
                card.canvas.queue_draw();
            }
        });
    });
}

/// Move the card to another place on the screen. Safe to call from any
/// thread. No-op when not installed.
pub fn set_position(app: &AppHandle, position: Position) {
    let _ = app.run_on_main_thread(move || {
        CARD.with(|slot| {
            if let Some(card) = slot.borrow().as_ref() {
                card.scene.borrow_mut().position = position;
                place(&card.window, position);
                card.canvas.queue_draw();
            }
        });
    });
}

/// Anchor the layer surface for `position`. The surface carries transparent
/// room for the card's shadow, so each margin is what is left of
/// [`EDGE_MARGIN`] after that room. Other surfaces' reserved areas (a panel
/// or dock) are respected by the compositor.
fn place(window: &gtk::Window, position: Position) {
    let top = position.at_top();
    let horizontal = position.horizontal();
    window.set_anchor(Edge::Top, top);
    window.set_anchor(Edge::Bottom, !top);
    window.set_anchor(Edge::Left, horizontal == Horizontal::Left);
    window.set_anchor(Edge::Right, horizontal == Horizontal::Right);
    let side = (EDGE_MARGIN - SHADOW_SIDE as i32).max(0);
    window.set_layer_shell_margin(Edge::Top, (EDGE_MARGIN - SHADOW_TOP as i32).max(0));
    window.set_layer_shell_margin(Edge::Bottom, (EDGE_MARGIN - SHADOW_BOTTOM as i32).max(0));
    window.set_layer_shell_margin(Edge::Left, side);
    window.set_layer_shell_margin(Edge::Right, side);
}

/// Release the overlay before the process exits. Must run on the main
/// thread. The card's widgets and its pending timers are dropped here, while
/// the toolkit is still alive, instead of during process teardown.
pub fn teardown() {
    CARD.with(|slot| {
        if let Some(mut card) = slot.borrow_mut().take() {
            if let Some(timer) = card.timer.take() {
                timer.remove();
            }
            stop_clock(&mut card);
            card.window.close();
        }
    });
}

fn render(card: &mut Card, presentation: OverlayPresentation) {
    if let Some(timer) = card.timer.take() {
        timer.remove();
    }
    match presentation {
        OverlayPresentation::Hidden => {
            if let Some(until) = card.hold_until
                && Instant::now() < until
            {
                schedule(card, until - Instant::now(), hide);
                return;
            }
            hide(card);
        }
        OverlayPresentation::Loading => {
            card.hold_until = None;
            show(card);
            {
                let mut scene = card.scene.borrow_mut();
                scene.streaming = false;
                scene.listening_since = None;
                scene.mode = Mode::Processing;
                scene.label = Some("Loading model…");
            }
            stop_clock(card);
        }
        OverlayPresentation::LoadingFinished => {
            // Listening normally follows at once and replaces this timer.
            schedule(card, LOADING_LINGER, hide);
        }
        OverlayPresentation::Listening { partial } => {
            card.hold_until = None;
            show(card);
            {
                let mut scene = card.scene.borrow_mut();
                scene.mode = Mode::Listening;
                scene.label = None;
                if scene.listening_since.is_none() {
                    scene.listening_since = Some(Instant::now());
                }
                match partial {
                    Some(text) => scene.set_text(text),
                    // Every new state entry starts clean (overlay.md truth 2).
                    None => {
                        scene.streaming = false;
                        scene.text = None;
                    }
                }
            }
            start_clock(card);
        }
        OverlayPresentation::ProcessingPending => {
            {
                let mut scene = card.scene.borrow_mut();
                // The waves settle at once. Text already heard stays (the
                // final text replaces it); without any, the card is the pill.
                scene.streaming = scene.text.is_some();
                scene.listening_since = None;
                scene.mode = Mode::Processing;
                scene.label = None;
            }
            stop_clock(card);
            // Delay the label so very short stages don't flash "Processing".
            schedule(card, PROCESSING_LABEL_DELAY, |card| {
                render(card, OverlayPresentation::Processing);
            });
        }
        OverlayPresentation::Processing => {
            let mut scene = card.scene.borrow_mut();
            // No words while working: the card pulses (the owner's choice).
            scene.mode = Mode::Processing;
            scene.label = None;
        }
        OverlayPresentation::Level(level) => {
            card.scene.borrow_mut().set_level(level);
            // Redrawn on the next frame; nothing else changes.
            card.canvas.queue_draw();
            return;
        }
        OverlayPresentation::Refining { text } => {
            show(card);
            {
                let mut scene = card.scene.borrow_mut();
                scene.listening_since = None;
                scene.mode = Mode::Processing;
                scene.label = None;
                // The cleaned text replaces the recognised text in place;
                // the words that changed are marked.
                scene.set_refined_text(text);
            }
            stop_clock(card);
        }
        OverlayPresentation::Delivered(result) => {
            {
                let mut scene = card.scene.borrow_mut();
                // A cleaned text stays on screen with the result; without
                // one the card is the pill.
                scene.listening_since = None;
                scene.mode = Mode::Done;
                // A delivered text just fades; only a result the user has
                // to act on says what happened.
                scene.label = result.needs_attention().then_some(result.label());
            }
            stop_clock(card);
            // A result the user has to act on stays as long as an error and
            // long enough to read; a pasted one fades at once.
            let hold = if result.needs_attention() {
                let reading = card
                    .scene
                    .borrow()
                    .text
                    .as_deref()
                    .map_or(Duration::ZERO, reading_time);
                ERROR_HOLD.max(reading)
            } else {
                DONE_HOLD
            };
            card.hold_until = Some(Instant::now() + hold);
        }
        OverlayPresentation::Error => {
            show(card);
            {
                let mut scene = card.scene.borrow_mut();
                scene.streaming = false;
                scene.text = None;
                scene.listening_since = None;
                scene.mode = Mode::Error;
                scene.label = Some(ERROR_MESSAGE);
            }
            stop_clock(card);
            card.hold_until = Some(Instant::now() + ERROR_HOLD);
        }
    }
    card.canvas.queue_draw();
}

fn schedule(card: &mut Card, delay: Duration, action: fn(&mut Card)) {
    card.timer = Some(glib::timeout_add_local_once(delay, move || {
        CARD.with(|slot| {
            if let Some(card) = slot.borrow_mut().as_mut() {
                card.timer = None;
                action(card);
            }
        });
    }));
}

/// Without animations nothing redraws on the frame clock, so the timer
/// redraws itself.
fn start_clock(card: &mut Card) {
    if card.clock.is_some() || card.scene.borrow().animate {
        return;
    }
    let canvas = card.canvas.clone();
    card.clock = Some(glib::timeout_add_local(
        Duration::from_millis(250),
        move || {
            canvas.queue_draw();
            glib::ControlFlow::Continue
        },
    ));
}

fn stop_clock(card: &mut Card) {
    if let Some(clock) = card.clock.take() {
        clock.remove();
    }
}

/// Fade out, then unmap (`finish_hide`, called once the fade is drawn).
fn hide(card: &mut Card) {
    card.hold_until = None;
    stop_clock(card);
    if !card.visible {
        return;
    }
    let animate = {
        let mut scene = card.scene.borrow_mut();
        scene.hiding = true;
        scene.presence.target = 0.0;
        scene.streaming = false;
        scene.listening_since = None;
        scene.animate
    };
    if animate {
        card.canvas.queue_draw();
    } else {
        finish_hide(card);
    }
}

fn finish_hide(card: &mut Card) {
    if !card.scene.borrow().hiding {
        return;
    }
    card.window.hide();
    card.visible = false;
    let mut scene = card.scene.borrow_mut();
    scene.hiding = false;
    scene.presence = Eased::at(0.0);
    scene.reset();
}

/// Show on the output under the pointer: map a transparent 1×1 layer-shell
/// probe, let the compositor place it, then move the card to that monitor.
fn show(card: &mut Card) {
    {
        let mut scene = card.scene.borrow_mut();
        if scene.hiding {
            // Shown again while fading out: come back, without the old text.
            scene.hiding = false;
            scene.reset();
        }
        scene.presence.target = 1.0;
    }
    if card.visible {
        return;
    }
    card.visible = true;
    {
        let mut scene = card.scene.borrow_mut();
        scene.reset();
        scene.epoch = Instant::now();
        scene.last_frame = None;
        scene.presence = Eased {
            value: 0.0,
            target: 1.0,
        };
    }
    let probe = gtk::Window::new(gtk::WindowType::Popup);
    probe.set_decorated(false);
    probe.set_accept_focus(false);
    probe.set_app_paintable(true);
    probe.set_opacity(0.0);
    probe.resize(1, 1);
    probe.init_layer_shell();
    probe.set_layer(Layer::Overlay);
    probe.set_keyboard_mode(KeyboardMode::None);
    probe.set_exclusive_zone(0);
    let target = card.window.clone();
    let scene = Rc::clone(&card.scene);
    probe.connect_map_event(move |probe, _| {
        if let Some(gdk_window) = probe.window()
            && let Some(monitor) = probe.display().monitor_at_window(&gdk_window)
        {
            target.set_monitor(&monitor);
            let geometry = monitor.geometry();
            let mut scene = scene.borrow_mut();
            scene.monitor = Some((f64::from(geometry.width()), f64::from(geometry.height())));
            let (width, height) = scene.surface_size();
            set_surface_size(&target, width, height);
        }
        target.show();
        target.queue_draw();
        probe.close();
        glib::Propagation::Proceed
    });
    probe.show();
}

/// Layer surfaces take their size from the size request; `resize(1, 1)`
/// forces the new request to apply (gtk-layer-shell's documented sequence).
fn set_surface_size(window: &gtk::Window, width: i32, height: i32) {
    window.set_size_request(width, height);
    window.resize(1, 1);
}

// ------------------------------------------------------------------ drawing

fn font(px: f64, weight: pango::Weight) -> pango::FontDescription {
    let mut font = pango::FontDescription::new();
    font.set_family("Inter Variable, Inter, sans-serif");
    font.set_weight(weight);
    font.set_absolute_size(px * f64::from(pango::SCALE));
    font
}

fn span(text: &str, colour: &str, alpha: f64) -> String {
    let percent = (alpha.clamp(0.0, 1.0) * 100.0).round().max(1.0);
    format!(
        "<span foreground=\"{colour}\" alpha=\"{percent}%\">{}</span>",
        glib::markup_escape_text(text)
    )
}

fn layout(widget: &impl IsA<gtk::Widget>, px: f64, weight: pango::Weight) -> pango::Layout {
    let layout = widget.create_pango_layout(None);
    layout.set_font_description(Some(&font(px, weight)));
    layout
}

/// The live text as markup: older words quieter, the newest three at full
/// strength, and the words that just arrived fading in.
fn text_markup(text: &str, palette: &Palette, fresh_from: usize, fresh: f64) -> String {
    let tail = tail_start(text, LIVE_TAIL_WORDS);
    let fresh_from = if text.is_char_boundary(fresh_from) {
        fresh_from.min(text.len())
    } else {
        text.len()
    };
    let mut cuts = vec![0, tail, fresh_from, text.len()];
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|pair| {
            let (start, end) = (pair[0], pair[1]);
            let colour = if start >= tail {
                palette.text_strong
            } else {
                palette.muted_strong
            };
            let alpha = if start >= fresh_from { fresh } else { 1.0 };
            span(&text[start..end], colour, alpha)
        })
        .collect()
}

/// How long a text stays after the dictation finished: time to read it
/// (about four words a second), between 2.5 and 8 seconds.
fn reading_time(text: &str) -> Duration {
    let words = text.split_whitespace().count() as u64;
    Duration::from_millis((words * 250).clamp(2_500, 8_000))
}

/// Byte ranges in `new` of the words that are not in `old`, in order
/// (longest common subsequence of the words). Long texts are not compared.
fn changed_words(old: &str, new: &str) -> Vec<(usize, usize)> {
    let words = |text: &str| -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        let mut start = None;
        for (index, character) in text.char_indices() {
            match (character.is_whitespace(), start) {
                (false, None) => start = Some(index),
                (true, Some(begin)) => {
                    spans.push((begin, index));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(begin) = start {
            spans.push((begin, text.len()));
        }
        spans
    };
    let (before, after) = (words(old), words(new));
    if before.len() > 400 || after.len() > 400 {
        return Vec::new();
    }
    // Added punctuation alone does not mark a word.
    let key = |word: &'_ str| {
        word.trim_matches(|c: char| c.is_ascii_punctuation() && c != '\'')
            .to_owned()
    };
    let a: Vec<String> = before.iter().map(|&(s, e)| key(&old[s..e])).collect();
    let b: Vec<String> = after.iter().map(|&(s, e)| key(&new[s..e])).collect();
    let mut table = vec![vec![0_u16; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut changed = Vec::new();
    while j < b.len() {
        if i < a.len() && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if i < a.len() && table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            changed.push(after[j]);
            j += 1;
        }
    }
    changed
}

/// The text being cleaned up: all of it plain, the changed words in the
/// accent colour, fading in.
fn refined_markup(
    text: &str,
    palette: &Palette,
    corrections: &[(usize, usize)],
    fresh: f64,
) -> String {
    let mut markup = String::new();
    let mut at = 0;
    for &(start, end) in corrections {
        if start < at
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            continue;
        }
        markup.push_str(&span(&text[at..start], palette.text_strong, 1.0));
        markup.push_str(&span(
            &text[start..end],
            palette.accent_strong,
            fresh.max(0.35),
        ));
        at = end;
    }
    markup.push_str(&span(&text[at..], palette.text_strong, 1.0));
    markup
}

fn rounded_rectangle(
    context: &cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
) {
    let radius = radius.min(height / 2.0).min(width / 2.0);
    context.new_sub_path();
    context.arc(x + width - radius, y + radius, radius, -PI / 2.0, 0.0);
    context.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        PI / 2.0,
    );
    context.arc(x + radius, y + height - radius, radius, PI / 2.0, PI);
    context.arc(x + radius, y + radius, radius, PI, 1.5 * PI);
    context.close_path();
}

fn source(context: &cairo::Context, colour: &str, alpha: f64) {
    let (red, green, blue, own) = rgba(colour);
    context.set_source_rgba(red, green, blue, own * alpha);
}

fn stop(gradient: &cairo::LinearGradient, offset: f64, colour: &str, alpha: f64) {
    let (red, green, blue, own) = rgba(colour);
    gradient.add_color_stop_rgba(offset, red, green, blue, own * alpha);
}

/// The logo's waveform centred on (`cx`, `cy`). `energy` scales the peaks;
/// while listening each peak also moves at its own pace.
fn wave_path(context: &cairo::Context, scene: &Scene, cx: f64, cy: f64, seconds: f64) {
    let scale_x = WAVE_WIDTH / 28.0;
    let scale_y = WAVE_HEIGHT / 22.0;
    let _ = seconds;
    let listening = scene.mode == Mode::Listening;
    for (index, (x, y)) in LOGO_POINTS.iter().enumerate() {
        // Listening: each peak is the voice's level a moment ago. Otherwise
        // the logo's own shape.
        let swing = if listening && PEAK_ORDER.contains(&index) {
            scene.peaks[index].value
        } else {
            1.0
        };
        let offset = (y - LOGO_BASELINE) * swing * scene.energy.value;
        let px = cx + (x - 16.0) * scale_x;
        let py = cy + offset * scale_y;
        if index == 0 {
            context.move_to(px, py);
        } else {
            context.line_to(px, py);
        }
    }
}

fn draw_wave(context: &cairo::Context, scene: &Scene, cx: f64, cy: f64, seconds: f64) {
    let palette = &scene.palette;
    let left = cx - WAVE_WIDTH / 2.0;
    let right = cx + WAVE_WIDTH / 2.0;
    context.set_line_cap(cairo::LineCap::Round);
    context.set_line_join(cairo::LineJoin::Round);

    if scene.mode == Mode::Listening {
        // A soft live halo under the waves, as bright as the voice is loud.
        let breath = scene.levels[0];
        wave_path(context, scene, cx, cy, seconds);
        source(context, palette.live, 0.14 + 0.10 * breath);
        context.set_line_width(7.0);
        let _ = context.stroke();
    }

    wave_path(context, scene, cx, cy, seconds);
    context.set_line_width(2.3);
    match scene.mode {
        Mode::Error => source(context, palette.err, 1.0),
        Mode::Listening | Mode::Processing | Mode::Done => {
            // The logo's own gradient.
            let gradient = cairo::LinearGradient::new(left, 0.0, right, 0.0);
            stop(&gradient, 0.0, palette.accent, 1.0);
            stop(&gradient, 1.0, palette.accent_strong, 1.0);
            let _ = context.set_source(&gradient);
        }
    }
    let _ = context.stroke();

    if scene.mode == Mode::Processing && scene.animate {
        // A light running along the waves.
        let travel = (seconds / 1.1).fract();
        let at = left - 12.0 + travel * (WAVE_WIDTH + 24.0);
        let glow = cairo::LinearGradient::new(at - 14.0, 0.0, at + 14.0, 0.0);
        stop(&glow, 0.0, palette.live, 0.0);
        stop(&glow, 0.5, palette.live, 0.95);
        stop(&glow, 1.0, palette.live, 0.0);
        wave_path(context, scene, cx, cy, seconds);
        context.set_line_width(2.6);
        let _ = context.set_source(&glow);
        let _ = context.stroke();
    }
}

/// Draw one frame into `size` (the surface). Returns true when a fade-out
/// has just finished and the surface can be unmapped.
fn paint(
    widget: &impl IsA<gtk::Widget>,
    context: &cairo::Context,
    scene: &mut Scene,
    size: (f64, f64),
    now: Instant,
) -> bool {
    let seconds_since = |instant: Instant| now.saturating_duration_since(instant).as_secs_f64();
    let dt = if scene.animate {
        scene.last_frame.map_or(0.0, seconds_since).min(0.1)
    } else {
        f64::INFINITY
    };
    scene.last_frame = Some(now);
    let seconds = seconds_since(scene.epoch);
    let palette = scene.palette;

    // What the card holds this frame.
    let timer = scene.listening_since.map(|since| {
        let timer = layout(widget, TIMER_PX, pango::Weight::Medium);
        timer.set_markup(&format!(
            "<span font_features=\"tnum\">{}</span>",
            span(
                &elapsed_label(seconds_since(since) as u64),
                palette.muted,
                1.0
            )
        ));
        timer
    });
    let timer_width = timer
        .as_ref()
        .map_or(0.0, |timer| f64::from(timer.pixel_size().0));
    let wide = scene.streaming_width();
    let label = scene.label.map(|text| {
        let label = layout(widget, LABEL_PX, pango::Weight::Semibold);
        label.set_width(
            ((wide - WAVE_WIDTH - PILL_GAP - 2.0 * PILL_PAD) * f64::from(pango::SCALE)) as i32,
        );
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.set_markup(&span(text, palette.text_strong, 1.0));
        label
    });
    let label_width = label
        .as_ref()
        .map_or(0.0, |label| f64::from(label.pixel_extents().1.width()));
    let side = if label.is_some() {
        Some(label_width)
    } else {
        timer.is_some().then_some(timer_width)
    };
    let compact = compact_width(side);

    let text_width = wide - 2.0 * SIDE_PAD;
    let fresh = if scene.animate {
        (seconds_since(scene.fresh_at) / WORD_FADE).min(1.0)
    } else {
        1.0
    };
    let text = scene.text.as_deref().map(|content| {
        let text = layout(widget, TEXT_PX, pango::Weight::Normal);
        text.set_width((text_width * f64::from(pango::SCALE)) as i32);
        text.set_wrap(pango::WrapMode::WordChar);
        text.set_alignment(pango::Alignment::Center);
        text.set_spacing(2 * pango::SCALE);
        text.set_markup(&match &scene.corrections {
            Some(corrections) => refined_markup(content, &palette, corrections, fresh),
            None => text_markup(content, &palette, scene.fresh_from, fresh),
        });
        text
    });
    let text_height = text
        .as_ref()
        .map_or(0.0, |text| f64::from(text.pixel_size().1));
    let visible_text = text_height.min(scene.max_text_height());
    let tall = TOP_PAD + ROW_HEIGHT + ROW_GAP + visible_text + BOTTOM_PAD;

    // Targets. Opening widens first, then grows down; closing shrinks the
    // height first, then the width.
    if scene.streaming && text.is_some() {
        scene.width.target = wide;
        scene.height.target =
            if !scene.animate || scene.width.value >= compact + 0.9 * (wide - compact) {
                tall
            } else {
                PILL_HEIGHT
            };
        scene.text_alpha.target = 1.0;
    } else {
        scene.height.target = PILL_HEIGHT;
        scene.width.target = if scene.animate && scene.height.value > PILL_HEIGHT + 4.0 {
            scene.width.value
        } else {
            compact
        };
        scene.text_alpha.target = 0.0;
    }
    scene.scroll.target = (text_height - visible_text).max(0.0);
    scene.energy.target = match scene.mode {
        Mode::Listening => 1.0,
        Mode::Processing => 0.32,
        Mode::Done => 0.6,
        Mode::Error => 0.25,
    };
    scene.label_alpha.target = if label.is_some() { 1.0 } else { 0.0 };

    scene.width.step(dt, RATE_WIDTH);
    scene.height.step(dt, RATE_HEIGHT);
    scene.scroll.step(dt, RATE_SCROLL);
    scene.energy.step(dt, RATE_ENERGY);
    for peak in &mut scene.peaks {
        peak.step(dt, RATE_PEAK);
    }
    scene.presence.step(dt, RATE_PRESENCE);
    scene.text_alpha.step(dt, RATE_FADE);
    scene.label_alpha.step(dt, RATE_FADE);
    if !scene.streaming && scene.text_alpha.value == 0.0 && scene.height.settled() {
        scene.text = None;
    }

    // Where the card sits on the surface.
    let (surface_width, surface_height) = size;
    let width = scene.width.value;
    let height = scene.height.value;
    let x = match scene.position.horizontal() {
        Horizontal::Left => SHADOW_SIDE,
        Horizontal::Center => (surface_width - width) / 2.0,
        Horizontal::Right => surface_width - SHADOW_SIDE - width,
    };
    let at_top = scene.position.at_top();
    let y = if at_top {
        SHADOW_TOP
    } else {
        surface_height - SHADOW_BOTTOM - height
    };

    // Appearing and leaving: fade, and rise from a slightly smaller card
    // towards the anchored edge.
    let presence = scene.presence.value;
    if presence <= 0.0 {
        return scene.hiding;
    }
    let grow = 0.94 + 0.06 * presence;
    let anchor_x = x + width / 2.0;
    let anchor_y = if at_top { y } else { y + height };
    context.save().ok();
    context.translate(anchor_x, anchor_y);
    context.scale(grow, grow);
    let rise = (1.0 - presence) * if at_top { -6.0 } else { 6.0 };
    context.translate(-anchor_x, -anchor_y + rise);
    context.push_group();

    // Shadow: a few soft layers under the card.
    let radius = (PILL_HEIGHT / 2.0).min(height / 2.0).min(22.0);
    let (_, _, _, shadow) = rgba(palette.shadow);
    for layer in 1..=6 {
        let spread = f64::from(layer) * 2.2;
        rounded_rectangle(
            context,
            x - spread,
            y - spread + 6.0,
            width + 2.0 * spread,
            height + 2.0 * spread,
            radius + spread,
        );
        context.set_source_rgba(0.0, 0.0, 0.0, shadow * 0.09);
        let _ = context.fill();
    }

    // State glow around the edge.
    let glow = match scene.mode {
        Mode::Listening => Some(palette.live),
        Mode::Processing | Mode::Done => Some(palette.accent),
        Mode::Error => Some(palette.err),
    };
    if let Some(colour) = glow {
        // Working: the glow pulses. Listening: it follows the voice.
        let pulse = match scene.mode {
            Mode::Processing if scene.animate => {
                0.7 + 0.6 * (0.5 + 0.5 * (seconds * TAU / 1.2).sin())
            }
            Mode::Listening => 0.55 + 0.6 * scene.levels[0],
            _ => 0.8,
        };
        for layer in 1..=4 {
            let spread = f64::from(layer) * 2.0;
            rounded_rectangle(
                context,
                x - spread,
                y - spread,
                width + 2.0 * spread,
                height + 2.0 * spread,
                radius + spread,
            );
            source(context, colour, 0.05 * pulse);
            let _ = context.fill();
        }
    }

    // The card.
    rounded_rectangle(context, x, y, width, height, radius);
    let fill = cairo::LinearGradient::new(0.0, y, 0.0, y + height);
    // See-through, so the card sits on the desktop rather than over it.
    stop(&fill, 0.0, palette.surface_top, SURFACE_ALPHA);
    stop(&fill, 1.0, palette.surface_bottom, SURFACE_ALPHA);
    let _ = context.set_source(&fill);
    let _ = context.fill_preserve();
    let edge = match scene.mode {
        Mode::Listening => (palette.live, 0.5),
        Mode::Processing | Mode::Done => (palette.accent, 0.4),
        Mode::Error => (palette.err, 0.6),
    };
    source(context, edge.0, edge.1);
    context.set_line_width(1.0);
    let _ = context.stroke();
    // A thin highlight along the top edge.
    rounded_rectangle(
        context,
        x + 0.5,
        y + 0.5,
        width - 1.0,
        height - 1.0,
        radius - 0.5,
    );
    let shine = cairo::LinearGradient::new(0.0, y, 0.0, y + 10.0);
    stop(&shine, 0.0, palette.highlight, 1.0);
    stop(&shine, 1.0, palette.highlight, 0.0);
    let _ = context.set_source(&shine);
    context.set_line_width(1.0);
    let _ = context.stroke();

    // How far the card has opened: 0 as a pill, 1 at the streaming width.
    let opened = if wide > compact {
        ((width - compact) / (wide - compact)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let centre = x + width / 2.0;
    let row_centre = y + (height / 2.0).min(TOP_PAD + ROW_HEIGHT / 2.0);

    // The waves: beside the timer or label in the pill, centred above the
    // text once open.
    let pill_offset = -side.map_or(0.0, |width| PILL_GAP + width) / 2.0;
    let wave_x = centre + pill_offset * (1.0 - opened);
    draw_wave(context, scene, wave_x, row_centre, seconds);

    if let Some(timer) = &timer {
        let (_, timer_height) = timer.pixel_size();
        let beside = wave_x + WAVE_WIDTH / 2.0 + PILL_GAP;
        let corner = x + width - SIDE_PAD - timer_width;
        let timer_x = beside + (corner - beside) * opened;
        gtk::render_layout(
            &widget.style_context(),
            context,
            timer_x,
            row_centre - f64::from(timer_height) / 2.0,
            timer,
        );
    }

    if let Some(label) = &label {
        let (_, label_height) = label.pixel_size();
        let label_x = wave_x + WAVE_WIDTH / 2.0 + PILL_GAP;
        context.push_group();
        gtk::render_layout(
            &widget.style_context(),
            context,
            label_x,
            row_centre - f64::from(label_height) / 2.0,
            label,
        );
        if let Ok(pattern) = context.pop_group() {
            let _ = context.set_source(&pattern);
            let _ = context.paint_with_alpha(scene.label_alpha.value);
        }
    }

    if let Some(text) = &text {
        let top = y + TOP_PAD + ROW_HEIGHT + ROW_GAP;
        let shown = (y + height - BOTTOM_PAD - top).max(0.0);
        let alpha = scene.text_alpha.value
            * ((height - PILL_HEIGHT) / (tall - PILL_HEIGHT).max(1.0)).clamp(0.0, 1.0);
        if shown > 0.0 && alpha > 0.0 {
            context.save().ok();
            context.rectangle(x, top, width, shown + BOTTOM_PAD / 2.0);
            context.clip();
            context.push_group();
            gtk::render_layout(
                &widget.style_context(),
                context,
                centre - text_width / 2.0,
                top - scene.scroll.value,
                text,
            );
            if let Ok(pattern) = context.pop_group() {
                let _ = context.set_source(&pattern);
                if scene.scroll.value > 0.5 {
                    // Text scrolled out at the top fades away.
                    let fade = cairo::LinearGradient::new(0.0, top, 0.0, top + SCROLL_FADE);
                    fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.0);
                    fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, alpha);
                    let _ = context.mask(&fade);
                } else {
                    let _ = context.paint_with_alpha(alpha);
                }
            }
            context.restore().ok();
        }
    }

    if let Ok(card) = context.pop_group() {
        let _ = context.set_source(&card);
        let _ = context.paint_with_alpha(presence);
    }
    context.restore().ok();
    scene.hiding && presence < 0.02
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay_theme::{Accent, Scheme, rgb};

    #[test]
    fn eased_values_settle_on_their_target() {
        let mut value = Eased::at(0.0);
        value.target = 100.0;
        value.step(0.016, RATE_WIDTH);
        assert!(value.value > 0.0 && value.value < 100.0);
        for _ in 0..200 {
            value.step(0.016, RATE_WIDTH);
        }
        assert!(value.settled());
        value.target = 5.0;
        value.step(f64::INFINITY, RATE_WIDTH);
        assert_eq!(value.value, 5.0, "without animations a value jumps");
    }

    #[test]
    fn new_words_are_marked_from_where_the_text_changed() {
        let palette = Appearance::default().palette();
        let mut scene = Scene::new(palette, true);
        scene.set_text("move the review".to_owned());
        assert_eq!(scene.fresh_from, 0);
        scene.set_text("move the review to Thursday".to_owned());
        assert_eq!(scene.fresh_from, "move the review".len());
        scene.set_text("move the meeting".to_owned());
        assert_eq!(scene.fresh_from, "move the ".len());

        let markup = text_markup("déjà vu encore une fois", &palette, 3, 0.5);
        assert!(markup.contains("alpha=\"50%\""), "{markup}");
        assert!(markup.contains(palette.text_strong));
        // A cut inside a character is ignored rather than splitting it.
        let whole = text_markup("déjà", &palette, 2, 0.0);
        assert!(whole.contains("déjà"), "{whole}");
        assert!(text_markup("a < b & c", &palette, 0, 1.0).contains("&lt;"));
    }

    #[test]
    fn the_surface_holds_the_widest_and_tallest_card() {
        let mut scene = Scene::new(Appearance::default().palette(), true);
        scene.monitor = Some((1920.0, 1200.0));
        assert_eq!(scene.streaming_width(), 672.0);
        let (width, height) = scene.surface_size();
        assert_eq!(width, 672 + 2 * SHADOW_SIDE as i32);
        assert_eq!(height, 600 + (SHADOW_TOP + SHADOW_BOTTOM) as i32);
        scene.monitor = Some((800.0, 600.0));
        assert_eq!(scene.streaming_width(), MIN_WIDTH);
        scene.monitor = Some((5120.0, 2880.0));
        assert_eq!(scene.streaming_width(), MAX_WIDTH);
        assert!(compact_width(None) < 100.0);
        assert!(compact_width(Some(30.0)) < 140.0);
    }

    #[test]
    fn a_finished_text_stays_long_enough_to_read() {
        assert_eq!(reading_time("Pasted."), Duration::from_millis(2_500));
        assert_eq!(
            reading_time(&"word ".repeat(20)),
            Duration::from_millis(5_000)
        );
        assert_eq!(
            reading_time(&"word ".repeat(100)),
            Duration::from_millis(8_000)
        );
    }

    #[test]
    fn the_words_a_clean_up_changed_are_found() {
        let raw = "so um i need to send the the report by friday no wait thursday";
        let cleaned = "I need to send the report by Thursday.";
        let changed: Vec<&str> = changed_words(raw, cleaned)
            .into_iter()
            .map(|(start, end)| &cleaned[start..end])
            .collect();
        assert_eq!(changed, ["I", "Thursday."]);
        // Punctuation added to a word alone does not mark it.
        assert!(changed_words("see you soon", "See you soon.").len() == 1);
        assert!(changed_words("same words", "same words").is_empty());
        let palette = Appearance::default().palette();
        let markup = refined_markup(cleaned, &palette, &changed_words(raw, cleaned), 1.0);
        assert!(markup.contains("Thursday."));
    }

    /// Renders the card off-screen in every appearance and state, for a
    /// person to look at. Needs a display, so it is not part of the suite:
    /// `NARRIVY_OVERLAY_PREVIEW_DIR=/some/dir cargo test -p narrivy-desktop
    /// overlay_preview -- --ignored`.
    #[test]
    #[ignore = "needs a display; writes preview images"]
    fn overlay_preview() -> Result<(), Box<dyn std::error::Error>> {
        let Ok(directory) = std::env::var("NARRIVY_OVERLAY_PREVIEW_DIR") else {
            return Ok(());
        };
        gtk::init()?;
        let widget = gtk::DrawingArea::new();
        let sample = "Let us move the design review to Thursday afternoon and invite the platform team as well, so that everyone hears the plan at once";
        for (scheme, scheme_name) in [(Scheme::Dark, "dark"), (Scheme::Light, "light")] {
            for (accent, accent_name) in [
                (Accent::Mono, "mono"),
                (Accent::Blue, "blue"),
                (Accent::Gold, "gold"),
            ] {
                let palette = Appearance { scheme, accent }.palette();
                for (mode, mode_name, label, partial) in [
                    (Mode::Listening, "listening", None, None),
                    (Mode::Listening, "streaming", None, Some(sample)),
                    (Mode::Processing, "processing", None, None),
                    (Mode::Processing, "loading", Some("Loading model…"), None),
                    (Mode::Processing, "refining", None, Some(sample)),
                    (Mode::Done, "pasted", None, None),
                    (Mode::Done, "pasted-text", None, Some(sample)),
                    (
                        Mode::Done,
                        "inbox",
                        Some("Not inserted — saved to Inbox"),
                        None,
                    ),
                    (Mode::Error, "error", Some(ERROR_MESSAGE), None),
                ] {
                    let mut scene = Scene::new(palette, false);
                    scene.monitor = Some((1920.0, 1200.0));
                    scene.mode = mode;
                    scene.label = label;
                    scene.presence = Eased::at(1.0);
                    if mode == Mode::Listening {
                        for level in [200, 750, 420, 900, 610, 330] {
                            scene.set_level(level);
                        }
                        scene.listening_since = Some(Instant::now() - Duration::from_secs(7));
                    }
                    if let Some(text) = partial {
                        if mode_name == "refining" {
                            scene.set_refined_text(
                                "let us um move the design review to thursday afternoon".to_owned(),
                            );
                            scene.set_refined_text(
                                "Let's move the design review to Thursday afternoon.".to_owned(),
                            );
                        } else {
                            scene.set_text(text.to_owned());
                        }
                    }
                    let width = scene.streaming_width() + 2.0 * SHADOW_SIDE;
                    let height = 220.0;
                    let surface = cairo::ImageSurface::create(
                        cairo::Format::ARgb32,
                        width as i32,
                        height as i32,
                    )?;
                    let context = cairo::Context::new(&surface)?;
                    // A desktop-like backdrop so the shadow and edges show.
                    let backdrop = if scheme == Scheme::Dark {
                        "#3a3f4b"
                    } else {
                        "#d9dde4"
                    };
                    let (red, green, blue) = rgb(backdrop);
                    context.set_source_rgb(red, green, blue);
                    context.paint()?;
                    // Two frames: targets settle on the second.
                    let now = Instant::now();
                    paint(&widget, &context, &mut scene, (width, height), now);
                    paint(&widget, &context, &mut scene, (width, height), now);
                    context.set_source_rgb(red, green, blue);
                    context.paint()?;
                    paint(&widget, &context, &mut scene, (width, height), now);
                    drop(context);
                    let image = gtk::gdk::pixbuf_get_from_surface(
                        &surface,
                        0,
                        0,
                        width as i32,
                        height as i32,
                    )
                    .ok_or("the frame could not be read")?;
                    image.savev(
                        format!("{directory}/{scheme_name}-{accent_name}-{mode_name}.png"),
                        "png",
                        &[],
                    )?;
                }
            }
        }

        // The opening, frame by frame: the pill widens, then grows down as
        // the text arrives, then grows again when a line is added.
        let mut scene = Scene::new(Appearance::default().palette(), true);
        scene.monitor = Some((1920.0, 1200.0));
        scene.presence = Eased::at(1.0);
        let start = Instant::now();
        scene.epoch = start;
        scene.listening_since = Some(start);
        let width = scene.streaming_width() + 2.0 * SHADOW_SIDE;
        let height = 220.0;
        for frame in 0..24 {
            let now = start + Duration::from_millis(frame * 40);
            if frame == 4 {
                scene.set_text("Let us move the design review".to_owned());
            }
            if frame == 14 {
                scene.set_text(sample.to_owned());
            }
            let surface =
                cairo::ImageSurface::create(cairo::Format::ARgb32, width as i32, height as i32)?;
            let context = cairo::Context::new(&surface)?;
            context.set_source_rgb(0.23, 0.25, 0.29);
            context.paint()?;
            paint(&widget, &context, &mut scene, (width, height), now);
            drop(context);
            let image =
                gtk::gdk::pixbuf_get_from_surface(&surface, 0, 0, width as i32, height as i32)
                    .ok_or("the frame could not be read")?;
            image.savev(format!("{directory}/opening-{frame:02}.png"), "png", &[])?;
        }
        Ok(())
    }
}
