//! The microphone and the speakers.
//!
//! - Linux: PipeWire's own `pw-record` and `pw-play` (from the system, by
//!   absolute path), which can name a source such as Narrivy's
//!   echo-cancelled microphone.
//! - Windows: WASAPI through `cpal`, with the chosen microphone's endpoint
//!   id, or the default one.
//!
//! Capture hands over 16 kHz mono pieces; playback takes any rate. Nothing
//! is written to disk.

use std::sync::mpsc::Receiver;

/// A running capture. Dropping it stops the microphone.
pub struct Capture {
    /// 16 kHz mono pieces of about 64 ms; closes when the microphone stops.
    pub pieces: Receiver<Vec<f32>>,
    #[cfg(target_os = "linux")]
    child: std::process::Child,
    #[cfg(windows)]
    _stop: std::sync::mpsc::Sender<()>,
}

/// Where sound goes: opened at a rate, written in small pieces.
pub trait Output: Send {
    /// Make ready for `rate` Hz (reopening if the rate changes).
    fn open(&mut self, rate: u32) -> bool;
    /// Queue samples; false when the output is gone.
    fn write(&mut self, samples: &[f32]) -> bool;
    /// Stop at once, dropping what is queued.
    fn close(&mut self);
}

/// Samples per capture piece at 16 kHz (two Silero windows).
pub const PIECE: usize = 1024;

/// The only variables the worker passes on to `pw-record` and `pw-play`:
/// PipeWire's same-user session routing. The daemon starts the worker with
/// an empty environment and names these as `--env KEY=VALUE`.
pub use crate::ROUTING_KEYS;

/// Routing variables for the PipeWire tools (ignored on Windows).
pub type Routing = Vec<(String, String)>;

#[cfg(target_os = "linux")]
mod linux {
    use std::io::{Read, Write};
    use std::process::{Child, Command, Stdio};

    use super::{Capture, Output, PIECE};

    const PW_RECORD: &str = "/usr/bin/pw-record";
    const PW_PLAY: &str = "/usr/bin/pw-play";

    pub fn capture(
        target: Option<&str>,
        routing: &super::Routing,
    ) -> Result<Capture, &'static str> {
        let mut child = Command::new(PW_RECORD)
            .env_clear()
            .envs(routing.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .args([
                "--raw",
                "--rate",
                "16000",
                "--channels",
                "1",
                "--format",
                "f32",
            ])
            .args(["--target", target.unwrap_or("@DEFAULT_AUDIO_SOURCE@"), "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "voice_microphone_unavailable")?;
        let mut stdout = child.stdout.take().ok_or("voice_microphone_unavailable")?;
        let (sender, pieces) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("voice-capture".into())
            .spawn(move || {
                let mut bytes = vec![0_u8; PIECE * 4];
                loop {
                    if stdout.read_exact(&mut bytes).is_err() {
                        return;
                    }
                    let piece = bytes
                        .chunks_exact(4)
                        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        .collect();
                    if sender.send(piece).is_err() {
                        return;
                    }
                }
            })
            .map_err(|_| "voice_microphone_unavailable")?;
        Ok(Capture { pieces, child })
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// `pw-play` reading raw `f32` from a pipe.
    pub struct PwPlay {
        target: Option<String>,
        routing: super::Routing,
        child: Option<(Child, u32)>,
    }

    impl PwPlay {
        pub fn new(target: Option<String>, routing: super::Routing) -> Self {
            Self {
                target,
                routing,
                child: None,
            }
        }
    }

    impl Output for PwPlay {
        fn open(&mut self, rate: u32) -> bool {
            if let Some((child, open_rate)) = self.child.as_mut()
                && *open_rate == rate
                && child.try_wait().ok().flatten().is_none()
            {
                return true;
            }
            self.close();
            let mut command = Command::new(PW_PLAY);
            command
                .env_clear()
                .envs(self.routing.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .args(["--raw", "--rate", &rate.to_string(), "--channels", "1"])
                .args(["--format", "f32", "--latency", "50ms"]);
            if let Some(target) = &self.target {
                command.args(["--target", target]);
            }
            match command
                .arg("-")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => {
                    self.child = Some((child, rate));
                    true
                }
                Err(_) => false,
            }
        }

        fn write(&mut self, samples: &[f32]) -> bool {
            let Some((child, _)) = self.child.as_mut() else {
                return false;
            };
            let Some(stdin) = child.stdin.as_mut() else {
                return false;
            };
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            stdin.write_all(&bytes).and_then(|()| stdin.flush()).is_ok()
        }

        fn close(&mut self) {
            if let Some((mut child, _)) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for PwPlay {
        fn drop(&mut self) {
            self.close();
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    use super::{Capture, Output, PIECE};
    use crate::resample;

    fn input_device(endpoint: Option<&str>) -> Option<cpal::Device> {
        let host = cpal::default_host();
        endpoint
            .and_then(|id| format!("wasapi:{id}").parse::<cpal::DeviceId>().ok())
            .and_then(|id| host.device_by_id(&id))
            .or_else(|| host.default_input_device())
    }

    pub fn capture(endpoint: Option<&str>) -> Result<Capture, &'static str> {
        let endpoint = endpoint.map(str::to_owned);
        let (sender, pieces) = std::sync::mpsc::channel::<Vec<f32>>();
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let (ready, started) = std::sync::mpsc::channel::<Result<(), &'static str>>();
        std::thread::Builder::new()
            .name("voice-capture".into())
            .spawn(move || {
                let opened = (|| {
                    let device =
                        input_device(endpoint.as_deref()).ok_or("voice_microphone_unavailable")?;
                    let config = device
                        .default_input_config()
                        .map_err(|_| "voice_microphone_unavailable")?;
                    let rate = config.sample_rate();
                    let channels = usize::from(config.channels());
                    let format = config.sample_format();
                    let stream_config: cpal::StreamConfig = config.into();
                    let mut pending: Vec<f32> = Vec::new();
                    let mut hand_over = move |mono: Vec<f32>| {
                        pending.extend(resample::linear(&mono, rate, crate::RATE));
                        while pending.len() >= PIECE {
                            let rest = pending.split_off(PIECE);
                            let piece = std::mem::replace(&mut pending, rest);
                            let _ = sender.send(piece);
                        }
                    };
                    let stream = match format {
                        cpal::SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
                            stream_config,
                            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                                hand_over(resample::mono(data, channels));
                            },
                            |_| {},
                            None,
                        ),
                        cpal::SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
                            stream_config,
                            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                                let floats: Vec<f32> =
                                    data.iter().map(|s| f32::from(*s) / 32_768.0).collect();
                                hand_over(resample::mono(&floats, channels));
                            },
                            |_| {},
                            None,
                        ),
                        _ => return Err("voice_microphone_unsupported"),
                    }
                    .map_err(|_| "voice_microphone_unavailable")?;
                    stream.play().map_err(|_| "voice_microphone_unavailable")?;
                    Ok(stream)
                })();
                match opened {
                    Ok(stream) => {
                        let _ = ready.send(Ok(()));
                        // Keep the stream until the capture is dropped.
                        let _ = stopped.recv();
                        drop(stream);
                    }
                    Err(code) => {
                        let _ = ready.send(Err(code));
                    }
                }
            })
            .map_err(|_| "voice_microphone_unavailable")?;
        started
            .recv()
            .map_err(|_| "voice_microphone_unavailable")??;
        Ok(Capture {
            pieces,
            _stop: stop,
        })
    }

    struct Shared {
        queue: VecDeque<f32>,
        rate: u32,
    }

    /// The default output device, fed from a queue at its own rate.
    pub struct Wasapi {
        shared: Arc<Mutex<Shared>>,
        device_rate: u32,
        source_rate: u32,
        _stop: Option<std::sync::mpsc::Sender<()>>,
    }

    impl Wasapi {
        pub fn new() -> Self {
            let shared = Arc::new(Mutex::new(Shared {
                queue: VecDeque::new(),
                rate: 0,
            }));
            let (stop, stopped) = std::sync::mpsc::channel::<()>();
            let (ready, started) = std::sync::mpsc::channel::<Option<u32>>();
            let feed = Arc::clone(&shared);
            let spawned = std::thread::Builder::new()
                .name("voice-playback".into())
                .spawn(move || {
                    let opened = (|| {
                        let device = cpal::default_host().default_output_device()?;
                        let config = device.default_output_config().ok()?;
                        let rate = config.sample_rate();
                        let channels = usize::from(config.channels());
                        if config.sample_format() != cpal::SampleFormat::F32 {
                            return None;
                        }
                        let stream = device
                            .build_output_stream::<f32, _, _>(
                                config.into(),
                                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                                    let mut shared = match feed.lock() {
                                        Ok(shared) => shared,
                                        Err(poisoned) => poisoned.into_inner(),
                                    };
                                    for frame in data.chunks_mut(channels) {
                                        let value = shared.queue.pop_front().unwrap_or(0.0);
                                        frame.fill(value);
                                    }
                                },
                                |_| {},
                                None,
                            )
                            .ok()?;
                        stream.play().ok()?;
                        Some((stream, rate))
                    })();
                    match opened {
                        Some((stream, rate)) => {
                            let _ = ready.send(Some(rate));
                            let _ = stopped.recv();
                            drop(stream);
                        }
                        None => {
                            let _ = ready.send(None);
                        }
                    }
                });
            let device_rate = spawned
                .ok()
                .and_then(|_| started.recv().ok().flatten())
                .unwrap_or(0);
            if let Ok(mut shared) = shared.lock() {
                shared.rate = device_rate;
            }
            Self {
                shared,
                device_rate,
                source_rate: 0,
                _stop: (device_rate > 0).then_some(stop),
            }
        }
    }

    impl Output for Wasapi {
        fn open(&mut self, rate: u32) -> bool {
            self.source_rate = rate;
            self.device_rate > 0
        }

        fn write(&mut self, samples: &[f32]) -> bool {
            if self.device_rate == 0 {
                return false;
            }
            let converted = resample::linear(samples, self.source_rate, self.device_rate);
            match self.shared.lock() {
                Ok(mut shared) => {
                    shared.queue.extend(converted);
                    true
                }
                Err(_) => false,
            }
        }

        fn close(&mut self) {
            if let Ok(mut shared) = self.shared.lock() {
                shared.queue.clear();
            }
        }
    }
}

/// Start listening on `microphone` (a PipeWire node name on Linux, a
/// WASAPI endpoint id on Windows), or the default one.
#[cfg(target_os = "linux")]
pub fn capture(microphone: Option<&str>, routing: &Routing) -> Result<Capture, &'static str> {
    linux::capture(microphone, routing)
}

/// Start listening on `microphone` (a WASAPI endpoint id), or the default
/// one.
#[cfg(windows)]
pub fn capture(microphone: Option<&str>, _routing: &Routing) -> Result<Capture, &'static str> {
    windows::capture(microphone)
}

/// The speakers (`speaker` names a PipeWire sink).
#[cfg(target_os = "linux")]
#[must_use]
pub fn output(speaker: Option<String>, routing: Routing) -> Box<dyn Output> {
    Box::new(linux::PwPlay::new(speaker, routing))
}

/// The default output device (Windows does not take a named one yet).
#[cfg(windows)]
#[must_use]
pub fn output(_speaker: Option<String>, _routing: Routing) -> Box<dyn Output> {
    Box::new(windows::Wasapi::new())
}
