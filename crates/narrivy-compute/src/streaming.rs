//! Streaming-support honesty (FUN-003 / PERF-005).
//!
//! Claims about streaming are explicit and testable. The transcribe.cpp
//! sidecar is `SimulatedFinalOnly` until upstream native streaming is
//! confirmed; PERF-005 (<250 ms first partial) is therefore recorded as
//! NotApplicable-with-reason rather than faked.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamingSupport {
    /// Engine emits genuine incremental partials while audio streams in.
    Native,
    /// Engine only produces final text per chunk; any "partial" shown is a
    /// simulated final-only update, not incremental decoding.
    SimulatedFinalOnly,
    /// No usable streaming behavior at all.
    Unavailable,
}

impl StreamingSupport {
    /// Can the engine produce a genuine first partial under PERF-005?
    pub fn supports_first_partial(self) -> bool {
        matches!(self, StreamingSupport::Native)
    }

    /// Whether this variant is native streaming.
    pub fn is_native(self) -> bool {
        matches!(self, StreamingSupport::Native)
    }

    /// String label for persistence/UI.
    pub fn as_str(self) -> &'static str {
        match self {
            StreamingSupport::Native => "native",
            StreamingSupport::SimulatedFinalOnly => "simulated_final_only",
            StreamingSupport::Unavailable => "unavailable",
        }
    }

    /// PERF-005 verdict for this support level. Returns `None` (NotApplicable)
    /// plus a reason when the measurement would be dishonest.
    pub fn perf005_applicability(self) -> Result<(), &'static str> {
        match self {
            StreamingSupport::Native => Ok(()),
            StreamingSupport::SimulatedFinalOnly => Err(
                "engine is SimulatedFinalOnly: no genuine first partial exists, \
                 so PERF-005 (<250 ms first partial) is NotApplicable",
            ),
            StreamingSupport::Unavailable => {
                Err("engine has no streaming support at all: PERF-005 is NotApplicable")
            }
        }
    }
}

/// Per-engine registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamingSupportRegistry {
    entries: std::collections::BTreeMap<String, StreamingSupport>,
}

impl Default for StreamingSupportRegistry {
    fn default() -> Self {
        let mut entries = std::collections::BTreeMap::new();
        // Truthful today: transcribe.cpp exposes chunked feeding
        // (--stream-chunk-ms) but emits final-only text per chunk until
        // upstream native partials are confirmed upstream.
        entries.insert(
            "transcribe.cpp".to_string(),
            StreamingSupport::SimulatedFinalOnly,
        );
        Self { entries }
    }
}

impl StreamingSupportRegistry {
    pub fn set(&mut self, engine: &str, support: StreamingSupport) {
        self.entries.insert(engine.to_string(), support);
    }

    pub fn get(&self, engine: &str) -> Option<StreamingSupport> {
        self.entries.get(engine).copied()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // test scaffolding only
mod tests {
    use super::*;

    #[test]
    fn transcribe_cpp_is_simulated_final_only() {
        let reg = StreamingSupportRegistry::default();
        assert_eq!(
            reg.get("transcribe.cpp"),
            Some(StreamingSupport::SimulatedFinalOnly)
        );
    }

    #[test]
    fn perf005_not_applicable_with_reason_for_simulated_final_only() {
        let support = StreamingSupportRegistry::default()
            .get("transcribe.cpp")
            .unwrap();
        assert!(!support.supports_first_partial());
        let reason = support.perf005_applicability().unwrap_err();
        assert!(reason.contains("NotApplicable"));
        assert!(reason.contains("SimulatedFinalOnly") || reason.contains("no genuine"));
    }

    #[test]
    fn native_support_makes_perf005_applicable() {
        let mut reg = StreamingSupportRegistry::default();
        reg.set("future-native-engine", StreamingSupport::Native);
        let support = reg.get("future-native-engine").unwrap();
        assert!(support.supports_first_partial());
        assert!(support.perf005_applicability().is_ok());
    }
}

#[cfg(test)]
mod streaming_disruptor_tests {
    use super::*;

    #[test]
    fn as_str_roundtrips() {
        assert_eq!(StreamingSupport::Native.as_str(), "native");
        assert_eq!(
            StreamingSupport::SimulatedFinalOnly.as_str(),
            "simulated_final_only"
        );
        assert_eq!(StreamingSupport::Unavailable.as_str(), "unavailable");
    }

    #[test]
    fn is_native_only_for_native() {
        assert!(StreamingSupport::Native.is_native());
        assert!(!StreamingSupport::SimulatedFinalOnly.is_native());
        assert!(!StreamingSupport::Unavailable.is_native());
    }

    #[test]
    fn as_str_and_is_native_agree_with_supports_first_partial() {
        for support in [
            StreamingSupport::Native,
            StreamingSupport::SimulatedFinalOnly,
            StreamingSupport::Unavailable,
        ] {
            assert_eq!(support.is_native(), support.supports_first_partial());
            assert!(!support.as_str().is_empty());
        }
    }
}
