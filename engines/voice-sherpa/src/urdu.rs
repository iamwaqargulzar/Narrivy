//! The Urdu voice, spoken by its own program (`narrivy-urdu-speaker`).
//!
//! Urdu needs eSpeak NG, which is GPL-3.0, so it is not linked into this
//! worker (D51, ADR-0082). The daemon checks the Urdu program and its voice
//! pack and names both (`--urdu-speaker`, `--urdu-voice`); this side starts
//! it with an empty environment, sends one line of text per sentence and
//! reads the audio back. When it fails it is not started again: Urdu falls
//! back to the English voice for the rest of the session.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use base64::Engine as _;
use serde_json::{Value, json};

/// Longest line the Urdu program may answer with (about 60 s of audio).
const MAX_LINE: usize = 16 << 20;

/// A running Urdu speaker.
pub struct Urdu {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    /// Its output rate, from its first line.
    pub rate: u32,
}

impl Urdu {
    /// Start `program` on the voice pack `voice`.
    pub fn start(program: &Path, voice: &Path, threads: i32) -> Result<Self, &'static str> {
        let mut command = Command::new(program);
        command
            .env_clear()
            .arg("--voice")
            .arg(voice)
            .args(["--threads", &threads.clamp(1, 8).to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|_| "urdu_speaker_failed")?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("urdu_speaker_failed");
        };
        let mut urdu = Self {
            child,
            input,
            output: BufReader::new(output),
            rate: 0,
        };
        let ready = urdu.line().ok_or("urdu_speaker_failed")?;
        urdu.rate = ready
            .get("rate")
            .and_then(Value::as_u64)
            .and_then(|rate| u32::try_from(rate).ok())
            .filter(|rate| *rate > 0 && ready.get("ready") == Some(&Value::Bool(true)))
            .ok_or("urdu_voice_invalid")?;
        Ok(urdu)
    }

    fn line(&mut self) -> Option<Value> {
        let mut line = String::new();
        let read = std::io::Read::take(&mut self.output, MAX_LINE as u64)
            .read_line(&mut line)
            .ok()?;
        (read > 0 && line.ends_with('\n'))
            .then(|| serde_json::from_str(&line).ok())
            .flatten()
    }

    /// `(samples, rate)` for `text` at `speed`; `None` when it failed.
    pub fn speak(&mut self, text: &str, speed: f32) -> Option<(Vec<f32>, u32)> {
        let request = json!({"text": text, "speed": speed});
        writeln!(self.input, "{request}").ok()?;
        self.input.flush().ok()?;
        let answer = self.line()?;
        let rate = answer
            .get("rate")
            .and_then(Value::as_u64)
            .and_then(|rate| u32::try_from(rate).ok())?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(answer.get("pcm")?.as_str()?)
            .ok()?;
        if bytes.len() % 4 != 0 {
            return None;
        }
        let samples = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        Some((samples, rate))
    }
}

impl Drop for Urdu {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What the daemon named for Urdu: the program and the voice pack.
#[derive(Clone, Debug)]
pub struct UrduFiles {
    pub program: PathBuf,
    pub voice: PathBuf,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn fake(dir: &Path, body: &str) -> std::io::Result<PathBuf> {
        let path = dir.join("speaker");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        Ok(path)
    }

    #[test]
    fn audio_comes_back_as_samples() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        // Two samples, 0.5 and -1.0, little-endian f32, in base64.
        let program = fake(
            dir.path(),
            r#"echo '{"ready":true,"rate":22050}'; read line; echo '{"rate":22050,"pcm":"AAAAPwAAgL8="}'"#,
        )?;
        let mut urdu = Urdu::start(&program, dir.path(), 2).map_err(std::io::Error::other)?;
        assert_eq!(urdu.rate, 22050);
        assert_eq!(urdu.speak("سلام", 1.0), Some((vec![0.5, -1.0], 22050)));
        // It has ended: the next sentence fails instead of hanging.
        assert_eq!(urdu.speak("سلام", 1.0), None);
        Ok(())
    }

    #[test]
    fn a_speaker_that_is_not_ready_is_refused() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let program = fake(dir.path(), r#"echo '{"error":"urdu_voice_missing"}'"#)?;
        assert!(Urdu::start(&program, dir.path(), 2).is_err());
        Ok(())
    }
}
