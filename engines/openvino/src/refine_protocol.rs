//! The private protocol between the daemon and `narrivy-openvino-llm-worker`.
//!
//! One request at a time over the worker's stdin and stdout. Every frame is
//! `[code u8][id u64 LE][len u32 LE][payload]`, the payload UTF-8 text of at
//! most [`MAX_TEXT_BYTES`]. The worker first writes one [`STATUS_READY`]
//! frame whose payload names the device it loaded on (`NPU`, `GPU`, `CPU`).
//! Text crosses this pipe only; neither side logs or stores it.

use std::io::{self, Read, Write};

/// Clean up one dictated text: the payload is the text.
pub const OP_CLEAN: u8 = 1;
/// One chat turn: the payload is a JSON array of `{role, content}` messages.
/// The reply streams as [`STATUS_PARTIAL`] frames, then one [`STATUS_OK`]
/// with the whole text.
pub const OP_CHAT: u8 = 2;
/// Stop the chat turn with this id (it ends with [`STATUS_CANCELLED`]).
/// Sent while a turn runs; ignored otherwise.
pub const OP_CANCEL: u8 = 3;
/// Stop the worker.
pub const OP_SHUTDOWN: u8 = 0xFF;

pub const STATUS_OK: u8 = 0;
/// The model failed to produce text.
pub const STATUS_FAILED: u8 = 1;
/// The request was malformed or too long.
pub const STATUS_INVALID: u8 = 2;
/// A piece of a chat reply, in order.
pub const STATUS_PARTIAL: u8 = 3;
/// A chat turn stopped by [`OP_CANCEL`]; the payload is what was produced.
pub const STATUS_CANCELLED: u8 = 4;
/// The worker is loaded; the payload names the device.
pub const STATUS_READY: u8 = 0x10;
/// The model or runtime could not be loaded. The worker exits after it.
pub const STATUS_LOAD_FAILED: u8 = 0x20;

/// The largest text either side sends.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// One frame.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub code: u8,
    pub id: u64,
    pub payload: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the text.
        formatter
            .debug_struct("Frame")
            .field("code", &self.code)
            .field("id", &self.id)
            .field("payload_bytes", &self.payload.len())
            .finish()
    }
}

/// Write one frame and flush it.
pub fn write_frame(writer: &mut impl Write, code: u8, id: u64, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_TEXT_BYTES {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let len =
        u32::try_from(payload.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut header = [0_u8; 13];
    header[0] = code;
    header[1..9].copy_from_slice(&id.to_le_bytes());
    header[9..13].copy_from_slice(&len.to_le_bytes());
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    writer.flush()
}

/// Read one frame. `Ok(None)` at a clean end of stream.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut header = [0_u8; 13];
    let mut filled = 0;
    while filled < header.len() {
        match reader.read(&mut header[filled..])? {
            0 if filled == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            count => filled += count,
        }
    }
    let code = header[0];
    let id = u64::from_le_bytes(
        header[1..9]
            .try_into()
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
    );
    let len = u32::from_le_bytes(
        header[9..13]
            .try_into()
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
    );
    let len = usize::try_from(len).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    if len > MAX_TEXT_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut payload = vec![0_u8; len];
    reader.read_exact(&mut payload)?;
    Ok(Some(Frame { code, id, payload }))
}

/// How a model wants to be asked, shipped beside it as
/// [`REFINE_PROFILE`] (and listed in its manifest, so it is verified).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RefineProfile {
    pub schema_version: u32,
    /// The system prompt the model was trained with.
    pub system: String,
    /// Put before the dictated text in the user message (for example a
    /// control line and a newline).
    #[serde(default)]
    pub user_prefix: String,
    /// Chat-template variables, such as `{"enable_thinking": false}`.
    #[serde(default)]
    pub extra_context: Option<serde_json::Value>,
}

/// The profile file in a language model pack.
pub const REFINE_PROFILE: &str = "narrivy-refine.json";

impl RefineProfile {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let profile: Self = serde_json::from_slice(bytes).ok()?;
        (profile.schema_version == 1 && !profile.system.is_empty()).then_some(profile)
    }

    /// The chat messages for cleaning up `text`, as JSON.
    #[must_use]
    pub fn clean_messages(&self, text: &str) -> String {
        serde_json::json!([
            {"role": "system", "content": self.system},
            {"role": "user", "content": format!("{}{text}", self.user_prefix)},
        ])
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_debug_hides_the_text() -> io::Result<()> {
        let mut wire = Vec::new();
        write_frame(&mut wire, OP_CLEAN, 7, "so um send it friday".as_bytes())?;
        write_frame(&mut wire, OP_SHUTDOWN, 8, &[])?;
        let mut reader = wire.as_slice();
        let first = read_frame(&mut reader)?.ok_or(io::ErrorKind::UnexpectedEof)?;
        assert_eq!((first.code, first.id), (OP_CLEAN, 7));
        assert_eq!(first.payload, b"so um send it friday");
        assert!(!format!("{first:?}").contains("friday"));
        let second = read_frame(&mut reader)?.ok_or(io::ErrorKind::UnexpectedEof)?;
        assert_eq!(second.code, OP_SHUTDOWN);
        assert!(read_frame(&mut reader)?.is_none());
        Ok(())
    }

    #[test]
    fn oversized_and_truncated_frames_are_refused() {
        let big = vec![b'a'; MAX_TEXT_BYTES + 1];
        assert!(write_frame(&mut Vec::new(), OP_CLEAN, 1, &big).is_err());
        let mut header = vec![OP_CLEAN];
        header.extend_from_slice(&1_u64.to_le_bytes());
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_frame(&mut header.as_slice()).is_err());
        assert!(read_frame(&mut [OP_CLEAN, 1, 2].as_slice()).is_err());
    }

    #[test]
    fn the_profile_builds_the_messages_the_model_was_trained_on() {
        let profile = RefineProfile::parse(
            br#"{"schema_version":1,"system":"Clean it.","user_prefix":"[Styling: casual]\n","extra_context":{"enable_thinking":false}}"#,
        );
        assert!(profile.is_some(), "profile should parse");
        let Some(profile) = profile else {
            return;
        };
        let messages: serde_json::Value =
            serde_json::from_str(&profile.clean_messages("um hello")).unwrap_or_default();
        assert_eq!(messages[0]["content"], "Clean it.");
        assert_eq!(messages[1]["content"], "[Styling: casual]\num hello");
        assert!(RefineProfile::parse(br#"{"schema_version":2,"system":"x"}"#).is_none());
    }
}
