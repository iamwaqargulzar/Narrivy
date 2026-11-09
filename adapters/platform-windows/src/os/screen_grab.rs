//! Pictures of every monitor through GDI, for `screen_keeper`: the
//! composed desktop as the user sees it, shrunk by GDI itself to at most
//! the kept width, so a 4K screen is never copied whole.

use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetDIBits, HALFTONE, HDC,
    HGDIOBJ, HMONITOR, ReleaseDC, SRCCOPY, SelectObject, SetBrushOrgEx, SetStretchBltMode,
    StretchBlt,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::core::BOOL;

use crate::screen_keeper::Picture;

/// Every monitor now, left to right as Windows lists them, at most
/// `max_width` wide.
///
/// # Errors
/// `screen_capture_failed` when the desktop can't be read (locked, or the
/// secure desktop is up).
pub fn grab_monitors(max_width: u32) -> Result<Vec<Picture>, &'static str> {
    // Real pixels, not the scaled ones a DPI-unaware thread is shown.
    // SAFETY: a predefined context; only this thread's awareness changes.
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut monitors: Vec<RECT> = Vec::new();
    // SAFETY: the callback only pushes into `monitors`, which outlives the
    // call (passed as LPARAM); no clipping DC or rectangle.
    let listed = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(std::ptr::from_mut(&mut monitors) as isize),
        )
    };
    if !listed.as_bool() || monitors.is_empty() {
        return Err("screen_capture_failed");
    }
    monitors.sort_by_key(|rect| (rect.left, rect.top));
    monitors.iter().map(|rect| grab(rect, max_width)).collect()
}

unsafe extern "system" fn collect_monitor(
    _monitor: HMONITOR,
    _hdc: HDC,
    rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the `Vec<RECT>` passed by `grab_monitors`, alive for
    // the enumeration; `rect` is the monitor's rectangle, valid during the
    // callback.
    unsafe {
        if let (Some(monitors), Some(rect)) = ((data.0 as *mut Vec<RECT>).as_mut(), rect.as_ref()) {
            monitors.push(*rect);
        }
    }
    BOOL(1)
}

/// The screen DC, released when dropped.
struct ScreenDc(HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: from GetDC(None), released once.
        unsafe {
            ReleaseDC(None, self.0);
        }
    }
}

/// A memory DC with a bitmap selected into it, both freed when dropped.
struct Canvas {
    dc: HDC,
    bitmap: HGDIOBJ,
    previous: HGDIOBJ,
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: the bitmap is deselected before it is deleted; each
        // handle came from a successful create and is freed once.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

fn grab(rect: &RECT, max_width: u32) -> Result<Picture, &'static str> {
    let failed = "screen_capture_failed";
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    if width <= 0 || height <= 0 {
        return Err(failed);
    }
    let out_width = width.min(i32::try_from(max_width).map_err(|_| failed)?);
    let out_height = i32::try_from(i64::from(height) * i64::from(out_width) / i64::from(width))
        .map_err(|_| failed)?
        .max(1);
    // SAFETY: the whole screen's DC (no window), released by the guard.
    let screen = ScreenDc(unsafe { GetDC(None) });
    if screen.0.is_invalid() {
        return Err(failed);
    }
    // SAFETY: a valid screen DC; the results are checked and owned by the
    // canvas guard.
    let canvas = unsafe {
        let dc = CreateCompatibleDC(Some(screen.0));
        if dc.is_invalid() {
            return Err(failed);
        }
        let bitmap = CreateCompatibleBitmap(screen.0, out_width, out_height);
        if bitmap.is_invalid() {
            let _ = DeleteDC(dc);
            return Err(failed);
        }
        let bitmap = HGDIOBJ(bitmap.0);
        let previous = SelectObject(dc, bitmap);
        Canvas {
            dc,
            bitmap,
            previous,
        }
    };
    // SAFETY: valid DCs; the source rectangle is a monitor's; HALFTONE
    // shrinks with averaging (it needs the brush origin set after).
    let copied = unsafe {
        SetStretchBltMode(canvas.dc, HALFTONE);
        let _ = SetBrushOrgEx(canvas.dc, 0, 0, None);
        StretchBlt(
            canvas.dc,
            0,
            0,
            out_width,
            out_height,
            Some(screen.0),
            rect.left,
            rect.top,
            width,
            height,
            SRCCOPY,
        )
    };
    if !copied.as_bool() {
        return Err(failed);
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>()).map_err(|_| failed)?,
            biWidth: out_width,
            // Negative: rows top to bottom.
            biHeight: -out_height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let (w, h) = (
        usize::try_from(out_width).map_err(|_| failed)?,
        usize::try_from(out_height).map_err(|_| failed)?,
    );
    let mut bgra = vec![0_u8; w * h * 4];
    // SAFETY: the bitmap is selected into a DC GetDIBits may read it from;
    // `bgra` holds exactly `out_height` rows of 32-bit pixels as `info`
    // describes; `info` is a live local.
    let rows = unsafe {
        GetDIBits(
            canvas.dc,
            windows::Win32::Graphics::Gdi::HBITMAP(canvas.bitmap.0),
            0,
            u32::try_from(out_height).map_err(|_| failed)?,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if rows != out_height {
        return Err(failed);
    }
    let rgb = bgra
        .chunks_exact(4)
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
        .collect();
    Ok(Picture {
        width: u32::try_from(out_width).map_err(|_| failed)?,
        height: u32::try_from(out_height).map_err(|_| failed)?,
        rgb,
    })
}
