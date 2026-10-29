//! Start at login, start hidden, and starting the service.
//!
//! - **Start at login** is an XDG autostart entry for this app
//!   (`~/.config/autostart/narrivy.desktop`), which every Linux desktop
//!   reads. "Start hidden" adds `--hidden`: the app starts in the tray.
//! - The **service** (the daemon) is the per-user systemd unit
//!   `narrivy-daemon.service` that the package installs (a development
//!   checkout installs one with `scripts/install-dev-service.sh`). Start at
//!   login enables it too, and the app starts it whenever it opens and finds
//!   the service not running, so dictation works after a reboot without any
//!   command.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Serialize;

pub const HIDDEN_ARG: &str = "--hidden";
const SERVICE: &str = "narrivy-daemon.service";
const MARKER: &str = "X-Narrivy-Autostart=true";

#[derive(Debug, Clone, Serialize)]
pub struct AutostartDto {
    /// The app starts at login.
    pub enabled: bool,
    /// It starts in the tray, without opening its window.
    pub hidden: bool,
    /// A Narrivy service is installed for this user.
    pub service_installed: bool,
}

fn entry_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("autostart").join("narrivy.desktop"))
}

fn systemctl(args: &[&str]) -> bool {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn service_installed() -> bool {
    systemctl(&["cat", SERVICE])
}

/// Quote a path for a desktop entry's `Exec` line.
fn quoted(path: &str) -> String {
    let escaped: String = path
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', c],
            _ => vec![c],
        })
        .collect();
    format!("\"{escaped}\"")
}

fn entry(exe: &str, hidden: bool) -> String {
    let args = if hidden {
        format!(" {HIDDEN_ARG}")
    } else {
        String::new()
    };
    format!(
        "[Desktop Entry]\nType=Application\nName=Narrivy\nComment=Local-first voice dictation\nExec={}{args}\nIcon=narrivy\nTerminal=false\nX-GNOME-Autostart-enabled=true\n{MARKER}\n",
        quoted(exe)
    )
}

/// The current choice.
#[must_use]
pub fn get() -> AutostartDto {
    let text = entry_path().and_then(|p| std::fs::read_to_string(p).ok());
    let enabled = text.as_deref().is_some_and(|t| t.contains(MARKER));
    let hidden = enabled
        && text.as_deref().is_some_and(|t| {
            t.lines()
                .any(|line| line.starts_with("Exec=") && line.ends_with(HIDDEN_ARG))
        });
    AutostartDto {
        enabled,
        hidden,
        service_installed: service_installed(),
    }
}

/// Turn start at login on or off, and choose whether it starts hidden.
///
/// # Errors
/// A short message when the autostart entry cannot be written or removed.
pub fn set(enabled: bool, hidden: bool) -> Result<AutostartDto, String> {
    let path = entry_path().ok_or("no home folder")?;
    if enabled {
        let exe = std::env::current_exe().map_err(|_| "cannot find this program")?;
        let exe = exe.to_str().ok_or("this program's path is not text")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("autostart folder: {e}"))?;
        }
        std::fs::write(&path, entry(exe, hidden)).map_err(|e| format!("autostart entry: {e}"))?;
        if service_installed() {
            let _ = systemctl(&["enable", SERVICE]);
        }
    } else {
        // Only remove an entry this app wrote.
        if std::fs::read_to_string(&path).is_ok_and(|t| t.contains(MARKER)) {
            std::fs::remove_file(&path).map_err(|e| format!("autostart entry: {e}"))?;
        }
        if service_installed() {
            let _ = systemctl(&["disable", SERVICE]);
        }
    }
    Ok(get())
}

/// Start the service when it is not answering. Runs off the UI thread.
pub fn ensure_service() {
    std::thread::spawn(|| {
        if crate::bridge::status_snapshot(&crate::bridge::socket_path()).is_ok() {
            return;
        }
        if service_installed() {
            let _ = systemctl(&["start", SERVICE]);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_quotes_the_path_and_marks_itself() {
        let text = entry("/home/a b/narrivy-desktop", true);
        assert!(text.contains("Exec=\"/home/a b/narrivy-desktop\" --hidden\n"));
        assert!(text.contains(MARKER));
        assert!(!entry("/x", false).contains(HIDDEN_ARG));
        assert_eq!(quoted("a\"$b"), "\"a\\\"\\$b\"");
    }
}
