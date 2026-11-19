//! The foreground window as the dictation target.

use narrivy_platform_api::{PlatformError, TargetProvider};
use narrivy_types::TargetContext;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

use crate::error;
use crate::target::{MAX_TITLE_CHARS, application_identity, bounded_title};

/// Reports the foreground window's application and title. An observer: it
/// never activates, focuses or sends input.
#[derive(Debug, Clone, Copy, Default)]
pub struct ForegroundTarget;

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: a handle we opened, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn image_path(pid: u32) -> Option<String> {
    // SAFETY: limited query access is enough for the image name and is
    // granted for most processes of the same user, elevated ones included.
    let process =
        ProcessHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    let mut buffer = vec![0_u16; 1_024];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `buffer` holds `length` u16s; on success `length` is the
    // number written, without the terminator.
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .ok()?;
    buffer.truncate(length as usize);
    String::from_utf16(&buffer).ok()
}

impl TargetProvider for ForegroundTarget {
    fn capture_target(&self) -> Result<TargetContext, PlatformError> {
        // SAFETY: plain query.
        let window = unsafe { GetForegroundWindow() };
        if window.is_invalid() {
            return Err(error("target_unavailable"));
        }
        let mut title = vec![0_u16; MAX_TITLE_CHARS * 2 + 1];
        // SAFETY: writes at most `title.len()` u16s including the
        // terminator. For another process's window it reads the cached
        // caption without sending a message, so it cannot hang.
        let copied = unsafe { GetWindowTextW(window, &mut title) };
        title.truncate(usize::try_from(copied).unwrap_or(0));
        let mut pid = 0_u32;
        // SAFETY: out-pointer to a live local.
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        let application = (pid != 0)
            .then(|| image_path(pid))
            .flatten()
            .and_then(|path| application_identity(&path));
        Ok(TargetContext {
            application,
            window_title: bounded_title(&String::from_utf16_lossy(&title)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs an interactive desktop"]
    fn reports_the_foreground_application() {
        let target = ForegroundTarget.capture_target();
        assert!(target.is_ok_and(|target| target.application.is_some()));
    }
}
