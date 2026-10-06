//! Per-application agent UI projection and delivery-safe state checkpoints.
//! Hosts perform Harness delivery between prepare and commit; UI contains no I/O.
use super::agents::{AgentAction, AgentEffect, AgentState, AgentsView};
use crate::harness::HarnessState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProjection {
    pub agents_revision: u64,
    pub view: AgentsView,
}

pub struct PreparedAgentAction {
    pub action: AgentAction,
    state: AgentState,
    pub effect: AgentEffect,
}

pub struct AgentSession {
    state: AgentState,
    projection: AgentProjection,
    serialized: serde_json::Value,
}

impl AgentSession {
    pub fn new(harness: &HarnessState) -> Self {
        let state = AgentState::default();
        let view = state.view(harness);
        let serialized = serde_json::to_value(&view).expect("serialize agent UI view");
        Self {
            state,
            projection: AgentProjection {
                agents_revision: 0,
                view,
            },
            serialized,
        }
    }

    pub fn state(&self) -> &AgentState {
        &self.state
    }

    /// Transitional access for native panels migrating field-by-field. Call
    /// `refresh` after editing to reconcile selection and update projections.
    pub fn compatibility_state_mut(&mut self) -> &mut AgentState {
        &mut self.state
    }

    pub fn projection(&self) -> AgentProjection {
        self.projection.clone()
    }

    pub fn prepare(&self, action: AgentAction, harness: &HarnessState) -> PreparedAgentAction {
        let mut state = self.state.clone();
        let effect = state.apply(action.clone(), harness);
        PreparedAgentAction {
            action,
            state,
            effect,
        }
    }

    /// Call only after any generated command was successfully delivered.
    pub fn commit(
        &mut self,
        prepared: PreparedAgentAction,
        harness: &HarnessState,
    ) -> (AgentProjection, AgentEffect) {
        self.state = prepared.state;
        self.state.reconcile(&harness.agent_group);
        self.projection.agents_revision = self.projection.agents_revision.saturating_add(1);
        self.projection.view = self.state.view(harness);
        self.serialized =
            serde_json::to_value(&self.projection.view).expect("serialize agent UI view");
        (self.projection(), prepared.effect)
    }

    /// Runtime content changes update the same view while preserving local UI intent.
    pub fn refresh(&mut self, harness: &HarnessState) -> Option<AgentProjection> {
        self.state.reconcile(&harness.agent_group);
        let view = self.state.view(harness);
        let serialized = serde_json::to_value(&view).expect("serialize agent UI view");
        if serialized == self.serialized {
            return None;
        }
        self.serialized = serialized;
        self.projection.agents_revision = self.projection.agents_revision.saturating_add(1);
        self.projection.view = view;
        Some(self.projection())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsuccessful_delivery_preserves_draft_and_runtime_refresh_keeps_intent() {
        let mut harness = HarnessState::default();
        let mut session = AgentSession::new(&harness);
        let prepared = session.prepare(AgentAction::CreateGroup, &harness);
        session.commit(prepared, &harness);
        let before = session.projection();
        let failed = session.prepare(AgentAction::SubmitDraft("objective".into()), &harness);
        assert!(failed.effect.command.is_some());
        drop(failed); // A host whose delivery failed must not commit the candidate.
        assert!(session.projection().view.state.creating_group);
        assert_eq!(session.projection().agents_revision, before.agents_revision);
        harness.agent_group.status = "created".into();
        assert!(session.refresh(&harness).is_some());
        assert!(session.projection().view.state.creating_group);
        assert!(session.refresh(&harness).is_none());
        let delivered = session.prepare(AgentAction::SubmitDraft("objective".into()), &harness);
        let (_, effect) = session.commit(delivered, &harness);
        assert_eq!(effect.submitted_text.as_deref(), Some("objective"));
        assert!(!session.projection().view.state.creating_group);
    }
}
