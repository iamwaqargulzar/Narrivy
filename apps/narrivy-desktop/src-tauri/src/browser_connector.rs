//! The writing assistant's browser connector (D26, D48): registering the
//! native-messaging host the package installs with this user's Chromium-based
//! browsers.
//!
//! The package ships the host and the unpacked extension: on Linux
//! `/usr/lib/narrivy/narrivy-browser-host` and
//! `/usr/share/narrivy/browser-extension`, on Windows
//! `narrivy-browser-host.exe` and `browser-extension` in the install folder.
//! Turning the writing assistant on in Settings → Extensions registers one
//! host manifest, allowed only for Narrivy's extension ID, with each browser:
//! on Linux a file in the browser's `NativeMessagingHosts` folder, on Windows
//! a key under the browser's `HKCU\…\NativeMessagingHosts` naming a manifest
//! in `%LOCALAPPDATA%\Narrivy`. Turning it off removes what this app wrote.
//! Nothing is written for a browser that was never opened.

use std::path::Path;

use serde::Serialize;

const HOST_NAME: &str = "com.narrivy.writing";
/// Fixed by the public key in the extension's `manifest.json`; the host
/// accepts no other caller.
const EXTENSION_ID: &str = "khkbeljpdjbgihgbedobeaoedkdffkie";

/// A browser the extension supports.
struct Browser {
    id: &'static str,
    name: &'static str,
    /// Its profile folder, under `~/.config` on Linux and `%LOCALAPPDATA%`
    /// on Windows.
    folder: &'static str,
    extensions_page: &'static str,
    /// The `NativeMessagingHosts` keys it reads, its own first (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    keys: &'static [&'static str],
}

#[cfg(not(windows))]
const BROWSERS: &[Browser] = &[
    Browser {
        id: "brave",
        name: "Brave",
        folder: "BraveSoftware/Brave-Browser",
        extensions_page: "brave://extensions",
        keys: &[],
    },
    Browser {
        id: "chrome",
        name: "Google Chrome",
        folder: "google-chrome",
        extensions_page: "chrome://extensions",
        keys: &[],
    },
    Browser {
        id: "chromium",
        name: "Chromium",
        folder: "chromium",
        extensions_page: "chrome://extensions",
        keys: &[],
    },
    Browser {
        id: "edge",
        name: "Microsoft Edge",
        folder: "microsoft-edge",
        extensions_page: "edge://extensions",
        keys: &[],
    },
];

#[cfg(windows)]
const BROWSERS: &[Browser] = &[
    Browser {
        id: "brave",
        name: "Brave",
        folder: r"BraveSoftware\Brave-Browser\User Data",
        extensions_page: "brave://extensions",
        // Brave releases have read either their own key or Chrome's.
        keys: &[
            r"HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
            r"HKCU\Software\Google\Chrome\NativeMessagingHosts",
        ],
    },
    Browser {
        id: "chrome",
        name: "Google Chrome",
        folder: r"Google\Chrome\User Data",
        extensions_page: "chrome://extensions",
        keys: &[r"HKCU\Software\Google\Chrome\NativeMessagingHosts"],
    },
    Browser {
        id: "chromium",
        name: "Chromium",
        folder: r"Chromium\User Data",
        extensions_page: "chrome://extensions",
        keys: &[r"HKCU\Software\Chromium\NativeMessagingHosts"],
    },
    Browser {
        id: "edge",
        name: "Microsoft Edge",
        folder: r"Microsoft\Edge\User Data",
        extensions_page: "edge://extensions",
        keys: &[r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts"],
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct BrowserDto {
    pub id: &'static str,
    pub name: &'static str,
    /// Its profile folder exists: it has been opened on this computer.
    pub found: bool,
    /// Narrivy's host is registered with it.
    pub registered: bool,
    pub extensions_page: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserConnectorDto {
    /// The package's host is installed (a development checkout has none).
    pub host_installed: bool,
    /// Where the extension to load unpacked is, when installed.
    pub extension_dir: Option<String>,
    pub browsers: Vec<BrowserDto>,
}

fn manifest(host: &str) -> String {
    serde_json::json!({
        "name": HOST_NAME,
        "description": "Narrivy writing assistant",
        "path": host,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    })
    .to_string()
}

/// Whether the manifest at `path` is Narrivy's host for Narrivy's extension.
fn is_ours(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| {
            value.get("name").and_then(|v| v.as_str()) == Some(HOST_NAME)
                && value
                    .get("allowed_origins")
                    .and_then(|v| v.as_array())
                    .is_some_and(|origins| {
                        origins.iter().any(|origin| {
                            origin.as_str() == Some(&format!("chrome-extension://{EXTENSION_ID}/"))
                        })
                    })
        })
}

const NOT_INSTALLED: &str = "The browser connector is not installed with this copy of Narrivy.";

/// "No Chrome, Chromium or Brave found. …", naming this system's browsers.
fn none_found() -> String {
    let names: Vec<&str> = BROWSERS
        .iter()
        .map(|browser| browser.name.trim_start_matches("Google "))
        .collect();
    let list = match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => names.concat(),
    };
    format!("No {list} found. Open one once, then try again.")
}

pub use platform::{get, set};

#[cfg(not(windows))]
mod platform {
    use std::path::{Path, PathBuf};

    use super::{
        BROWSERS, BrowserConnectorDto, BrowserDto, HOST_NAME, NOT_INSTALLED, is_ours, manifest,
        none_found,
    };

    pub(super) const HOST_PATH: &str = "/usr/lib/narrivy/narrivy-browser-host";
    const EXTENSION_DIR: &str = "/usr/share/narrivy/browser-extension";

    fn config_home() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }

    pub(super) fn manifest_path(profile: &Path) -> PathBuf {
        profile
            .join("NativeMessagingHosts")
            .join(format!("{HOST_NAME}.json"))
    }

    pub(super) fn status_under(config: Option<&Path>, host_installed: bool) -> BrowserConnectorDto {
        let browsers = BROWSERS
            .iter()
            .map(|browser| {
                let profile = config.map(|config| config.join(browser.folder));
                BrowserDto {
                    id: browser.id,
                    name: browser.name,
                    found: profile.as_deref().is_some_and(Path::is_dir),
                    registered: profile
                        .as_deref()
                        .is_some_and(|profile| is_ours(&manifest_path(profile))),
                    extensions_page: browser.extensions_page,
                }
            })
            .collect();
        BrowserConnectorDto {
            host_installed,
            extension_dir: Path::new(EXTENSION_DIR)
                .is_dir()
                .then(|| EXTENSION_DIR.to_owned()),
            browsers,
        }
    }

    pub(super) fn set_under(config: &Path, on: bool, host: &str) -> Result<(), String> {
        let mut registered = 0;
        for browser in BROWSERS {
            let profile = config.join(browser.folder);
            let path = manifest_path(&profile);
            if on {
                if !profile.is_dir() {
                    continue;
                }
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("browser folder: {e}"))?;
                }
                std::fs::write(&path, manifest(host))
                    .map_err(|e| format!("browser connector: {e}"))?;
                registered += 1;
            } else if is_ours(&path) {
                // Only a manifest for Narrivy's extension is removed.
                std::fs::remove_file(&path).map_err(|e| format!("browser connector: {e}"))?;
            }
        }
        if on && registered == 0 {
            return Err(none_found());
        }
        Ok(())
    }

    /// What is registered now.
    #[must_use]
    pub fn get() -> BrowserConnectorDto {
        status_under(config_home().as_deref(), Path::new(HOST_PATH).is_file())
    }

    /// Register the host with every browser found (`on`), or remove it.
    ///
    /// # Errors
    /// A short message when the host is not installed, no browser is found, or
    /// a manifest cannot be written or removed.
    pub fn set(on: bool) -> Result<BrowserConnectorDto, String> {
        let config = config_home().ok_or("no home folder")?;
        if on && !Path::new(HOST_PATH).is_file() {
            return Err(NOT_INSTALLED.into());
        }
        set_under(&config, on, HOST_PATH)?;
        Ok(get())
    }
}

#[cfg(windows)]
mod platform {
    use std::path::{Path, PathBuf};

    use super::{
        BROWSERS, BrowserConnectorDto, BrowserDto, HOST_NAME, NOT_INSTALLED, is_ours, none_found,
    };
    use crate::windows_reg::{reg, string_value};

    const HOST_FILE: &str = "narrivy-browser-host.exe";
    const EXTENSION_FOLDER: &str = "browser-extension";

    /// The install folder: the one this program runs from.
    fn install_dir() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()?
            .parent()
            .map(Path::to_path_buf)
            .filter(|dir| dir.is_absolute())
    }

    fn local_app_data() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    }

    /// The one manifest every browser's key names.
    fn manifest_path(local: &Path) -> PathBuf {
        local
            .join("Narrivy")
            .join("browser-connector")
            .join(format!("{HOST_NAME}.json"))
    }

    /// The manifest `key` names, when it names one.
    fn registered_manifest(key: &str) -> Option<String> {
        string_value(&reg(&["query", &format!(r"{key}\{HOST_NAME}"), "/ve"])?)
    }

    /// `key` names Narrivy's manifest for Narrivy's extension.
    fn key_is_ours(key: &str, manifest: &Path) -> bool {
        registered_manifest(key).is_some_and(|named| Path::new(&named) == manifest)
            && is_ours(manifest)
    }

    /// What is registered now.
    #[must_use]
    pub fn get() -> BrowserConnectorDto {
        let local = local_app_data();
        let manifest = local.as_deref().map(manifest_path);
        let browsers = BROWSERS
            .iter()
            .map(|browser| {
                let found = local
                    .as_deref()
                    .is_some_and(|local| local.join(browser.folder).is_dir());
                BrowserDto {
                    id: browser.id,
                    name: browser.name,
                    found,
                    registered: found
                        && manifest.as_deref().is_some_and(|manifest| {
                            browser
                                .keys
                                .first()
                                .is_some_and(|key| key_is_ours(key, manifest))
                        }),
                    extensions_page: browser.extensions_page,
                }
            })
            .collect();
        let install = install_dir();
        BrowserConnectorDto {
            host_installed: install
                .as_deref()
                .is_some_and(|dir| dir.join(HOST_FILE).is_file()),
            extension_dir: install
                .map(|dir| dir.join(EXTENSION_FOLDER))
                .filter(|dir| dir.is_dir())
                .and_then(|dir| dir.to_str().map(str::to_owned)),
            browsers,
        }
    }

    /// Register the host with every browser found (`on`), or remove it.
    ///
    /// # Errors
    /// A short message when the host is not installed, no browser is found,
    /// or the manifest or a key cannot be written or removed.
    pub fn set(on: bool) -> Result<BrowserConnectorDto, String> {
        let local = local_app_data().ok_or("no local app data folder")?;
        let manifest = manifest_path(&local);
        let manifest_text = manifest.to_str().ok_or("the app data path is not text")?;
        if on {
            let host = install_dir()
                .map(|dir| dir.join(HOST_FILE))
                .filter(|host| host.is_file())
                .ok_or(NOT_INSTALLED)?;
            let host = host.to_str().ok_or("the install path is not text")?;
            let found: Vec<_> = BROWSERS
                .iter()
                .filter(|browser| local.join(browser.folder).is_dir())
                .collect();
            if found.is_empty() {
                return Err(none_found());
            }
            if let Some(dir) = manifest.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("browser connector: {e}"))?;
            }
            std::fs::write(&manifest, super::manifest(host))
                .map_err(|e| format!("browser connector: {e}"))?;
            for browser in found {
                for key in browser.keys {
                    reg(&[
                        "add",
                        &format!(r"{key}\{HOST_NAME}"),
                        "/ve",
                        "/t",
                        "REG_SZ",
                        "/d",
                        manifest_text,
                        "/f",
                    ])
                    .ok_or_else(|| format!("{} could not be connected", browser.name))?;
                }
            }
        } else {
            // Only keys naming Narrivy's manifest are removed.
            for key in BROWSERS.iter().flat_map(|browser| browser.keys) {
                if registered_manifest(key).is_some_and(|named| Path::new(&named) == manifest) {
                    reg(&["delete", &format!(r"{key}\{HOST_NAME}"), "/f"])
                        .ok_or("the browser connector could not be removed")?;
                }
            }
            if is_ours(&manifest) {
                std::fs::remove_file(&manifest).map_err(|e| format!("browser connector: {e}"))?;
            }
        }
        Ok(get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[cfg(not(windows))]
    use platform::{HOST_PATH, manifest_path, set_under, status_under};

    #[test]
    fn the_no_browser_message_names_this_systems_browsers() {
        let message = none_found();
        assert!(message.starts_with("No Brave, Chrome"));
        assert!(message.contains(" or "));
        assert!(!message.contains("Google"));
        assert!(message.ends_with(" found. Open one once, then try again."));
    }

    #[cfg(not(windows))]
    #[test]
    fn registering_writes_only_for_browsers_found_and_removing_only_ours() -> TestResult {
        let config = tempfile::tempdir()?;
        let brave = config.path().join("BraveSoftware/Brave-Browser");
        std::fs::create_dir_all(&brave)?;
        // Nothing found but Brave.
        set_under(config.path(), true, "/usr/lib/narrivy/narrivy-browser-host")?;
        let status = status_under(Some(config.path()), true);
        let registered: Vec<&str> = status
            .browsers
            .iter()
            .filter(|b| b.registered)
            .map(|b| b.id)
            .collect();
        assert_eq!(registered, ["brave"]);
        assert!(!config.path().join("google-chrome").exists());
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(manifest_path(&brave))?)?;
        assert_eq!(written["path"], "/usr/lib/narrivy/narrivy-browser-host");
        assert_eq!(
            written["allowed_origins"][0],
            "chrome-extension://khkbeljpdjbgihgbedobeaoedkdffkie/"
        );

        // Someone else's manifest under the same name stays.
        let chromium = config.path().join("chromium");
        std::fs::create_dir_all(chromium.join("NativeMessagingHosts"))?;
        std::fs::write(
            manifest_path(&chromium),
            r#"{"name":"com.narrivy.writing"}"#,
        )?;
        set_under(config.path(), false, HOST_PATH)?;
        assert!(!manifest_path(&brave).exists());
        assert!(manifest_path(&chromium).exists());
        Ok(())
    }

    #[cfg(not(windows))]
    #[test]
    fn turning_on_with_no_browser_says_so() -> TestResult {
        let config = tempfile::tempdir()?;
        assert!(set_under(config.path(), true, HOST_PATH).is_err());
        Ok(())
    }

    #[test]
    fn the_id_is_the_one_the_extension_key_gives() -> TestResult {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../narrivy-browser-extension/manifest.json"
        ))?;
        assert!(manifest["key"].as_str().is_some_and(|key| !key.is_empty()));
        assert_eq!(EXTENSION_ID.len(), 32);
        assert!(EXTENSION_ID.bytes().all(|b| (b'a'..=b'p').contains(&b)));
        Ok(())
    }
}
