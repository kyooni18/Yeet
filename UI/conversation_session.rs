//! Shared transcript intent and delivery-safe command checkpoints.
use super::conversation::{
    ConversationAction, ConversationEffect, ConversationState, ConversationView,
};
use crate::harness::HarnessState;
use crate::model::ConversationEntry;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationProjection {
    pub conversation_revision: u64,
    pub view: ConversationView,
}
pub struct PreparedConversationAction {
    state: ConversationState,
    pub effect: ConversationEffect,
}
pub struct ConversationSession {
    state: ConversationState,
    projection: ConversationProjection,
    serialized: serde_json::Value,
}
impl ConversationSession {
    pub fn new(harness: &HarnessState) -> Self {
        let mut state = ConversationState::default();
        let view = state.project(harness.conversation.as_deref().unwrap_or_default(), harness);
        let serialized = serde_json::to_value(&view).expect("serialize conversation UI");
        Self {
            state,
            projection: ConversationProjection {
                conversation_revision: 0,
                view,
            },
            serialized,
        }
    }
    pub fn state(&self) -> &ConversationState {
        &self.state
    }
    pub fn projection(&self) -> ConversationProjection {
        self.projection.clone()
    }
    pub fn prepare(
        &self,
        action: ConversationAction,
        harness: &HarnessState,
    ) -> PreparedConversationAction {
        self.prepare_entries(
            action,
            harness.conversation.as_deref().unwrap_or_default(),
            harness,
        )
    }
    pub fn prepare_entries(
        &self,
        action: ConversationAction,
        entries: &[ConversationEntry],
        harness: &HarnessState,
    ) -> PreparedConversationAction {
        let mut state = self.state.clone();
        let view = state.project(entries, harness);
        let effect = state.apply(action, &view, harness);
        PreparedConversationAction { state, effect }
    }
    /// Publish intent only after any Harness command was accepted by the host.
    pub fn commit(
        &mut self,
        prepared: PreparedConversationAction,
        harness: &HarnessState,
    ) -> (ConversationProjection, ConversationEffect) {
        self.commit_entries(
            prepared,
            harness.conversation.as_deref().unwrap_or_default(),
            harness,
        )
    }
    pub fn commit_entries(
        &mut self,
        prepared: PreparedConversationAction,
        entries: &[ConversationEntry],
        harness: &HarnessState,
    ) -> (ConversationProjection, ConversationEffect) {
        self.state = prepared.state;
        self.publish(entries, harness, true);
        (self.projection(), prepared.effect)
    }
    pub fn refresh(&mut self, harness: &HarnessState) -> Option<ConversationProjection> {
        self.refresh_entries(harness.conversation.as_deref().unwrap_or_default(), harness)
    }
    pub fn refresh_entries(
        &mut self,
        entries: &[ConversationEntry],
        harness: &HarnessState,
    ) -> Option<ConversationProjection> {
        self.publish(entries, harness, false)
            .then(|| self.projection())
    }
    fn publish(
        &mut self,
        entries: &[ConversationEntry],
        harness: &HarnessState,
        force: bool,
    ) -> bool {
        let view = self.state.project(entries, harness);
        let serialized = serde_json::to_value(&view).expect("serialize conversation UI");
        if !force && serialized == self.serialized {
            return false;
        }
        self.projection.conversation_revision =
            self.projection.conversation_revision.saturating_add(1);
        self.projection.view = view;
        self.serialized = serialized;
        true
    }
}
