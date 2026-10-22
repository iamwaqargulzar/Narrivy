//! Narrivy desktop shell library: Tauri 2 app composition.
//!
//! Layout mirrors the binding contract in apps/narrivy-desktop/CONTRACT.md:
//! - [`bridge`]: typed commands proxying narrivyd over narrivy-ipc;
//! - [`theme`]: System/Light/Dark persistence;
//! - [`capability_ui`]: capability → UI-state vocabulary + the P3-006 gate.
//!
//! Qualification env hooks (all optional; unset env changes nothing):
//! - `NARRIVY_READY_FILE`: write a JSON readiness marker in [`on_setup`];
//! - `NARRIVY_QUALIFY_THEME_APPLY`: one-shot theme apply through the same
//!   ThemeStore the `theme_set` command uses (`system|light|dark`);
//! - `NARRIVY_QUALIFY_LOG`: JSONL observations appended across restarts;
//! - `NARRIVY_THEME_FILE`: theme persistence path override;
//! - `NARRIVY_SOCKET`: daemon socket path override.

#![forbid(unsafe_code)]

pub mod autostart;
pub mod bridge;
pub mod capability_ui;
#[cfg(target_os = "linux")]
mod linux_overlay;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod overlay_state;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod overlay_theme;
#[cfg(target_os = "linux")]
pub mod portal_parent;
pub mod session_events;
mod tauri_commands;
pub mod theme;
mod tray;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewWindow, WindowEvent};

use bridge::{BridgeError, CapabilityCheckResult, StatusSnapshot};
use theme::ThemeStore;

/// Env var for the qualification readiness marker written by [`write_ready_marker`].
pub const NARRIVY_READY_FILE_ENV: &str = "NARRIVY_READY_FILE";
/// Env var selecting a one-shot qualify action at launch (`system|light|dark`).
pub const NARRIVY_QUALIFY_APPLY_ENV: &str = "NARRIVY_QUALIFY_THEME_APPLY";
/// Env var appending JSONL qualify observations across restarts.
pub const NARRIVY_QUALIFY_LOG_ENV: &str = "NARRIVY_QUALIFY_LOG";

/// App setup: overlay placement per overlay.md, qualify lane, and readiness
/// marker. The event-forwarder owner is created after the app build succeeds.
fn on_setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // Linux: a native layer-shell card that never takes focus (overlay.md).
    // Elsewhere, the configured non-focusable overlay window is placed here.
    #[cfg(target_os = "linux")]
    let _ = linux_overlay::install(app.handle());
    #[cfg(not(target_os = "linux"))]
    place_overlay_top_center(app.handle().get_webview_window("overlay").as_ref());

    let installed = match tray::install(app.handle()) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("narrivy-desktop: tray unavailable ({error}); closing the window quits");
            false
        }
    };
    app.manage(tray::TrayState { installed });

    // The window opens unless the app was started hidden (at login) and
    // has a tray to come back from.
    let hidden = installed && std::env::args().any(|arg| arg == autostart::HIDDEN_ARG);
    if !hidden {
        tray::show_main_window(app.handle());
    }
    // The service starts if it is not running (after a reboot, say).
    autostart::ensure_service();

    let store = ThemeStore::resolve(app.path().app_config_dir().ok());

    // Qualify lane: optional one-shot theme application through the SAME
    // ThemeStore the commands use.
    if let Some(mode_raw) = std::env::var(NARRIVY_QUALIFY_APPLY_ENV)
        .ok()
        .filter(|v| !v.is_empty())
    {
        let applied = match mode_raw.as_str() {
            "system" => Some(theme::ThemeMode::System),
            "light" => Some(theme::ThemeMode::Light),
            "dark" => Some(theme::ThemeMode::Dark),
            _ => None,
        };
        match applied {
            Some(mode) => store
                .save(mode)
                .map_err(|e| format!("qualify theme apply failed: {e}"))?,
            None => {
                return Err(format!(
                    "invalid {NARRIVY_QUALIFY_APPLY_ENV}={mode_raw:?} (want system|light|dark)"
                )
                .into());
            }
        }
    }

    // The probes below block, so they run only for qualification lanes that
    // asked for a marker or log; a normal launch never waits on the daemon
    // on the UI thread.
    let qualifying = [NARRIVY_READY_FILE_ENV, NARRIVY_QUALIFY_LOG_ENV]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    if !qualifying {
        return Ok(());
    }

    // Bridge probes for the readiness marker: they exercise the exact code
    // paths the Tauri commands expose (daemon absence lands here as Err and
    // degrades — it must never abort startup).
    // Bounded retry: the daemon may still be starting when we launch (real
    // users and qualification runners both hit this race).
    let mut status = bridge::status_snapshot(&bridge::socket_path());
    if status.is_err() {
        for _ in 0..9 {
            std::thread::sleep(std::time::Duration::from_millis(300));
            status = bridge::status_snapshot(&bridge::socket_path());
            if status.is_ok() {
                break;
            }
        }
    }
    let theme_store_check =
        bridge::run_capability_check(&bridge::socket_path(), "theme-store".to_owned());

    append_qualify_log(&store, &status, &theme_store_check);
    write_ready_marker(app.handle(), &store, &status, &theme_store_check)?;
    Ok(())
}

/// overlay.md truths encoded here (docs/product/ux/overlay.md):
/// 1. never steal focus — the window is created `focusable=false` from
///    config and this code never calls set_focus;
/// 3. monitor-relative geometry — width is 35% of the CURRENT monitor
///    clamped to [400, 760] logical px, top-center placement;
/// 5. mixed-DPI first-class — size/scale are read per-monitor at runtime,
///    so placement stays correct on mixed-DPI rigs.
///
/// Truth 2 (clear transcript state on entry) lives with the overlay UI;
/// truth 4 (services init in the daemon) holds by architecture: this shell
/// initializes nothing domain-shaped, it only proxies narrivyd.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn place_overlay_top_center(overlay: Option<&WebviewWindow>) {
    let Some(window) = overlay else { return };
    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let logical_monitor_width = f64::from(monitor.size().width) / scale;
    let width = (logical_monitor_width * 0.35).clamp(400.0, 760.0);
    let height = 72.0_f64; // one live line per overlay.md truth 3
    let x_physical =
        f64::from(monitor.position().x) + ((logical_monitor_width - width) / 2.0 * scale);
    let y_physical = f64::from(monitor.position().y) + (8.0 * scale);
    let _ = window.set_size(tauri::LogicalSize::new(width, height));
    let _ = window.set_position(tauri::PhysicalPosition::new(
        x_physical.round() as i32,
        y_physical.round() as i32,
    ));
}

/// Explicit owner for the shell's one daemon event-forwarding worker.
struct SessionEventForwarder {
    shutdown: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl SessionEventForwarder {
    fn spawn(app: AppHandle) -> Result<Self, std::io::Error> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = std::thread::Builder::new()
            .name("narrivy-session-events".into())
            .spawn(move || {
                session_events::run(
                    || worker_shutdown.load(Ordering::SeqCst),
                    |payload| {
                        #[cfg(target_os = "linux")]
                        if let Ok(json) = serde_json::to_string(&payload) {
                            linux_overlay::apply_event(&app, json);
                        }
                        if app.emit("session-event", payload).is_ok() {
                            session_events::ForwardControl::Continue
                        } else {
                            session_events::ForwardControl::Stop
                        }
                    },
                );
            })?;
        Ok(Self {
            shutdown,
            worker: Some(worker),
        })
    }

    fn shutdown_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }

    fn shutdown_and_join(mut self) -> Result<(), std::io::Error> {
        self.shutdown.store(true, Ordering::SeqCst);
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker
            .join()
            .map_err(|_| std::io::Error::other("desktop session-event worker panicked"))
    }
}

impl Drop for SessionEventForwarder {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// JSONL observation row for the theme-switch qualification lane. Only runs
/// when `NARRIVY_QUALIFY_LOG` is set; failures degrade to no-op (never panic).
fn append_qualify_log(
    store: &ThemeStore,
    status: &Result<StatusSnapshot, BridgeError>,
    theme_store_check: &Result<CapabilityCheckResult, BridgeError>,
) {
    let Some(path) = std::env::var_os(NARRIVY_QUALIFY_LOG_ENV).filter(|v| !v.is_empty()) else {
        return;
    };
    let row = json!({
        "pid": std::process::id(),
        "at_unix_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        "persisted_theme": store.load().as_str(),
        "bridge_status_ok": status.is_ok(),
        "theme_store_check": match theme_store_check {
            Ok(result) => json!({ "pass": result.pass, "detail": result.detail }),
            Err(err) => json!({ "error": err }),
        },
    });
    use std::io::Write as _;
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{row}");
    }
}

/// Qualification readiness marker (test lanes only): records the RESOLVED
/// built-app window config flags plus live window introspection, bridge
/// status and theme state. Production runs write nothing.
fn write_ready_marker(
    app: &AppHandle,
    store: &ThemeStore,
    status: &Result<StatusSnapshot, BridgeError>,
    theme_store_check: &Result<CapabilityCheckResult, BridgeError>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(raw_path) = std::env::var_os(NARRIVY_READY_FILE_ENV).filter(|v| !v.is_empty()) else {
        return Ok(()); // production runs write nothing
    };

    // Resolved WindowConfig straight from the built app configuration.
    let mut windows: Vec<serde_json::Value> = app
        .config()
        .app
        .windows
        .iter()
        .map(|cfg| {
            json!({
                "label": cfg.label,
                "source": "resolved-config",
                "visible": cfg.visible,
                "decorations": cfg.decorations,
                "focusable": cfg.focusable,
                "always_on_top": cfg.always_on_top,
                "skip_taskbar": cfg.skip_taskbar,
                "focus_on_create": cfg.focus,
                "resizable": cfg.resizable,
                "width": cfg.width,
                "height": cfg.height,
            })
        })
        .collect();

    // Live introspection of actually-created windows (runtime half of the
    // VER-UX-004 assertion).
    for value in windows.iter_mut() {
        let label = value["label"].as_str().unwrap_or_default().to_owned();
        if let Some(w) = app.get_webview_window(&label) {
            value["runtime"] = json!({
                "registered": true,
                "visible": w.is_visible().unwrap_or(false),
                "decorated": w.is_decorated().unwrap_or(true),
                "always_on_top": w.is_always_on_top().unwrap_or(false),
                "resizable": w.is_resizable().unwrap_or(true),
            });
        } else {
            value["runtime"] = json!({ "registered": false });
        }
    }

    let status_json = match status {
        Ok(snapshot) => json!({ "ok": true, "snapshot": snapshot }),
        Err(err) => json!({ "ok": false, "error": err }),
    };
    let check_json = match theme_store_check {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err(err) => json!({ "ok": false, "error": err }),
    };

    let marker = json!({
        "pid": std::process::id(),
        "written_unix_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        "windows": windows,
        "bridge_status": status_json,
        "theme_store_check": check_json,
        "theme_mode": store.load().as_str(),
        "theme_file": store.path().display().to_string(),
        "overlay_lessons": [
            "overlay.md #1 never steal focus: focusable=false, focus=false, never set_focus",
            "overlay.md #3 monitor-relative geometry: 35% width clamp [400,760], bottom-center by default",
            "overlay.md #4 services init in daemon: shell owns zero domain init",
            "overlay.md #5 mixed-DPI: per-monitor scale read at placement time"
        ],
    });

    if let Some(parent) = std::path::Path::new(&raw_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(raw_path, serde_json::to_vec_pretty(&marker)?)?;
    Ok(())
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let app = tauri::Builder::default()
        // A second launch focuses the running window instead of starting a
        // second shell. Registered first, as the plugin requires.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
                && window
                    .try_state::<tray::TrayState>()
                    .is_some_and(|state| state.installed)
            {
                api.prevent_close();
                let _ = window.hide();
            } else if let WindowEvent::CloseRequested { .. } = event
                && let Some(webview) = window.app_handle().get_webview_window(window.label())
            {
                // No tray: closing the window quits. End its web content
                // process first, as Quit does.
                tray::end_web_content(&webview);
            }
        })
        .invoke_handler(tauri::generate_handler![
            tauri_commands::daemon_status,
            tauri_commands::daemon_start,
            tauri_commands::daemon_stop,
            tauri_commands::daemon_cancel,
            tauri_commands::daemon_list_pipelines,
            tauri_commands::daemon_history,
            tauri_commands::daemon_history_entry,
            tauri_commands::daemon_history_delete,
            tauri_commands::daemon_models,
            tauri_commands::daemon_model_select,
            tauri_commands::daemon_pipeline_engine_set,
            tauri_commands::daemon_model_download,
            tauri_commands::daemon_model_download_cancel,
            tauri_commands::daemon_store_list,
            tauri_commands::daemon_store_remove,
            tauri_commands::daemon_assistant_send,
            tauri_commands::daemon_assistant_cancel,
            tauri_commands::daemon_assistant_library,
            tauri_commands::daemon_voice_mode,
            tauri_commands::daemon_assistant_tool_decision,
            tauri_commands::daemon_set_meeting_screen,
            tauri_commands::daemon_meeting_slide,
            tauri_commands::daemon_assistant_unload,
            tauri_commands::daemon_meeting_start,
            tauri_commands::daemon_meeting_stop,
            tauri_commands::daemon_screen_start,
            tauri_commands::daemon_meeting_control,
            tauri_commands::daemon_meeting_sources,
            tauri_commands::pick_folder,
            tauri_commands::daemon_meetings,
            tauri_commands::daemon_meeting,
            tauri_commands::daemon_audio_options,
            tauri_commands::daemon_set_echo_cancel,
            tauri_commands::daemon_set_only_my_voice,
            tauri_commands::daemon_train_voice,
            tauri_commands::daemon_set_noise_suppression,
            tauri_commands::daemon_set_microphone,
            tauri_commands::daemon_hold_key,
            tauri_commands::daemon_set_sounds,
            tauri_commands::daemon_set_history_retention,
            tauri_commands::daemon_clear_history,
            tauri_commands::daemon_text_options,
            tauri_commands::daemon_set_pipeline_voice,
            tauri_commands::daemon_meeting_import,
            tauri_commands::daemon_set_delivery_method,
            tauri_commands::daemon_meeting_options,
            tauri_commands::daemon_set_auto_notes,
            tauri_commands::daemon_set_process_on_stop,
            tauri_commands::daemon_set_meeting_devices,
            tauri_commands::daemon_set_cleanup_idle,
            tauri_commands::daemon_set_voice_strictness,
            tauri_commands::daemon_set_spoken_breaks,
            tauri_commands::daemon_set_numbers_as_digits,
            tauri_commands::daemon_set_hold_key,
            tauri_commands::daemon_vocabulary,
            tauri_commands::app_autostart,
            tauri_commands::app_set_autostart,
            tauri_commands::daemon_set_vocabulary,
            tauri_commands::daemon_forget_voice,
            tauri_commands::daemon_meeting_process,
            tauri_commands::daemon_meeting_rename,
            tauri_commands::daemon_meeting_delete,
            tauri_commands::open_meeting_folder,
            tauri_commands::daemon_meeting_summarize,
            tauri_commands::daemon_language_models,
            tauri_commands::daemon_language_model_select,
            tauri_commands::daemon_shortcuts_list,
            tauri_commands::daemon_shortcut_set,
            tauri_commands::daemon_shortcut_remove,
            tauri_commands::daemon_recovery_entry,
            tauri_commands::daemon_capability_check,
            // P7 FUN-033: versioned pipeline export/import.
            tauri_commands::pipelines_export,
            tauri_commands::pipelines_import,
            tauri_commands::theme_get,
            tauri_commands::theme_set,
            tauri_commands::overlay_appearance,
            tauri_commands::model_install_bundled,
            tauri_commands::model_install_cancel,
        ])
        .setup(on_setup)
        .build(tauri::generate_context!())?;

    let forwarder = SessionEventForwarder::spawn(app.handle().clone())?;
    let shutdown = forwarder.shutdown_signal();
    // `App::run` ends the process itself and never returns, so everything
    // this shell owns is released inside the run-event callback.
    let mut forwarder = Some(forwarder);
    app.run(move |_app_handle, event| match event {
        RunEvent::ExitRequested { .. } => shutdown.store(true, Ordering::SeqCst),
        RunEvent::Exit => {
            shutdown.store(true, Ordering::SeqCst);
            // Join the event worker (bounded by its 500 ms wait) and release
            // the overlay while the toolkit is still alive.
            if let Some(forwarder) = forwarder.take() {
                let _ = forwarder.shutdown_and_join();
            }
            #[cfg(target_os = "linux")]
            linux_overlay::teardown();
        }
        _ => {}
    });
    Ok(())
}
