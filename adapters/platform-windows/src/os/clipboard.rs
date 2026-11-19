//! The Windows clipboard for the paste route: snapshot, write, ownership
//! check and restore (the policy is in [`crate::delivery`]).
//!
//! The clipboard is opened with a message-only window of our own as owner
//! (opening it with no window makes `SetClipboardData` unreliable), with a
//! short bounded retry because another application may hold it open.
//!
//! The snapshot copies every format held in global memory, which covers
//! text, rich text, HTML, file lists and device-independent bitmaps
//! (`CF_BITMAP` and `CF_PALETTE` are re-created by Windows from `CF_DIB`).
//! A clipboard holding formats that cannot be copied this way (metafiles,
//! owner-drawn or display formats, GDI objects) or more than
//! [`MAX_SNAPSHOT_BYTES`] is refused: the paste route then sends nothing
//! rather than lose the user's clipboard.
//!
//! The text we put on the clipboard is marked to stay out of clipboard
//! history (Win+V), cloud clipboard sync and clipboard monitors.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::{PCWSTR, w};

use crate::delivery::ClipboardPort;

/// Largest clipboard the paste route will save and restore.
pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const OPEN_BOUND: Duration = Duration::from_millis(500);

const CF_BITMAP: u32 = 2;
const CF_METAFILEPICT: u32 = 3;
const CF_PALETTE: u32 = 9;
const CF_UNICODETEXT: u32 = 13;
const CF_ENHMETAFILE: u32 = 14;
const CF_OWNERDISPLAY: u32 = 0x0080;
const CF_DSPTEXT: u32 = 0x0081;
const CF_DSPENHMETAFILE: u32 = 0x008E;
const CF_PRIVATEFIRST: u32 = 0x0200;
const CF_PRIVATELAST: u32 = 0x02FF;
const CF_GDIOBJFIRST: u32 = 0x0300;
const CF_GDIOBJLAST: u32 = 0x03FF;

/// How one format is treated by the snapshot.
fn format_class(format: u32) -> FormatClass {
    match format {
        // Re-synthesised by Windows from CF_DIB on restore.
        CF_BITMAP | CF_PALETTE => FormatClass::Skip,
        // Handles only meaningful to the application that put them there.
        CF_PRIVATEFIRST..=CF_PRIVATELAST => FormatClass::Skip,
        CF_METAFILEPICT | CF_ENHMETAFILE | CF_OWNERDISPLAY => FormatClass::Unsupported,
        CF_DSPTEXT..=CF_DSPENHMETAFILE => FormatClass::Unsupported,
        CF_GDIOBJFIRST..=CF_GDIOBJLAST => FormatClass::Unsupported,
        _ => FormatClass::Memory,
    }
}

enum FormatClass {
    Memory,
    Skip,
    Unsupported,
}

/// The user's clipboard, copied. Holds user content: never logged, and not
/// `Debug`.
pub struct ClipboardSnapshot {
    formats: Vec<(u32, Vec<u8>)>,
}

/// The clipboard opened by us. Closes and destroys its window on drop.
struct OpenClipboardGuard {
    window: HWND,
}

impl OpenClipboardGuard {
    fn open() -> Result<Self, &'static str> {
        // SAFETY: the predefined STATIC class needs no registration; a
        // message-only window (parent HWND_MESSAGE) is never shown. It is
        // destroyed by this guard on the same thread.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|_| "clipboard_window_failed")?;
        let deadline = Instant::now() + OPEN_BOUND;
        loop {
            // SAFETY: `window` is a live window owned by this thread.
            if unsafe { OpenClipboard(Some(window)) }.is_ok() {
                return Ok(Self { window });
            }
            if Instant::now() >= deadline {
                // SAFETY: our window, destroyed once.
                let _ = unsafe { DestroyWindow(window) };
                return Err("clipboard_busy");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: we opened the clipboard; our window, destroyed once.
        unsafe {
            let _ = CloseClipboard();
            let _ = DestroyWindow(self.window);
        }
    }
}

/// Copy a global-memory block. `None` when it cannot be read.
fn read_global(handle: HANDLE) -> Option<Vec<u8>> {
    let global = HGLOBAL(handle.0);
    // SAFETY: a clipboard data handle owned by the clipboard, valid while
    // it is open; locked, copied and unlocked within this function.
    unsafe {
        let size = GlobalSize(global);
        let pointer = GlobalLock(global);
        if pointer.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(global);
        Some(bytes)
    }
}

/// Hand `bytes` to the clipboard as `format`. The clipboard owns the memory
/// on success; it is freed here on failure.
fn set_global(format: u32, bytes: &[u8]) -> Result<(), &'static str> {
    // SAFETY: a fresh movable block of at least one byte, filled while
    // locked and then given to the open clipboard, or freed.
    unsafe {
        let global =
            GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|_| "clipboard_write_failed")?;
        let pointer = GlobalLock(global);
        if pointer.is_null() {
            let _ = GlobalFree(Some(global));
            return Err("clipboard_write_failed");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(global);
        if SetClipboardData(format, Some(HANDLE(global.0))).is_err() {
            let _ = GlobalFree(Some(global));
            return Err("clipboard_write_failed");
        }
    }
    Ok(())
}

fn utf16_nul(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Formats that keep our text out of clipboard history, cloud sync and
/// monitors. Registered names are system-wide and stable.
fn privacy_formats() -> [(u32, &'static [u8]); 3] {
    // SAFETY: static NUL-terminated names.
    unsafe {
        [
            (
                RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
                &[0, 0, 0, 0],
            ),
            (
                RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
                &[0, 0, 0, 0],
            ),
            (
                RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")),
                &[0, 0, 0, 0],
            ),
        ]
    }
}

/// The real clipboard.
#[derive(Debug, Clone, Copy, Default)]
pub struct Win32Clipboard;

impl ClipboardPort for Win32Clipboard {
    type Snapshot = ClipboardSnapshot;

    fn snapshot(&mut self) -> Result<ClipboardSnapshot, &'static str> {
        let _open = OpenClipboardGuard::open()?;
        let mut formats = Vec::new();
        let mut total = 0_usize;
        let mut format = 0_u32;
        loop {
            // SAFETY: the clipboard is open on this thread.
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            match format_class(format) {
                FormatClass::Skip => continue,
                FormatClass::Unsupported => return Err("clipboard_snapshot_unsupported_format"),
                FormatClass::Memory => {}
            }
            // SAFETY: the clipboard is open; the handle stays owned by it.
            let handle =
                unsafe { GetClipboardData(format) }.map_err(|_| "clipboard_snapshot_incomplete")?;
            let bytes = read_global(handle).ok_or("clipboard_snapshot_incomplete")?;
            total = total.saturating_add(bytes.len());
            if total > MAX_SNAPSHOT_BYTES {
                return Err("clipboard_snapshot_too_large");
            }
            formats.push((format, bytes));
        }
        Ok(ClipboardSnapshot { formats })
    }

    fn write(&mut self, text: &str) -> Result<u32, &'static str> {
        let _open = OpenClipboardGuard::open()?;
        // SAFETY: the clipboard is open with our window as owner.
        unsafe { EmptyClipboard() }.map_err(|_| "clipboard_write_failed")?;
        set_global(CF_UNICODETEXT, &utf16_nul(text))?;
        for (format, value) in privacy_formats() {
            if format != 0 {
                // Best effort: the paste works without the markers.
                let _ = set_global(format, value);
            }
        }
        // SAFETY: plain query; no one else can change the clipboard while it
        // is open.
        Ok(unsafe { GetClipboardSequenceNumber() })
    }

    fn still_owned(&mut self, token: u32, text: &str) -> bool {
        // SAFETY: plain query.
        if unsafe { GetClipboardSequenceNumber() } == token {
            return true;
        }
        // Some readers make Windows synthesise formats, which may count as a
        // change; the content tells whether it is still our text.
        let Ok(_open) = OpenClipboardGuard::open() else {
            return false;
        };
        // SAFETY: the clipboard is open.
        let Ok(handle) = (unsafe { GetClipboardData(CF_UNICODETEXT) }) else {
            return false;
        };
        read_global(handle).is_some_and(|bytes| bytes.starts_with(&utf16_nul(text)))
    }

    fn restore(&mut self, snapshot: &ClipboardSnapshot) -> Result<(), &'static str> {
        let _open = OpenClipboardGuard::open()?;
        // SAFETY: the clipboard is open with our window as owner.
        unsafe { EmptyClipboard() }.map_err(|_| "clipboard_restore_failed")?;
        for (format, bytes) in &snapshot.formats {
            set_global(*format, bytes).map_err(|_| "clipboard_restore_failed")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Touches the real clipboard: run on the Windows test machine with
    /// `--ignored`, with nothing important on the clipboard.
    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn write_then_restore_round_trips() {
        let mut clipboard = Win32Clipboard;
        assert!(
            clipboard
                .restore(&ClipboardSnapshot {
                    formats: vec![(CF_UNICODETEXT, utf16_nul("before"))],
                })
                .is_ok()
        );
        let snapshot = match clipboard.snapshot() {
            Ok(snapshot) => snapshot,
            Err(code) => unreachable!("{code}"),
        };
        let token = match clipboard.write("dictated 😀") {
            Ok(token) => token,
            Err(code) => unreachable!("{code}"),
        };
        assert!(clipboard.still_owned(token, "dictated 😀"));
        assert!(clipboard.restore(&snapshot).is_ok());
        assert!(!clipboard.still_owned(token, "dictated 😀"));
    }

    #[test]
    fn utf16_text_is_nul_terminated() {
        assert_eq!(utf16_nul("a"), [0x61, 0, 0, 0]);
    }
}
