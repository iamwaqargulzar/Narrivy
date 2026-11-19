//! Global shortcuts on Windows (PORT-007).
//!
//! Linux binds shortcuts in the desktop's own shortcut settings (which run
//! `narrivy-cli toggle ...`); Windows has no such store, so Narrivy keeps
//! its own (`desktop_shortcuts`, edited through the same Pipelines screen)
//! and the daemon registers every entry with `RegisterHotKey`
//! (`narrivy-platform-windows`). A press performs the entry's command
//! through this daemon's own pipe (`ToggleSession` with its pipeline and
//! languages, or `CancelSession`), so it takes exactly the admission path
//! every other client takes. Presses are handled one at a time, in order.
//!
//! The plain toggle, [`DEFAULT_SHORTCUT`] (or `NARRIVY_TOGGLE_SHORTCUT`), is
//! registered too unless an entry uses the same keys: dictation works before
//! any shortcut is set, and it uses the current dictation language. Keys
//! another program holds are left unregistered and reported once.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use narrivy_ipc::{
    ActivationSourceDto, Command, RecognitionSettingsDto, RecognitionTagDto, SpokenLanguageDto,
};
use narrivy_platform_windows::os::{ShortcutEvent, ShortcutListener};
use narrivy_platform_windows::shortcuts::Hotkey;

use super::host::linux_features::desktop_shortcuts::{Action, DesktopShortcuts};
use super::shortcuts::{CANCEL_TARGET, Target, resolve_cli, target_of};

/// The toggle shortcut when none is configured.
pub const DEFAULT_SHORTCUT: &str = "Ctrl+Alt+space";
/// Environment override for the toggle shortcut.
pub const SHORTCUT_ENV: &str = "NARRIVY_TOGGLE_SHORTCUT";

/// Starting may first load the speech model.
const TOGGLE_BOUND: Duration = Duration::from_secs(30);

struct Running {
    socket: PathBuf,
    listener: Option<ShortcutListener>,
    /// The registered keys, in listener order, with the result of each.
    registered: Vec<(Hotkey, Result<(), &'static str>)>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Register the shortcuts; `socket` is where this daemon serves. Failure
/// leaves the daemon serving without them (the CLI still toggles).
pub fn start(socket: PathBuf) {
    let mut running = RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let running = running.insert(Running {
        socket,
        listener: None,
        registered: Vec::new(),
    });
    register(running);
    for (hotkey, result) in &running.registered {
        match result {
            Ok(()) => eprintln!("narrivyd: shortcut {hotkey} registered"),
            Err(code) => eprintln!("narrivyd: shortcut {hotkey} unavailable ({code})"),
        }
    }
}

/// Register the stored shortcuts again after the store changed.
pub fn reload() {
    let mut running = RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(running) = running.as_mut() {
        register(running);
    }
}

/// [`reload`], then whether `hotkey` is now registered: `Err` with the
/// reason when Windows refused it (`shortcut_in_use`: another program holds
/// the keys). A daemon without shortcuts accepts everything.
pub fn reload_and_check(hotkey: &Hotkey) -> Result<(), &'static str> {
    let mut running = RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(running) = running.as_mut() else {
        return Ok(());
    };
    register(running);
    running
        .registered
        .iter()
        .find(|(held, _)| held == hotkey)
        .map_or(Ok(()), |(_, result)| *result)
}

/// Unregister the shortcuts and join their threads (daemon shutdown).
pub fn stop() {
    let listener = RUNNING
        .lock()
        .ok()
        .and_then(|mut running| running.as_mut().and_then(|running| running.listener.take()));
    if let Some(listener) = listener
        && let Err(error) = listener.stop()
    {
        eprintln!("narrivyd: shortcuts stop failed ({})", error.code);
    }
}

/// What one registered shortcut does.
#[derive(Debug, PartialEq, Eq)]
enum Press {
    Toggle(Option<Target>),
    Cancel,
}

/// The stored entries Narrivy can perform, then the plain toggle unless an
/// entry already uses its keys.
fn bindings() -> Vec<(Hotkey, Press)> {
    let cli = resolve_cli();
    let mut bindings: Vec<(Hotkey, Press)> = DesktopShortcuts::detect()
        .ok()
        .and_then(|store| store.list().ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let combo = entry.combo?;
            let Action::Spawn(command) = entry.action else {
                return None;
            };
            let target = target_of(command.strip_prefix(cli.as_str())?.strip_prefix(' ')?)?;
            let press = if target.pipeline_id == CANCEL_TARGET {
                Press::Cancel
            } else {
                Press::Toggle(Some(target))
            };
            Some((combo.hotkey().clone(), press))
        })
        .collect();
    let configured = std::env::var(SHORTCUT_ENV).unwrap_or_else(|_| DEFAULT_SHORTCUT.to_owned());
    match Hotkey::parse(&configured) {
        Ok(plain) if !bindings.iter().any(|(held, _)| *held == plain) => {
            bindings.push((plain, Press::Toggle(None)));
        }
        Ok(_) => {}
        Err(_) => eprintln!("narrivyd: toggle shortcut is not a valid combination; not registered"),
    }
    bindings
}

/// Replace the running listener with one for the current bindings.
fn register(running: &mut Running) {
    if let Some(listener) = running.listener.take()
        && let Err(error) = listener.stop()
    {
        eprintln!("narrivyd: shortcuts stop failed ({})", error.code);
    }
    let (hotkeys, presses): (Vec<Hotkey>, Vec<Press>) = bindings().into_iter().unzip();
    let socket = running.socket.clone();
    let on_event = Box::new(move |event: ShortcutEvent| {
        if let ShortcutEvent::Pressed(index) = event
            && let Some(press) = presses.get(index)
        {
            perform(&socket, press);
        }
    });
    match ShortcutListener::start(hotkeys.clone(), None, on_event) {
        Ok((listener, registrations)) => {
            running.registered = hotkeys.into_iter().zip(registrations).collect();
            running.listener = Some(listener);
        }
        Err(error) => {
            eprintln!("narrivyd: shortcuts unavailable ({})", error.code);
            running.registered = hotkeys
                .into_iter()
                .map(|hotkey| (hotkey, Err("shortcut_register_failed")))
                .collect();
        }
    }
}

fn perform(socket: &Path, press: &Press) {
    let command = match press {
        Press::Cancel => Command::CancelSession,
        Press::Toggle(target) => Command::ToggleSession {
            pipeline_id: target.as_ref().map(|target| target.pipeline_id.clone()),
            activation: ActivationSourceDto::InstalledCliCommand,
            recognition: target.as_ref().and_then(recognition_of),
        },
    };
    let answered = super::host::IpcClient::connect_as(socket, "shortcut")
        .and_then(|mut client| client.request(&command, TOGGLE_BOUND));
    if let Err(error) = answered {
        eprintln!("narrivyd: shortcut request failed ({})", error.code);
    }
}

/// The recognition settings a shortcut's languages ask for, as the CLI
/// builds them from `--language` and `--output-language`.
fn recognition_of(target: &Target) -> Option<RecognitionSettingsDto> {
    if target.spoken_language.is_none() && target.output_language.is_none() {
        return None;
    }
    let spoken_language = match target.spoken_language.as_deref() {
        None => SpokenLanguageDto::Unspecified,
        Some("auto") => SpokenLanguageDto::AutoDetect,
        Some(value) => SpokenLanguageDto::Explicit {
            tag: RecognitionTagDto::parse(value.to_owned()).ok()?,
        },
    };
    let output_language = match &target.output_language {
        Some(value) => Some(RecognitionTagDto::parse(value.clone()).ok()?),
        None => None,
    };
    Some(RecognitionSettingsDto {
        spoken_language,
        output_language,
        output_script: None,
        translation_target: None,
        locale: None,
        context_hints: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_language_shortcut_asks_for_its_languages() {
        let target = Target {
            pipeline_id: "clean".to_owned(),
            spoken_language: None,
            output_language: Some("ur".to_owned()),
        };
        let settings = recognition_of(&target);
        assert!(matches!(
            settings.as_ref().map(|s| &s.spoken_language),
            Some(SpokenLanguageDto::Unspecified)
        ));
        assert_eq!(
            settings
                .and_then(|s| s.output_language)
                .map(|tag| tag.as_str().to_owned()),
            Some("ur".to_owned())
        );
        let plain = Target {
            pipeline_id: "clean".to_owned(),
            ..Target::default()
        };
        assert!(recognition_of(&plain).is_none());
    }
}
