//! LEGACY Phase-0 scaffold types.
//!
//! Superseded by [`crate::PipelineDefinition`] + [`crate::CapabilitySlot`]
//! (ADR-0016 first-class pipelines). Kept so Phase-0 scaffolds keep compiling;
//! the migration path is documented in `IMPLEMENTATION-NOTES.md`. New code
//! must use `PipelineDefinition`.

use narrivy_types::ProcessingBoundary;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    AudioEnhancement,
    Asr,
    ContextResolve,
    ConfidenceRepair,
    DeterministicFormat,
    SemanticTransform,
    Deliver,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineStage {
    pub id: String,
    pub kind: StageKind,
    pub boundary: ProcessingBoundary,
    pub optional: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipelinePlan {
    pub stages: Vec<PipelineStage>,
}

impl PipelinePlan {
    /// Legacy helper, still fully functional. The first-class equivalent is a
    /// policy gate over each node's boundary plus the pipeline's own privacy
    /// document; see `crates/narrivy-policy`.
    pub fn requires_remote_processing(&self) -> bool {
        self.stages
            .iter()
            .any(|stage| stage.boundary == ProcessingBoundary::Remote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_plan_requires_remote_processing_still_works() {
        let plan = PipelinePlan {
            stages: vec![PipelineStage {
                id: String::from("s1"),
                kind: StageKind::Asr,
                boundary: ProcessingBoundary::Local,
                optional: false,
            }],
        };
        assert!(!plan.requires_remote_processing());
        let remote = PipelinePlan {
            stages: vec![PipelineStage {
                id: String::from("s2"),
                kind: StageKind::ConfidenceRepair,
                boundary: ProcessingBoundary::Remote,
                optional: true,
            }],
        };
        assert!(remote.requires_remote_processing());
    }
}
