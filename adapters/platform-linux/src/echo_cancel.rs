//! Echo cancellation: the microphone without the computer's own sound.
//!
//! When a video or a call plays through the speakers, the microphone hears it
//! too, and the other voices end up in the dictation. PipeWire's echo-cancel
//! module (WebRTC's canceller) removes from the microphone whatever the
//! default output is playing. It runs here as a private PipeWire client
//! process owned by the daemon, so nothing in the user's audio setup changes:
//!
//! - it reads the default microphone and, in monitor mode, the default
//!   output's sound as the reference;
//! - it offers one extra source, [`SOURCE`], which capture targets instead of
//!   the default microphone;
//! - it has the lowest priority, so it never becomes the default source;
//! - stopping the process (or dropping [`EchoCancel`]) removes it again.
//!
//! When it cannot start, callers fall back to the plain microphone.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The echo-cancelled microphone's PipeWire node name.
pub const SOURCE: &str = "narrivy.aec.source";

/// How long the module may take to offer its source.
const START_BOUND: Duration = Duration::from_secs(4);

/// How much background noise the canceller removes as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Noise {
    /// Echo only.
    Off,
    /// WebRTC's noise suppression (its default).
    Standard,
    /// Also its stronger, newer noise suppressor.
    Strong,
}

impl Noise {
    /// The canceller's arguments for this level.
    fn args(self) -> &'static str {
        match self {
            Self::Off => "webrtc.noise_suppression = false webrtc.experimental_ns = false",
            Self::Standard => "webrtc.noise_suppression = true webrtc.experimental_ns = false",
            Self::Strong => "webrtc.noise_suppression = true webrtc.experimental_ns = true",
        }
    }
}

const CONFIG: &str = r#"# Written by Narrivy: echo cancellation for the microphone.
context.properties = { log.level = 2 }
context.spa-libs = {
  audio.convert.* = audioconvert/libspa-audioconvert
  support.*       = support/libspa-support
}
context.modules = [
  { name = libpipewire-module-protocol-native }
  { name = libpipewire-module-client-node }
  { name = libpipewire-module-adapter }
  { name = libpipewire-module-echo-cancel
    args = {
      library.name   = aec/libspa-aec-webrtc
      aec.args       = { @NOISE@ }
      monitor.mode   = true
      audio.channels = 1
      audio.position = [ MONO ]
      capture.props  = { node.name = "narrivy.aec.capture" node.passive = true @MIC@ }
      sink.props     = { node.name = "narrivy.aec.reference" node.passive = true }
      source.props   = {
        node.name        = "narrivy.aec.source"
        node.description = "Narrivy: microphone without computer sound"
        priority.session = 0
        priority.driver  = 0
      }
    }
  }
]
"#;

/// The running echo canceller. Dropping it stops the process.
#[derive(Debug)]
pub struct EchoCancel {
    child: Child,
}

impl EchoCancel {
    /// Start the echo canceller, writing its configuration under `dir`.
    ///
    /// # Errors
    /// A short code: PipeWire or its echo-cancel module missing, or the
    /// source not offered in time.
    /// `microphone` is a source's node name, or `None` for the default.
    pub fn start(dir: &Path, noise: Noise, microphone: Option<&str>) -> Result<Self, &'static str> {
        std::fs::create_dir_all(dir).map_err(|_| "echo_cancel_unavailable")?;
        let config = dir.join("echo-cancel.conf");
        std::fs::write(&config, config_text(noise, microphone))
            .map_err(|_| "echo_cancel_unavailable")?;
        // A daemon that was killed leaves its canceller behind: end it, so
        // two sources never share the name.
        stop_leftovers(&config);
        let child = Command::new("pipewire")
            .arg("-c")
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "echo_cancel_missing")?;
        let mut running = Self { child };
        let deadline = Instant::now() + START_BOUND;
        while Instant::now() < deadline {
            if !running.is_running() {
                return Err("echo_cancel_failed");
            }
            if source_offered() {
                return Ok(running);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("echo_cancel_timeout")
    }

    /// Whether the process is still there (it ends if PipeWire restarts).
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for EchoCancel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn config_text(noise: Noise, microphone: Option<&str>) -> String {
    let target = microphone
        .filter(|name| valid_node_name(name))
        .map(|name| format!("target.object = \"{name}\""))
        .unwrap_or_default();
    CONFIG.replace("@NOISE@", noise.args()).replace("@MIC@", &target)
}

/// PipeWire node names as `pactl` lists them: no quotes, spaces or controls.
#[must_use]
pub fn valid_node_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-:@".contains(&b))
}

#[derive(serde::Deserialize)]
struct Device {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    properties: std::collections::BTreeMap<String, serde_json::Value>,
}

/// PipeWire's `sources` or `sinks`, as PulseAudio reports them.
fn devices(kind: &str) -> Vec<Device> {
    Command::new("pactl")
        .args(["--format=json", "list", kind])
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|out| serde_json::from_slice(&out.stdout).ok())
        .unwrap_or_default()
}

fn named(device: Device) -> (String, String) {
    let description = if device.description.is_empty() {
        device.name.clone()
    } else {
        device.description
    };
    (device.name, description)
}

/// The microphones (sources that are not an output's monitor, nor this
/// canceller's own source), as `(node name, description)`.
#[must_use]
pub fn microphones() -> Vec<(String, String)> {
    devices("sources")
        .into_iter()
        .filter(|s| {
            let class = s.properties.get("device.class").and_then(|v| v.as_str());
            s.name != SOURCE && class != Some("monitor") && !s.name.ends_with(".monitor")
        })
        .filter(|s| valid_node_name(&s.name))
        .map(named)
        .collect()
}

/// The outputs (speakers, headphones, headsets), as `(node name,
/// description)`. Their sound can be recorded, which is how a meeting hears
/// the other side of a call.
#[must_use]
pub fn speakers() -> Vec<(String, String)> {
    devices("sinks")
        .into_iter()
        .filter(|s| !s.name.starts_with("narrivy.") && valid_node_name(&s.name))
        .map(named)
        .collect()
}

fn source_offered() -> bool {
    Command::new("pactl")
        .args(["list", "short", "sources"])
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|line| line.split('\t').nth(1) == Some(SOURCE))
        })
}

/// End `pipewire -c <config>` processes of this user left by an earlier
/// daemon. Only processes started with exactly this configuration match.
fn stop_leftovers(config: &Path) {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    let wanted = config.as_os_str().as_encoded_bytes();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
        let is_ours = args.first().is_some_and(|a| a.ends_with(b"pipewire"))
            && args.get(1) == Some(&b"-c".as_slice())
            && args.get(2) == Some(&wanted);
        if is_ours && pid != std::process::id() {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
    }
}

/// Where the daemon keeps the canceller's configuration.
#[must_use]
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("narrivy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_names_the_source_and_keeps_it_out_of_the_default() {
        assert!(CONFIG.contains(&format!("node.name        = \"{SOURCE}\"")));
        assert!(CONFIG.contains("monitor.mode   = true"));
        assert!(CONFIG.contains("priority.session = 0"));
    }

    #[test]
    fn noise_levels_reach_the_canceller() {
        assert!(config_text(Noise::Off, None).contains("aec.args       = { webrtc.noise_suppression = false"));
        assert!(config_text(Noise::Strong, None).contains("webrtc.experimental_ns = true"));
        assert!(!config_text(Noise::Standard, None).contains("@NOISE@"));
        assert!(!config_text(Noise::Standard, None).contains("@MIC@"));
        assert!(config_text(Noise::Standard, Some("alsa_input.usb-x.mono"))
            .contains("target.object = \"alsa_input.usb-x.mono\""));
        // A name that could break the file is never written.
        assert!(!config_text(Noise::Standard, Some("a\" b")).contains("target.object"));
    }
}
