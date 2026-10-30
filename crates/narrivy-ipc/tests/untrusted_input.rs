//! Property tests for the IPC decode path: everything a peer can send.
//!
//! A frame, a hello, an envelope and a command are all hostile until
//! decoded. None of them may panic, read or allocate past the bounds, or
//! loop. The case counts are kept small so `cargo test` stays fast; set
//! `PROPTEST_CASES` (see `scripts/fuzz-untrusted-input.sh`) for longer runs.

use std::io::{self, Cursor, Read};

use narrivy_ipc::framing::{FRAME_PREFIX_BYTES, MAX_FRAME_PAYLOAD_BYTES, encode_frame, read_frame};
use narrivy_ipc::{
    Command, Envelope, HandshakeHello, HandshakeResponse, IpcErrorCode, MAX_MESSAGE_BYTES,
    MessageKind, PROTOCOL_MAJOR, PROTOCOL_MINOR, RequestId, ResponsePayload, negotiate_handshake,
};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// `cases` per property, unless `PROPTEST_CASES` asks for more (see
/// `scripts/fuzz-untrusted-input.sh`). No regression files are written.
fn fuzz_config(cases: u32) -> ProptestConfig {
    let mut config = ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = cases;
    }
    config
}

/// Every command tag the daemon serves, plus a few it does not.
const COMMAND_TAGS: &[&str] = &[
    "get_status",
    "start_session",
    "stop_session",
    "toggle_session",
    "cancel_session",
    "subscribe",
    "unsubscribe",
    "list_pipelines",
    "select_pipeline",
    "get_history",
    "get_history_entry",
    "delete_history_entry",
    "set_shortcut",
    "remove_shortcut",
    "select_model",
    "set_pipeline_engine",
    "download_model",
    "remove_store_item",
    "assistant_send",
    "check_writing",
    "writing_action",
    "meeting_start",
    "get_meeting",
    "rename_meeting_speaker",
    "summarize_meeting",
    "set_audio_options",
    "set_vocabulary",
    "set_hold_key",
    "set_history_retention",
    "set_text_options",
    "import_meeting",
    "set_meeting_options",
    "assistant_tool_decision",
    "meeting_control",
    "set_pipeline_voice",
    "start_offline_model_provision",
    "request_portal_keyboard_permission",
    "transcribe_stream_v2",
    "",
    "GET_STATUS",
];

/// Field names the commands use, so random payloads reach the field
/// validators and not only the unknown-variant path.
const FIELD_NAMES: &[&str] = &[
    "id",
    "pipeline_id",
    "activation",
    "recognition",
    "binding",
    "key",
    "modifiers",
    "model_id",
    "path",
    "source_path",
    "operation_id",
    "parent_window",
    "text",
    "title",
    "messages",
    "vocabulary",
    "words",
    "dictation_language",
    "dictation_languages",
    "delivery_method",
    "cleanup_idle_minutes",
    "retention",
    "turn_id",
    "speaker",
    "name",
    "rules",
    "action",
];

/// Any JSON value, a few levels deep.
fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>().prop_map(Value::from),
        ".{0,40}".prop_map(Value::String),
        "[a-z_-]{0,12}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 48, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::btree_map(".{0,12}", inner, 0..8)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

/// A command-shaped object: a known or unknown tag and command fields with
/// random values.
fn command_value() -> impl Strategy<Value = Value> {
    (
        prop::sample::select(COMMAND_TAGS),
        prop::collection::vec((prop::sample::select(FIELD_NAMES), json_value()), 0..6),
    )
        .prop_map(|(tag, fields)| {
            let mut map = Map::new();
            map.insert("command".into(), Value::String(tag.into()));
            for (name, value) in fields {
                map.insert(name.into(), value);
            }
            Value::Object(map)
        })
}

/// `[[[...]]]` (or `{"a":{"a":...}}`) nested `depth` times.
fn nested_json(depth: usize, objects: bool) -> String {
    let (open, close) = if objects {
        (r#"{"a":"#, "}")
    } else {
        ("[", "]")
    };
    let mut out = String::with_capacity(depth * (open.len() + close.len()) + 1);
    for _ in 0..depth {
        out.push_str(open);
    }
    out.push('1');
    for _ in 0..depth {
        out.push_str(close);
    }
    out
}

fn envelope_bytes(kind: &str, minor: u64, payload: &str) -> Vec<u8> {
    format!(
        r#"{{"version_major":{PROTOCOL_MAJOR},"version_minor":{minor},"kind":"{kind}","request_id":1,"payload":{payload}}}"#
    )
    .into_bytes()
}

/// A reader that hands out at most `step` bytes per call, interleaved with
/// `Interrupted`, and counts what it gave.
struct Trickle {
    data: Vec<u8>,
    at: usize,
    step: usize,
    interrupt_next: bool,
}

impl Read for Trickle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.interrupt_next = !self.interrupt_next;
        if self.interrupt_next {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let n = buf.len().min(self.step).min(self.data.len() - self.at);
        buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// A reader that declares a length and then never ends, to show an
/// over-bound length is refused before the body is read.
struct Endless {
    prefix: [u8; FRAME_PREFIX_BYTES],
    given: usize,
}

impl Read for Endless {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        while n < buf.len() {
            buf[n] = self.prefix.get(self.given).copied().unwrap_or(b'x');
            self.given += 1;
            n += 1;
        }
        Ok(n)
    }
}

proptest! {
    #![proptest_config(fuzz_config(256))]

    /// Arbitrary bytes on the wire: a typed result, never a panic, and a
    /// payload no larger than the bound or than what the prefix declared.
    #[test]
    fn read_frame_never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
        match read_frame(&mut Cursor::new(&bytes)) {
            Ok(payload) => {
                prop_assert!(bytes.len() >= FRAME_PREFIX_BYTES);
                let declared = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
                prop_assert_eq!(payload.len(), declared);
                prop_assert_eq!(&payload[..], &bytes[FRAME_PREFIX_BYTES..FRAME_PREFIX_BYTES + declared]);
            }
            Err(err) => prop_assert!(matches!(
                err.code,
                IpcErrorCode::Cancelled | IpcErrorCode::BadRequest | IpcErrorCode::PayloadTooLarge
            )),
        }
    }

    /// Any declared length over the bound is refused after the four prefix
    /// bytes, even when the peer would keep sending forever.
    #[test]
    fn oversized_length_is_refused_before_the_body(len in (MAX_FRAME_PAYLOAD_BYTES as u32 + 1)..=u32::MAX) {
        let mut reader = Endless { prefix: len.to_le_bytes(), given: 0 };
        let err = read_frame(&mut reader).err();
        prop_assert_eq!(err.map(|e| e.code), Some(IpcErrorCode::PayloadTooLarge));
        prop_assert_eq!(reader.given, FRAME_PREFIX_BYTES);
    }

    /// Every strict prefix of a valid frame is a typed truncation (or a
    /// polite close at zero bytes), however the bytes are split up.
    #[test]
    fn truncated_frames_are_typed(
        payload in prop::collection::vec(any::<u8>(), 0..256),
        cut in any::<prop::sample::Index>(),
        step in 1usize..7,
    ) {
        let frame = encode_frame(&payload).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let cut = cut.index(frame.len());
        let mut reader = Trickle { data: frame[..cut].to_vec(), at: 0, step, interrupt_next: false };
        let code = read_frame(&mut reader).err().map(|e| e.code);
        let expected = if cut == 0 { IpcErrorCode::Cancelled } else { IpcErrorCode::BadRequest };
        prop_assert_eq!(code, Some(expected));

        // The whole frame, trickled, still decodes.
        let mut reader = Trickle { data: frame, at: 0, step, interrupt_next: false };
        prop_assert_eq!(read_frame(&mut reader).ok(), Some(payload));
    }

    /// Arbitrary bytes as an envelope or a hello: typed errors only.
    /// Most of these are not UTF-8 at all.
    #[test]
    fn envelope_and_hello_decode_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        if let Err(err) = Envelope::from_json_bytes(&bytes) {
            prop_assert!(matches!(
                err.code,
                IpcErrorCode::BadRequest | IpcErrorCode::IncompatibleProtocol | IpcErrorCode::PayloadTooLarge
            ));
        }
        let _ = serde_json::from_slice::<HandshakeHello>(&bytes);
    }

    /// Invalid UTF-8 inside a JSON string is refused, wherever it sits.
    #[test]
    fn invalid_utf8_in_strings_is_refused(
        prefix in "[a-z]{0,8}",
        bad in prop::sample::select(vec![&[0xffu8][..], &[0xc0, 0x80], &[0xed, 0xa0, 0x80], &[0xe2, 0x82], &[0xf8, 0x88, 0x80, 0x80, 0x80]]),
        in_kind in any::<bool>(),
    ) {
        let mut bytes = Vec::new();
        if in_kind {
            bytes.extend_from_slice(format!(r#"{{"version_major":{PROTOCOL_MAJOR},"version_minor":0,"kind":"request.{prefix}"#).as_bytes());
            bytes.extend_from_slice(bad);
            bytes.extend_from_slice(br#"","request_id":1,"payload":null}"#);
        } else {
            bytes.extend_from_slice(format!(r#"{{"version_major":{PROTOCOL_MAJOR},"version_minor":0,"kind":"request.get_status","request_id":1,"payload":{{"command":"get_status","x":"{prefix}"#).as_bytes());
            bytes.extend_from_slice(bad);
            bytes.extend_from_slice(br#""}}"#);
        }
        let err = Envelope::from_json_bytes(&bytes).err().map(|e| e.code);
        prop_assert_eq!(err, Some(IpcErrorCode::BadRequest));
    }

    /// Random command-shaped payloads, as the server decodes them: an
    /// envelope first, then the command. Never a panic.
    #[test]
    fn command_parsing_never_panics(value in command_value(), minor in any::<u16>()) {
        let bytes = envelope_bytes("request.get_status", u64::from(minor), &value.to_string());
        if let Ok(envelope) = Envelope::from_json_bytes(&bytes) {
            if let Ok(command) = envelope.parse_payload::<Command>() {
                // A command that decodes also encodes and decodes the same.
                let again = serde_json::to_value(&command)
                    .map_err(|e| TestCaseError::fail(e.to_string()))?;
                let back: Command = serde_json::from_value(again)
                    .map_err(|e| TestCaseError::fail(e.to_string()))?;
                prop_assert!(back == command);
                // Debug output is used in logs; it must not panic either.
                let _ = format!("{command:?}");
            }
        }
        let _ = serde_json::from_value::<ResponsePayload>(value);
    }

    /// Unknown kinds decode as `Other` and are never client requests.
    #[test]
    fn unknown_kinds_are_not_requests(tag in "[a-z._]{1,40}") {
        let bytes = envelope_bytes(&tag, 0, "null");
        if let Ok(envelope) = Envelope::from_json_bytes(&bytes) {
            if let MessageKind::Other(raw) = &envelope.kind {
                prop_assert_eq!(raw, &tag);
                prop_assert!(!envelope.kind.is_request());
            }
        }
    }

    /// Any minor within this major is accepted with the daemon's minor;
    /// any other major is refused with a typed error, in a hello or an
    /// envelope.
    #[test]
    fn protocol_versions_are_typed(major in any::<u16>(), minor in any::<u16>()) {
        let hello = HandshakeHello {
            protocol_major: major,
            protocol_minor: minor,
            build_identity: "test".into(),
            client_role: "cli".into(),
        };
        match negotiate_handshake(&hello) {
            HandshakeResponse::Accepted { protocol_minor, .. } => {
                prop_assert_eq!(major, PROTOCOL_MAJOR);
                prop_assert_eq!(protocol_minor, PROTOCOL_MINOR);
            }
            HandshakeResponse::Rejected { code, .. } => {
                prop_assert_ne!(major, PROTOCOL_MAJOR);
                prop_assert_eq!(code, IpcErrorCode::IncompatibleProtocol);
            }
        }
        let bytes = format!(
            r#"{{"version_major":{major},"version_minor":{minor},"kind":"request.get_status","request_id":1,"payload":{{"command":"get_status"}}}}"#
        );
        let result = Envelope::from_json_bytes(bytes.as_bytes());
        if major == PROTOCOL_MAJOR {
            prop_assert!(result.is_ok());
        } else {
            prop_assert_eq!(result.err().map(|e| e.code), Some(IpcErrorCode::IncompatibleProtocol));
        }
    }

    /// Version numbers past `u16` are refused, not wrapped.
    #[test]
    fn out_of_range_versions_are_refused(minor in (u64::from(u16::MAX) + 1)..u64::MAX) {
        let bytes = envelope_bytes("request.get_status", minor, r#"{"command":"get_status"}"#);
        prop_assert_eq!(Envelope::from_json_bytes(&bytes).err().map(|e| e.code), Some(IpcErrorCode::BadRequest));
    }
}

/// Nesting at every depth up to far past the limits: refused or accepted,
/// never a stack overflow. The envelope's own limit is 64 levels of
/// payload; the JSON parser stops at 128.
#[test]
fn deeply_nested_json_is_refused_without_overflow() -> TestResult {
    for objects in [false, true] {
        for depth in [
            1, 32, 63, 64, 65, 66, 100, 127, 128, 129, 200, 10_000, 200_000,
        ] {
            let payload = nested_json(depth, objects);
            let bytes = envelope_bytes("request.get_status", 0, &payload);
            let decoded = Envelope::from_json_bytes(&bytes);
            if bytes.len() > MAX_MESSAGE_BYTES {
                assert_eq!(
                    decoded.err().map(|e| e.code),
                    Some(IpcErrorCode::PayloadTooLarge)
                );
                continue;
            }
            // The payload sits one level inside the envelope object.
            if depth <= 64 {
                let envelope = decoded?;
                assert!(envelope.parse_payload::<Command>().is_err());
            } else {
                assert_eq!(
                    decoded.err().map(|e| e.code),
                    Some(IpcErrorCode::BadRequest),
                    "depth {depth} objects={objects}"
                );
            }
            // Straight into a command and a hello, as other decoders do.
            assert!(serde_json::from_str::<Command>(&payload).is_err());
            assert!(serde_json::from_str::<HandshakeHello>(&payload).is_err());
        }
    }
    Ok(())
}

/// A deep value built in memory (not parsed) is refused by the bound check
/// before it can reach the wire, and dropping it does not overflow.
#[test]
fn deep_in_memory_payload_is_refused() {
    let mut value = json!(1);
    for _ in 0..100 {
        value = json!([value]);
    }
    let err = Envelope::new(MessageKind::GetStatus, RequestId(1))
        .with_payload(&value)
        .err()
        .map(|e| e.code);
    assert_eq!(err, Some(IpcErrorCode::BadRequest));
}

/// The frame bound and the message bound agree, so a frame that decodes is
/// never too large for an envelope purely by size.
#[test]
fn frame_and_message_bounds_agree() {
    assert_eq!(MAX_FRAME_PAYLOAD_BYTES, MAX_MESSAGE_BYTES);
}
