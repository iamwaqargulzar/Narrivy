//! Text options: how the Clean pipeline writes its final text.
//!
//! - **Numbers as digits** ("twenty five" as 25, see `narrivyd::numbers`): on
//!   unless turned off. The Raw pipeline is never changed.
//! - **Delivery method**: `paste` (the clipboard and Shift+Insert, then the
//!   user's clipboard put back) or `type` (the virtual keyboard). Until the
//!   user chooses, the daemon's composition decides
//!   (`NARRIVY_CLIPBOARD_AUTHORIZED`). Either way, a dictation that cannot be
//!   delivered is kept in the Inbox.
//!
//! Kept in `text.json` in the state directory.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Options {
    #[serde(default = "yes")]
    numbers_as_digits: bool,
    /// `paste` or `type`; `None` until the user chooses.
    #[serde(default)]
    delivery: Option<DeliveryMethod>,
    /// Unload the clean-up model after this many idle minutes; 0 never.
    #[serde(default)]
    cleanup_idle_minutes: u32,
    /// "new line" / "new paragraph" become line breaks (Clean only).
    #[serde(default)]
    spoken_breaks: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeliveryMethod {
    Paste,
    Type,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            numbers_as_digits: true,
            delivery: None,
            cleanup_idle_minutes: 0,
            spoken_breaks: false,
        }
    }
}

fn read() -> Options {
    path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Options>(&bytes).ok())
        .unwrap_or_default()
}

fn write(options: &Options) -> Result<(), &'static str> {
    let path = path().ok_or("text_options_unavailable")?;
    let bytes = serde_json::to_vec(options).map_err(|_| "text_options_unavailable")?;
    std::fs::write(path, bytes).map_err(|_| "text_options_unavailable")
}

/// Whether to paste: the user's choice, else `composed`.
#[must_use]
pub fn paste(composed: bool) -> bool {
    read()
        .delivery
        .map_or(composed, |method| method == DeliveryMethod::Paste)
}

/// Minutes of no use after which the clean-up model is unloaded; 0 never.
#[must_use]
pub fn cleanup_idle_minutes() -> u32 {
    read().cleanup_idle_minutes
}

/// # Errors
/// `cleanup_idle_invalid` past a day, or `text_options_unavailable`.
pub fn set_cleanup_idle_minutes(minutes: u32) -> Result<(), &'static str> {
    if minutes > 24 * 60 {
        return Err("cleanup_idle_invalid");
    }
    let mut options = read();
    options.cleanup_idle_minutes = minutes;
    write(&options)
}

/// # Errors
/// `text_options_unavailable` when it cannot be saved.
pub fn set_spoken_breaks(on: bool) -> Result<(), &'static str> {
    let mut options = read();
    options.spoken_breaks = on;
    write(&options)
}

/// Choose `paste` or `type`.
///
/// # Errors
/// `delivery_method_unknown`, or `text_options_unavailable`.
pub fn set_delivery(method: &str) -> Result<(), &'static str> {
    let method = match method {
        "paste" => DeliveryMethod::Paste,
        "type" => DeliveryMethod::Type,
        _ => return Err("delivery_method_unknown"),
    };
    let mut options = read();
    options.delivery = Some(method);
    write(&options)
}

const fn yes() -> bool {
    true
}

fn path() -> Option<std::path::PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("text.json"))
}

#[must_use]
pub fn numbers_as_digits() -> bool {
    read().numbers_as_digits
}

/// # Errors
/// `text_options_unavailable` when the choice cannot be saved.
pub fn set_numbers_as_digits(on: bool) -> Result<(), &'static str> {
    let mut options = read();
    options.numbers_as_digits = on;
    write(&options)
}

/// `composed`: the daemon's own clipboard authorization, for the method in
/// use until the user chooses.
#[must_use]
pub fn dto(composed: bool) -> narrivy_ipc::TextOptionsDto {
    narrivy_ipc::TextOptionsDto {
        numbers_as_digits: numbers_as_digits(),
        delivery_method: if paste(composed) { "paste" } else { "type" }.to_owned(),
        cleanup_idle_minutes: cleanup_idle_minutes(),
        spoken_breaks: read().spoken_breaks,
    }
}

/// The Clean pipeline's final step.
#[must_use]
pub fn finish_clean(text: String) -> String {
    let options = read();
    let text = if options.numbers_as_digits {
        narrivyd::numbers::digits(&text)
    } else {
        text
    };
    if options.spoken_breaks {
        narrivyd::breaks::line_breaks(&text)
    } else {
        text
    }
}
