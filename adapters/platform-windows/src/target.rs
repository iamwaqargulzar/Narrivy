//! Naming the foreground application from what Windows reports about it.
//!
//! The OS calls (foreground window, its title, its process image path) live
//! in `os::target`; this module turns the image path into the portable
//! [`ApplicationIdentity`], so the rule is tested everywhere.
//!
//! Target identity is a routing input. Callers must not log it.

#![forbid(unsafe_code)]

use narrivy_types::ApplicationIdentity;

/// Longest window title kept. Titles are user content (document names, chat
/// partners); a bound keeps one pathological title from bloating a session.
pub const MAX_TITLE_CHARS: usize = 512;

/// Identity from a process image path such as
/// `C:\Windows\System32\notepad.exe`: the executable's file name, lowercased,
/// is the stable id (`notepad.exe`); its stem is the display name.
#[must_use]
pub fn application_identity(image_path: &str) -> Option<ApplicationIdentity> {
    let file = image_path.rsplit(['\\', '/']).next()?.trim();
    if file.is_empty() {
        return None;
    }
    let stem = match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file,
    };
    Some(ApplicationIdentity {
        app_id: file.to_lowercase(),
        display_name: stem.to_owned(),
    })
}

/// Trim a window title to [`MAX_TITLE_CHARS`]; an empty title is absent.
#[must_use]
pub fn bounded_title(title: &str) -> Option<String> {
    let title = title.trim();
    (!title.is_empty()).then(|| title.chars().take(MAX_TITLE_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_comes_from_the_executable_name() {
        let identity = application_identity(r"C:\Program Files\Mozilla Firefox\Firefox.EXE");
        assert_eq!(
            identity,
            Some(ApplicationIdentity {
                app_id: "firefox.exe".to_owned(),
                display_name: "Firefox".to_owned(),
            })
        );
        assert_eq!(
            application_identity("notepad").map(|i| i.app_id),
            Some("notepad".to_owned())
        );
        assert_eq!(application_identity(r"C:\dir\"), None);
        assert_eq!(application_identity(""), None);
    }

    #[test]
    fn titles_are_trimmed_and_bounded() {
        assert_eq!(bounded_title("  "), None);
        assert_eq!(bounded_title(" a ").as_deref(), Some("a"));
        let long = "é".repeat(MAX_TITLE_CHARS + 10);
        assert_eq!(
            bounded_title(&long).map(|t| t.chars().count()),
            Some(MAX_TITLE_CHARS)
        );
    }
}
