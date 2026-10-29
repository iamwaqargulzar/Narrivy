//! System tray: open the window, start/stop dictation, choose the language
//! dictation writes in, start/stop recording a meeting, copy a recent
//! dictation (when a paste was missed), quit.
//!
//! Closing the main window hides it to the tray while the tray exists, like
//! Handy. When no tray host is available (the build fails), closing the
//! window quits as before so the app can never become unreachable.

use std::sync::Mutex;
use std::time::Duration;

use narrivy_ipc::ShortcutBindingDto;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::bridge;

const TRAY_ID: &str = "narrivy";
const MAIN_WINDOW: &str = "main";

/// Whether a tray icon was installed; read when the main window closes.
pub struct TrayState {
    pub installed: bool,
}

/// How often the languages are read again from the daemon, so a change
/// made by another client (or the CLI) shows up too.
const LANGUAGES_REFRESH: Duration = Duration::from_secs(60);
/// The shortcut target that cancels dictation; it has no languages.
const CANCEL_TARGET: &str = "cancel";
/// The menu id of "Automatic" in the Language submenu.
const AUTOMATIC: &str = "auto";

/// The Language submenu: the current dictation language (`None` is
/// automatic) and the languages to choose from. The daemon owns both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageMenu {
    current: Option<String>,
    choices: Vec<String>,
}

/// The menu the tray shows now.
#[derive(Default)]
pub struct TrayLanguages(Mutex<LanguageMenu>);

/// A language tag as the daemon keeps it (`ur`, `en-us`).
fn normalized(tag: &str) -> String {
    tag.trim().to_ascii_lowercase().replace('_', "-")
}

/// The languages to choose from: the user's list, then the language each
/// language shortcut writes in, then the current one, each once.
pub fn language_choices(
    listed: &[String],
    bindings: &[ShortcutBindingDto],
    current: Option<&str>,
) -> Vec<String> {
    let from_shortcuts = bindings
        .iter()
        .filter(|binding| binding.pipeline_id != CANCEL_TARGET)
        .filter_map(|binding| {
            binding
                .output_language
                .as_deref()
                .or(binding.spoken_language.as_deref())
        })
        .filter(|tag| *tag != AUTOMATIC);
    let mut choices: Vec<String> = Vec::new();
    for tag in listed
        .iter()
        .map(String::as_str)
        .chain(from_shortcuts)
        .chain(current)
    {
        let tag = normalized(tag);
        if !tag.is_empty() && !choices.contains(&tag) {
            choices.push(tag);
        }
    }
    choices
}

/// An English name for a language tag; the tag itself when it is not one
/// of the languages people commonly dictate in.
fn language_name(tag: &str) -> String {
    let primary = tag
        .split(['-', '_'])
        .next()
        .unwrap_or(tag)
        .to_ascii_lowercase();
    let name = match primary.as_str() {
        "en" => "English",
        "ur" => "Urdu",
        "hi" => "Hindi",
        "pa" => "Punjabi",
        "sd" => "Sindhi",
        "ps" => "Pashto",
        "fa" => "Persian",
        "ar" => "Arabic",
        "bn" => "Bengali",
        "tr" => "Turkish",
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        "it" => "Italian",
        "pt" => "Portuguese",
        "nl" => "Dutch",
        "pl" => "Polish",
        "ru" => "Russian",
        "uk" => "Ukrainian",
        "sv" => "Swedish",
        "zh" => "Chinese",
        "ja" => "Japanese",
        "ko" => "Korean",
        "id" => "Indonesian",
        "ms" => "Malay",
        "vi" => "Vietnamese",
        "th" => "Thai",
        _ => return tag.to_owned(),
    };
    name.to_owned()
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, languages: &LanguageMenu) -> tauri::Result<Menu<R>> {
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
    let menu = Menu::with_items(app, &[&open, &PredefinedMenuItem::separator(app)?, &start])?;
    // Choosing a language never starts dictation: Start, the toggle and
    // press and hold then dictate in it.
    let automatic = CheckMenuItem::with_id(
        app,
        format!("lang:{AUTOMATIC}"),
        "Automatic",
        true,
        languages.current.is_none(),
        None::<&str>,
    )?;
    let mut items = vec![automatic];
    for tag in &languages.choices {
        items.push(CheckMenuItem::with_id(
            app,
            format!("lang:{tag}"),
            language_name(tag),
            true,
            languages.current.as_deref() == Some(tag.as_str()),
            None::<&str>,
        )?);
    }
    let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> = items
        .iter()
        .map(|item| item as &dyn tauri::menu::IsMenuItem<R>)
        .collect();
    let title = format!(
        "Language: {}",
        languages
            .current
            .as_deref()
            .map_or_else(|| "Automatic".to_owned(), language_name)
    );
    menu.append(&Submenu::with_id_and_items(
        app, "language", title, true, &refs,
    )?)?;
    menu.append_items(&[
        &stop,
        &PredefinedMenuItem::separator(app)?,
        &meeting_start,
        &meeting_stop,
        &PredefinedMenuItem::separator(app)?,
        &copy_last,
        &copy_previous,
        &PredefinedMenuItem::separator(app)?,
        &quit,
    ])?;
    Ok(menu)
}

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    app.manage(TrayLanguages::default());
    let menu = build_menu(app, &LanguageMenu::default())?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Narrivy")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "start" => request(app, || bridge::daemon_start(None, None)),
            "stop" => request(app, bridge::daemon_stop),
            "meeting_start" => request(app, || {
                bridge::daemon_meeting_start(None, false).map(|_| ())
            }),
            "meeting_stop" => request(app, bridge::daemon_meeting_stop),
            "copy_last" => request(app, || bridge::daemon_copy_recent(0)),
            "copy_previous" => request(app, || bridge::daemon_copy_recent(1)),
            "quit" => quit_in_order(app),
            other => {
                if let Some(tag) = other.strip_prefix("lang:") {
                    choose_language(app, tag.to_owned());
                }
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    // Owned by the app for its whole life: reads the languages now
    // and every minute. A failed read (daemon not up yet) keeps the last
    // menu; the thread ends with the process.
    let handle = app.clone();
    std::thread::Builder::new()
        .name("narrivy-tray-languages".to_owned())
        .spawn(move || {
            loop {
                refresh_languages(&handle);
                std::thread::sleep(LANGUAGES_REFRESH);
            }
        })
        .map_err(tauri::Error::Io)?;
    Ok(())
}

/// Read the languages from the daemon and rebuild the menu when they
/// changed. Blocking: call it off the UI thread.
pub fn refresh_languages<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<TrayLanguages>() else {
        return;
    };
    let Ok(options) = bridge::daemon_text_options() else {
        return;
    };
    // Shortcuts are optional: without them the user's list still shows.
    let bindings = bridge::daemon_shortcuts_list()
        .map(|shortcuts| shortcuts.bindings)
        .unwrap_or_default();
    let current = options.dictation_language.as_deref().map(normalized);
    let languages = LanguageMenu {
        choices: language_choices(&options.dictation_languages, &bindings, current.as_deref()),
        current,
    };
    {
        let Ok(mut shown) = state.0.lock() else {
            return;
        };
        if *shown == languages {
            return;
        }
        shown.clone_from(&languages);
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let (Some(tray), Ok(menu)) =
            (handle.tray_by_id(TRAY_ID), build_menu(&handle, &languages))
        {
            let _ = tray.set_menu(Some(menu));
        }
    });
}

/// Make `tag` (or `auto`) the dictation language, then show the menu again
/// with its check mark; the app's pages read it on their next refresh.
fn choose_language<R: Runtime>(app: &AppHandle<R>, tag: String) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let chosen = bridge::daemon_set_dictation_language(tag);
        // Always rebuild: a check menu item toggles itself on click.
        if let Some(state) = app.try_state::<TrayLanguages>()
            && let Ok(mut shown) = state.0.lock()
        {
            *shown = LanguageMenu::default();
        }
        refresh_languages(&app);
        if chosen.is_err() {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || show_main_window(&handle));
        }
    });
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
fn request<R: Runtime>(
    app: &AppHandle<R>,
    call: impl FnOnce() -> bridge::CmdResult<()> + Send + 'static,
) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if call().is_err() {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || show_main_window(&handle));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(
        pipeline: &str,
        spoken: Option<&str>,
        output: Option<&str>,
        key: &str,
    ) -> ShortcutBindingDto {
        ShortcutBindingDto {
            modifiers: vec!["Super".to_owned()],
            key: key.to_owned(),
            pipeline_id: pipeline.to_owned(),
            spoken_language: spoken.map(str::to_owned),
            output_language: output.map(str::to_owned),
        }
    }

    #[test]
    fn lists_the_users_languages_then_each_shortcut_language_once() {
        let choices = language_choices(
            &["en".to_owned()],
            &[
                binding("clean", None, None, "D"),
                binding("cancel", None, None, "Escape"),
                binding("clean", None, Some("ur"), "B"),
                binding("clean", Some("ur"), None, "U"),
                binding("raw", Some("auto"), Some("EN"), "C"),
            ],
            None,
        );
        assert_eq!(choices, ["en", "ur"]);
    }

    #[test]
    fn the_current_language_is_listed_even_off_the_list() {
        assert_eq!(language_choices(&[], &[], Some("pa")), ["pa"]);
        assert!(language_choices(&[], &[], None).is_empty());
    }

    #[test]
    fn names_an_unknown_language_by_its_tag() {
        assert_eq!(language_name("ur-PK"), "Urdu");
        assert_eq!(language_name("yo"), "yo");
    }
}
