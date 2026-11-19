//! Pictures of the screen during a meeting ("Watch with me"), kept the way
//! the Linux helper (`scripts/screen_capture.py`) keeps them, into the same
//! files, so the app and the meeting's summary read them alike:
//!
//! - `frame-0001.png` (up to [`WIDTH`] px wide, for the model) and
//!   `thumb-0001.png` ([`THUMB`] px, for the app), one line each in
//!   `frames.jsonl` (`file`, `thumb`, `at_ms` since the meeting's start,
//!   `source`);
//! - `sources.json`: each screen watched (kind, width, height);
//! - `live-N.png`: what screen N shows now, small, every second, and
//!   `now-N.png`, larger, every few seconds, for the assistant.
//!
//! A picture is kept only when the screen changed from the last kept one
//! and then stayed the same for a moment (a new slide, a new page), or, for
//! something that keeps moving (a video), at most every `moving_every`.
//!
//! Windows has no screen-sharing dialog for a program with no window, so
//! every monitor is watched (each is one source); the meeting's own "screen
//! on" switch is the user's choice. The pictures come from a grabber
//! ([`Grab`]): `os::grab_monitors` in the service, a fake in tests. One
//! thread owns it; [`ScreenKeeper::stop`] ends it within one tick.

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Kept pictures are at most this wide (as on Linux).
pub const WIDTH: u32 = 1024;
/// The app's thumbnails.
pub const THUMB: u32 = 480;
/// The live picture of each screen.
const LIVE: u32 = 320;
const LIVE_EVERY: Duration = Duration::from_secs(1);
const NOW_EVERY: Duration = Duration::from_secs(3);
/// A sample counts as changed above this difference (0..255), and the
/// screen as changed when this share of samples did.
const PIXEL_DIFF: u8 = 28;
const CHANGED_SHARE: f32 = 0.03;
const GRID: (u32, u32) = (64, 36);

/// One screen's picture, 8-bit RGB, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Picture {
    fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.rgb.len() == self.width as usize * self.height as usize * 3
    }
}

/// Takes a picture of every screen now (at most [`WIDTH`] wide), in a
/// stable order; an error code when the screen cannot be read (the desktop
/// is locked, for one), and the keeper tries again on the next tick.
pub type Grab = Box<dyn FnMut() -> Result<Vec<Picture>, &'static str> + Send>;

/// How a meeting keeps its screens.
#[derive(Debug, Clone, Copy)]
pub struct KeepSettings {
    /// How often the screen is compared with the last kept picture.
    pub interval: Duration,
    /// Something that keeps moving is kept at most this often.
    pub moving_every: Duration,
    /// The meeting's start, Unix ms (`at_ms` counts from it).
    pub started_unix_ms: u64,
}

/// What one look at a screen calls for.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Look {
    live: bool,
    large: bool,
    keep: bool,
}

/// One screen's comparison state.
struct Watch {
    kept: Option<Vec<u8>>,
    candidate: Option<Vec<u8>>,
    since_kept: Instant,
    live: Option<Instant>,
    now: Option<Instant>,
    looked: Option<Instant>,
}

impl Watch {
    fn new(now: Instant) -> Self {
        Self {
            kept: None,
            candidate: None,
            since_kept: now,
            live: None,
            now: None,
            looked: None,
        }
    }

    /// Compare `grid` with the last kept picture at `now`: a change is kept
    /// once it has settled, something that keeps moving now and then.
    fn look(&mut self, now: Instant, grid: Vec<u8>, settings: &KeepSettings) -> Look {
        let since = |at: Option<Instant>| at.map_or(Duration::MAX, |at| now.duration_since(at));
        let mut look = Look::default();
        // A little early still counts: pictures come about once a second.
        if since(self.live) >= LIVE_EVERY.mul_f32(0.8) {
            self.live = Some(now);
            look.live = true;
            if since(self.now) >= NOW_EVERY.mul_f32(0.9) {
                self.now = Some(now);
                look.large = true;
            }
        }
        if since(self.looked) < settings.interval.mul_f32(0.9) {
            return look;
        }
        self.looked = Some(now);
        if changed(self.kept.as_deref(), &grid) {
            let settled = self
                .candidate
                .as_deref()
                .is_some_and(|candidate| !changed(Some(candidate), &grid));
            look.keep = settled || now.duration_since(self.since_kept) > settings.moving_every;
            self.candidate = Some(grid.clone());
        } else {
            self.candidate = None;
        }
        if look.keep {
            self.kept = Some(grid);
            self.candidate = None;
            self.since_kept = now;
        }
        look
    }
}

/// A small grid of luminance samples (as the Linux helper takes them).
fn samples(picture: &Picture) -> Vec<u8> {
    let (gx, gy) = GRID;
    let (width, height) = (picture.width as usize, picture.height as usize);
    let mut out = Vec::with_capacity((gx * gy) as usize);
    for j in 0..gy as usize {
        let row = (j * height / gy as usize) * width * 3;
        for i in 0..gx as usize {
            let k = row + (i * width / gx as usize) * 3;
            let (r, g, b) = (
                u32::from(picture.rgb[k]),
                u32::from(picture.rgb[k + 1]),
                u32::from(picture.rgb[k + 2]),
            );
            out.push(u8::try_from((r * 3 + g * 6 + b) / 10).unwrap_or(u8::MAX));
        }
    }
    out
}

fn changed(a: Option<&[u8]>, b: &[u8]) -> bool {
    let Some(a) = a else {
        return true;
    };
    if a.len() != b.len() {
        return true;
    }
    let differ = a
        .iter()
        .zip(b)
        .filter(|(x, y)| x.abs_diff(**y) > PIXEL_DIFF)
        .count();
    differ as f32 > CHANGED_SHARE * a.len() as f32
}

/// `picture` shrunk to `width` (never enlarged), each pixel the average of
/// the ones it covers.
#[must_use]
pub fn shrink(picture: &Picture, width: u32) -> Picture {
    if picture.width <= width {
        return picture.clone();
    }
    let height = (u64::from(picture.height) * u64::from(width) / u64::from(picture.width)).max(1);
    let height = u32::try_from(height).unwrap_or(1);
    let (sw, sh) = (picture.width as usize, picture.height as usize);
    let (dw, dh) = (width as usize, height as usize);
    let mut rgb = Vec::with_capacity(dw * dh * 3);
    for y in 0..dh {
        let (y0, y1) = (y * sh / dh, ((y + 1) * sh / dh).max(y * sh / dh + 1));
        for x in 0..dw {
            let (x0, x1) = (x * sw / dw, ((x + 1) * sw / dw).max(x * sw / dw + 1));
            let mut sum = [0_u64; 3];
            for row in y0..y1 {
                for column in x0..x1 {
                    let k = (row * sw + column) * 3;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u64::from(picture.rgb[k + channel]);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            rgb.extend(sum.map(|total| u8::try_from(total / count).unwrap_or(u8::MAX)));
        }
    }
    Picture { width, height, rgb }
}

/// Every screen in one picture, left to right in the order given, tops
/// lined up; space under a shorter screen is black. `None` when there is
/// no picture or one is malformed.
#[must_use]
pub fn side_by_side(pictures: &[Picture]) -> Option<Picture> {
    if pictures.is_empty() || !pictures.iter().all(Picture::valid) {
        return None;
    }
    let width: u32 = pictures.iter().map(|p| p.width).sum();
    let height = pictures.iter().map(|p| p.height).max()?;
    let stride = width as usize * 3;
    let mut rgb = vec![0_u8; stride * height as usize];
    let mut left = 0_usize;
    for picture in pictures {
        let row = picture.width as usize * 3;
        for (y, line) in picture.rgb.chunks_exact(row).enumerate() {
            let start = y * stride + left;
            rgb[start..start + row].copy_from_slice(line);
        }
        left += row;
    }
    Some(Picture { width, height, rgb })
}

/// Write `picture` as a PNG at `path`.
///
/// # Errors
/// The file cannot be written or the picture cannot be encoded.
pub fn write_png(path: &Path, picture: &Picture) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, picture.width, picture.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer
        .write_image_data(&picture.rgb)
        .map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)
}

/// Write `picture` to `name` in `folder` whole: a reader never sees half.
fn replace_png(folder: &Path, name: &str, picture: &Picture) -> std::io::Result<()> {
    let part = folder.join(format!("{name}.part"));
    write_png(&part, picture)?;
    std::fs::rename(part, folder.join(name))
}

/// The pictures kept so far (numbering goes on when the screen is switched
/// back on in the same recording).
fn kept_so_far(folder: &Path) -> u32 {
    std::fs::read_to_string(folder.join("frames.jsonl"))
        .map(|text| u32::try_from(text.lines().count()).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

/// Keep picture `n` of `source`, with its thumbnail and its line.
fn keep(
    folder: &Path,
    n: u32,
    picture: &Picture,
    at_ms: u64,
    source: usize,
) -> std::io::Result<()> {
    let (file, thumb) = (format!("frame-{n:04}.png"), format!("thumb-{n:04}.png"));
    write_png(&folder.join(&file), picture)?;
    write_png(&folder.join(&thumb), &shrink(picture, THUMB))?;
    let line = serde_json::json!({"file": file, "thumb": thumb, "at_ms": at_ms, "source": source});
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(folder.join("frames.jsonl"))?;
    writeln!(out, "{line}")
}

/// What the live and large pictures of `source` show now.
fn save_live(folder: &Path, picture: &Picture, source: usize, large: bool) -> std::io::Result<()> {
    replace_png(
        folder,
        &format!("live-{source}.png"),
        &shrink(picture, LIVE),
    )?;
    if large {
        replace_png(folder, &format!("now-{source}.png"), picture)?;
    }
    Ok(())
}

fn write_sources(folder: &Path, pictures: &[Picture]) -> std::io::Result<()> {
    let sources: Vec<_> = pictures
        .iter()
        .enumerate()
        .map(|(source, picture)| {
            serde_json::json!({
                "source": source,
                "kind": "monitor",
                "width": picture.width,
                "height": picture.height,
            })
        })
        .collect();
    std::fs::write(
        folder.join("sources.json"),
        serde_json::Value::Array(sources).to_string(),
    )
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Keeps a meeting's screen pictures on its own thread until stopped.
pub struct ScreenKeeper {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl ScreenKeeper {
    /// Start watching the screens into `folder` (made by the caller, private
    /// to the user).
    ///
    /// # Errors
    /// The thread could not be started.
    pub fn start(folder: PathBuf, settings: KeepSettings, grab: Grab) -> std::io::Result<Self> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("narrivy-meeting-screen".into())
            .spawn(move || run(&folder, settings, grab, &stopped))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    /// Stop watching; returns once the thread has ended (within a tick and
    /// a picture being written).
    pub fn stop(mut self) {
        self.end();
    }

    fn end(&mut self) {
        let (flag, wake) = &*self.stop;
        if let Ok(mut stopped) = flag.lock() {
            *stopped = true;
        }
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ScreenKeeper {
    fn drop(&mut self) {
        self.end();
    }
}

fn run(folder: &Path, settings: KeepSettings, mut grab: Grab, stop: &(Mutex<bool>, Condvar)) {
    // Pictures as often as the live picture needs; changes are looked for
    // every interval.
    let tick = settings
        .interval
        .min(LIVE_EVERY)
        .max(Duration::from_millis(200));
    let mut counter = kept_so_far(folder);
    let mut watched: Vec<Watch> = Vec::new();
    let mut shapes: Vec<(u32, u32)> = Vec::new();
    loop {
        let started = Instant::now();
        if let Ok(pictures) = grab() {
            let pictures: Vec<Picture> = pictures.into_iter().filter(Picture::valid).collect();
            let now = Instant::now();
            // Screens plugged in, taken away or resized: watched afresh.
            let shape: Vec<(u32, u32)> = pictures.iter().map(|p| (p.width, p.height)).collect();
            if shape != shapes {
                let _ = write_sources(folder, &pictures);
                watched = pictures.iter().map(|_| Watch::new(now)).collect();
                shapes = shape;
            }
            for (source, (picture, watch)) in pictures.iter().zip(&mut watched).enumerate() {
                let look = watch.look(now, samples(picture), &settings);
                if look.live {
                    let _ = save_live(folder, picture, source, look.large);
                }
                if look.keep {
                    counter = counter.saturating_add(1);
                    let at_ms = unix_ms().saturating_sub(settings.started_unix_ms);
                    let _ = keep(folder, counter, picture, at_ms, source);
                }
            }
        }
        let (flag, wake) = stop;
        let Ok(mut stopped) = flag.lock() else {
            return;
        };
        let rest = tick.saturating_sub(started.elapsed());
        let deadline = Instant::now() + rest;
        while !*stopped {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match wake.wait_timeout(stopped, left) {
                Ok((guard, _)) => stopped = guard,
                Err(_) => return,
            }
        }
        if *stopped {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_side_by_side_line_up_at_the_top() {
        let both = side_by_side(&[plain(2, 2, 10), plain(3, 1, 20)]);
        assert!(both.is_some(), "two screens give one picture");
        let Some(both) = both else { return };
        assert_eq!((both.width, both.height), (5, 2));
        let pixel = |x: usize, y: usize| both.rgb[(y * 5 + x) * 3];
        assert_eq!(
            [pixel(0, 0), pixel(1, 1), pixel(2, 0), pixel(4, 0)],
            [10, 10, 20, 20]
        );
        assert_eq!(pixel(3, 1), 0, "under the shorter screen is black");
        assert!(side_by_side(&[]).is_none());
        let broken = Picture {
            width: 2,
            height: 2,
            rgb: vec![0; 3],
        };
        assert!(side_by_side(&[broken]).is_none());
    }

    fn plain(width: u32, height: u32, value: u8) -> Picture {
        Picture {
            width,
            height,
            rgb: vec![value; width as usize * height as usize * 3],
        }
    }

    fn settings() -> KeepSettings {
        KeepSettings {
            interval: Duration::from_secs(2),
            moving_every: Duration::from_secs(30),
            started_unix_ms: 0,
        }
    }

    #[test]
    fn a_new_screen_is_kept_once_it_has_settled() {
        let start = Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let mut watch = Watch::new(start);
        let (white, black) = (samples(&plain(64, 36, 255)), samples(&plain(64, 36, 0)));
        // The first picture is a change; kept only when the next is the same.
        assert!(!watch.look(at(0), white.clone(), &settings()).keep);
        assert!(watch.look(at(2), white.clone(), &settings()).keep);
        // Nothing new: nothing kept.
        assert!(!watch.look(at(4), white.clone(), &settings()).keep);
        // A new slide: kept once it stays.
        assert!(!watch.look(at(6), black.clone(), &settings()).keep);
        assert!(watch.look(at(8), black, &settings()).keep);
        // Looked at again sooner than the interval: no comparison.
        assert!(!watch.look(at(9), white, &settings()).keep);
    }

    #[test]
    fn something_that_keeps_moving_is_kept_now_and_then() {
        let start = Instant::now();
        let mut watch = Watch::new(start);
        let frames = [0_u8, 120, 240];
        let mut kept = 0;
        for second in (0..=64).step_by(2) {
            let value = frames[(second / 2) % frames.len()];
            let grid = samples(&plain(64, 36, value));
            if watch
                .look(
                    start + Duration::from_secs(second as u64),
                    grid,
                    &settings(),
                )
                .keep
            {
                kept += 1;
            }
        }
        // 64 s of a picture that never settles, kept every 30 s at most.
        assert_eq!(kept, 2);
    }

    #[test]
    fn live_pictures_follow_the_screen_and_large_ones_come_less_often() {
        let start = Instant::now();
        let mut watch = Watch::new(start);
        let grid = samples(&plain(64, 36, 9));
        let looks: Vec<Look> = (0..4)
            .map(|s| watch.look(start + Duration::from_secs(s), grid.clone(), &settings()))
            .collect();
        assert!(looks.iter().all(|look| look.live));
        assert_eq!(
            looks.iter().map(|look| look.large).collect::<Vec<_>>(),
            [true, false, false, true]
        );
    }

    #[test]
    fn shrinking_averages_and_keeps_the_shape() {
        let mut picture = plain(4, 2, 0);
        // The left half white.
        for row in 0..2 {
            for column in 0..2 {
                let k = (row * 4 + column) * 3;
                picture.rgb[k..k + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let small = shrink(&picture, 2);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.rgb, [255, 255, 255, 0, 0, 0]);
        assert_eq!(shrink(&picture, 8), picture);
    }

    #[test]
    fn the_keeper_writes_what_the_linux_helper_writes() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let folder = dir.path().to_path_buf();
        let fast = KeepSettings {
            interval: Duration::from_millis(200),
            moving_every: Duration::from_secs(30),
            started_unix_ms: unix_ms(),
        };
        let grab: Grab = Box::new(|| Ok(vec![plain(64, 36, 200), plain(32, 18, 10)]));
        let keeper = ScreenKeeper::start(folder.clone(), fast, grab)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while kept_so_far(&folder) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        keeper.stop();
        let lines = std::fs::read_to_string(folder.join("frames.jsonl"))?;
        let sources: Vec<u64> = lines
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|line| line.get("source").and_then(serde_json::Value::as_u64))
            .collect();
        assert_eq!(sources, [0, 1]);
        for name in [
            "frame-0001.png",
            "thumb-0001.png",
            "frame-0002.png",
            "live-0.png",
            "live-1.png",
            "now-0.png",
            "sources.json",
        ] {
            assert!(folder.join(name).is_file(), "{name}");
        }
        let sources = std::fs::read_to_string(folder.join("sources.json"))?;
        assert!(sources.contains("\"kind\":\"monitor\""), "{sources}");
        // Switched on again: numbering goes on.
        assert_eq!(kept_so_far(&folder), 2);
        Ok(())
    }
}
