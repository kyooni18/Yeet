//! Passive execution-phase telemetry derived from coordinator-observed facts.
//!
//! This module does not prescribe the model's next action. It names the runtime's
//! current execution state for metadata, checkpoints, diagnostics, and evaluation.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentPhase {
    Understand,
    Inspect,
    Act,
    Observe,
    Verify,
    Finish,
}

impl AgentPhase {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Understand => "understand",
            Self::Inspect => "inspect",
            Self::Act => "act",
            Self::Observe => "observe",
            Self::Verify => "verify",
            Self::Finish => "finish",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PhaseState {
    pub observed_tool_calls: usize,
    pub successful_mutations: usize,
    pub unresolved_failed_mutation: bool,
    pub verification_succeeded: bool,
    pub has_inspection_evidence: bool,
}

pub(super) fn recommend(
    state: PhaseState,
    implementation_requested: bool,
    planning_or_documentation: bool,
) -> AgentPhase {
    if state.observed_tool_calls == 0 {
        AgentPhase::Understand
    } else if state.unresolved_failed_mutation {
        AgentPhase::Act
    } else if implementation_requested {
        if state.successful_mutations == 0 {
            if state.has_inspection_evidence {
                AgentPhase::Act
            } else {
                AgentPhase::Inspect
            }
        } else if !planning_or_documentation && !state.verification_succeeded {
            AgentPhase::Verify
        } else {
            AgentPhase::Finish
        }
    } else {
        AgentPhase::Observe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_telemetry_tracks_observed_execution_state_without_routing_actions() {
        let mut state = PhaseState {
            observed_tool_calls: 0,
            successful_mutations: 0,
            unresolved_failed_mutation: false,
            verification_succeeded: false,
            has_inspection_evidence: false,
        };
        assert_eq!(recommend(state, true, false), AgentPhase::Understand);

        state.observed_tool_calls = 1;
        assert_eq!(recommend(state, true, false), AgentPhase::Inspect);

        state.has_inspection_evidence = true;
        assert_eq!(recommend(state, true, false), AgentPhase::Act);

        state.successful_mutations = 1;
        assert_eq!(recommend(state, true, false), AgentPhase::Verify);

        state.verification_succeeded = true;
        assert_eq!(recommend(state, true, false), AgentPhase::Finish);

        assert_eq!(recommend(state, false, false), AgentPhase::Observe);
    }
}
