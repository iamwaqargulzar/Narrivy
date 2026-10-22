//! System tray: open the window, start/stop dictation, start/stop recording
//! a meeting, copy a recent dictation (when a paste was missed), quit.
//!
//! Closing the main window hides it to the tray while the tray exists, like
//! Handy. When no tray host is available (the build fails), closing the
//! window quits as before so the app can never become unreachable.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::bridge;

const TRAY_ID: &str = "narrivy";
const MAIN_WINDOW: &str = "main";

/// Whether a tray icon was installed; read when the main window closes.
pub struct TrayState {
    pub installed: bool,
}

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Narrivy", true, None::<&str>)?;
    let start = MenuItem::with_id(app, "start", "Start dictation", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "Stop dictation", true, None::<&str>)?;
    let copy_last = MenuItem::with_id(app, "copy_last", "Copy last dictation", true, None::<&str>)?;
    let copy_previous = MenuItem::with_id(
        app,
        "copy_previous",
        "Copy the one before",
        true,
        None::<&str>,
    )?;
    let meeting_start =
        MenuItem::with_id(app, "meeting_start", "Record a meeting", true, None::<&str>)?;
    let meeting_stop = MenuItem::with_id(
        app,
        "meeting_stop",
        "Stop meeting and transcribe",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Narrivy", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &start,
            &stop,
            &PredefinedMenuItem::separator(app)?,
            &meeting_start,
            &meeting_stop,
            &PredefinedMenuItem::separator(app)?,
            &copy_last,
            &copy_previous,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Narrivy")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "start" => request(app, || bridge::daemon_start(None, None)),
            "stop" => request(app, bridge::daemon_stop),
            "meeting_start" => request(app, || bridge::daemon_meeting_start(None, false).map(|_| ())),
            "meeting_stop" => request(app, bridge::daemon_meeting_stop),
            "copy_last" => request(app, || bridge::daemon_copy_recent(0)),
            "copy_previous" => request(app, || bridge::daemon_copy_recent(1)),
            "quit" => quit_in_order(app),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Quit: end each web view's content process, then exit.
///
/// The Linux web view's content process can abort inside its own exit
/// cleanup (a heap-corruption report from the C library) when it is left to
/// shut down by itself. Ending it first, through the web view's own API,
/// means that cleanup never runs. Nothing is lost: the shell is exiting and
/// the page holds no unsaved state.
pub fn quit_in_order<R: Runtime>(app: &AppHandle<R>) {
    for window in app.webview_windows().into_values() {
        end_web_content(&window);
    }
    app.exit(0);
}

/// End one window's web content process. A no-op off Linux.
pub fn end_web_content<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    #[cfg(target_os = "linux")]
    {
        use webkit2gtk::WebViewExt as _;
        let _ = window.with_webview(|webview| webview.inner().terminate_web_process());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = window;
}

/// Bring the main window back from the tray (or from a second launch).
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Run one blocking daemon request off the UI thread. On failure the main
/// window opens, where the daemon state and error are shown.
fn request<R: Runtime>(app: &AppHandle<R>, call: fn() -> bridge::CmdResult<()>) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if call().is_err() {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || show_main_window(&handle));
        }
    });
}
